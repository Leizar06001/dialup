//! Authentification des abonnés du central, façon SCRAM-SHA-256 (RFC 5802).
//!
//! Le serveur ne stocke PAS le mot de passe, ni rien qui suffise à se faire
//! passer pour l'abonné :
//! ```text
//! mot_de_passe_salé = PBKDF2-HMAC-SHA256(mot_de_passe, sel, itérations)
//! clé_client        = HMAC(mot_de_passe_salé, "Client Key")
//! clé_stockée       = SHA-256(clé_client)          ← seule chose dans la config
//! ```
//! Échange (le mot de passe ne circule jamais) :
//! ```text
//! client  → REGISTER(nom)
//! serveur → CHALLENGE(nonce_s, sel, itérations)
//! client  → AUTH(nom, nonce_s, nonce_c, preuve)
//!           message   = "V34-AUTH\0" ‖ len ‖ nom ‖ nonce_s ‖ nonce_c
//!           signature = HMAC(clé_stockée, message)
//!           preuve    = clé_client XOR signature
//! serveur : clé_client' = preuve XOR signature ; accepte si SHA-256(clé_client') = clé_stockée
//! clé de session = HMAC(clé_stockée, "V34-SESSION\0" ‖ message)
//! ```
//! Un fichier de configuration volé ne permet donc pas de s'authentifier
//! (il faudrait clé_client), seulement de tenter une attaque par
//! dictionnaire, ralentie par PBKDF2.

use sha2::{Digest, Sha256};

use crate::crypto::{ct_eq, hmac_sha256, random_bytes};
use crate::signal::{SessionKey, CLIENT_NONCE_LEN, SERVER_NONCE_LEN};

/// Itérations PBKDF2 par défaut pour les nouveaux mots de passe.
pub const DEFAULT_ITERATIONS: u32 = 100_000;
/// Bornes acceptées (le client refuse de calculer plus : un faux serveur
/// pourrait sinon lui faire brûler du CPU).
pub const MIN_ITERATIONS: u32 = 1_000;
pub const MAX_ITERATIONS: u32 = 5_000_000;
pub const SALT_LEN: usize = 16;
pub const MAX_SALT_LEN: usize = 32;
const SCHEME: &str = "scram-sha256";

/// PBKDF2-HMAC-SHA256 pour une clé de 32 octets (un seul bloc).
/// Les états intérieur/extérieur du HMAC sont précalculés : 2 compressions
/// SHA-256 par itération.
pub fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    let mut k = [0u8; 64];
    if password.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(password));
    } else {
        k[..password.len()].copy_from_slice(password);
    }
    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5Cu8; 64];
    for i in 0..64 {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let inner = Sha256::new_with_prefix(ipad);
    let outer = Sha256::new_with_prefix(opad);
    let prf = |data: &[u8]| -> [u8; 32] {
        let mut h = inner.clone();
        h.update(data);
        let ih = h.finalize();
        let mut h = outer.clone();
        h.update(ih);
        h.finalize().into()
    };
    let mut first = salt.to_vec();
    first.extend_from_slice(&1u32.to_be_bytes());
    let mut u = prf(&first);
    let mut t = u;
    for _ in 1..iterations.max(1) {
        u = prf(&u);
        for (a, b) in t.iter_mut().zip(&u) {
            *a ^= b;
        }
    }
    t
}

fn client_key(salted: &[u8; 32]) -> [u8; 32] {
    hmac_sha256(salted, &[b"Client Key"])
}

fn stored_key(client_key: &[u8; 32]) -> [u8; 32] {
    Sha256::digest(client_key).into()
}

fn auth_message(name: &str, sn: &[u8; SERVER_NONCE_LEN], cn: &[u8; CLIENT_NONCE_LEN]) -> Vec<u8> {
    let mut m = b"V34-AUTH\0".to_vec();
    m.push(name.len() as u8);
    m.extend_from_slice(name.as_bytes());
    m.extend_from_slice(sn);
    m.extend_from_slice(cn);
    m
}

fn session_key(stored: &[u8; 32], msg: &[u8]) -> SessionKey {
    hmac_sha256(stored, &[b"V34-SESSION\0", msg])
}

/// Côté client : mot de passe salé (calcul lent, à mettre en cache).
pub fn salted_password(password: &str, salt: &[u8], iterations: u32) -> [u8; 32] {
    pbkdf2_sha256(password.as_bytes(), salt, iterations)
}

/// Côté client : preuve à envoyer dans AUTH, et clé de session.
pub fn client_proof(
    salted: &[u8; 32],
    name: &str,
    server_nonce: &[u8; SERVER_NONCE_LEN],
    client_nonce: &[u8; CLIENT_NONCE_LEN],
) -> ([u8; 32], SessionKey) {
    let ck = client_key(salted);
    let sk = stored_key(&ck);
    let msg = auth_message(name, server_nonce, client_nonce);
    let sig = hmac_sha256(&sk, &[&msg]);
    let mut proof = [0u8; 32];
    for i in 0..32 {
        proof[i] = ck[i] ^ sig[i];
    }
    (proof, session_key(&sk, &msg))
}

/// Côté serveur : vérifie la preuve ; renvoie la clé de session si elle est bonne.
pub fn verify_proof(
    stored: &[u8; 32],
    name: &str,
    server_nonce: &[u8; SERVER_NONCE_LEN],
    client_nonce: &[u8; CLIENT_NONCE_LEN],
    proof: &[u8; 32],
) -> Option<SessionKey> {
    let msg = auth_message(name, server_nonce, client_nonce);
    let sig = hmac_sha256(stored, &[&msg]);
    let mut ck = [0u8; 32];
    for i in 0..32 {
        ck[i] = proof[i] ^ sig[i];
    }
    ct_eq(&stored_key(&ck), stored).then(|| session_key(stored, &msg))
}

/// Ce que la configuration conserve pour un abonné :
/// `scram-sha256$<itérations>$<sel hex>$<clé stockée hex>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PasswordHash {
    pub iterations: u32,
    pub salt: Vec<u8>,
    pub stored_key: [u8; 32],
}

impl PasswordHash {
    /// Hache un nouveau mot de passe (sel aléatoire).
    pub fn new(password: &str, iterations: u32) -> PasswordHash {
        let salt: [u8; SALT_LEN] = random_bytes();
        Self::with_salt(password, &salt, iterations)
    }

    pub fn with_salt(password: &str, salt: &[u8], iterations: u32) -> PasswordHash {
        let salted = salted_password(password, salt, iterations);
        PasswordHash { iterations, salt: salt.to_vec(), stored_key: stored_key(&client_key(&salted)) }
    }

    pub fn parse(s: &str) -> Result<PasswordHash, String> {
        let parts: Vec<&str> = s.trim().split('$').collect();
        let [scheme, iter, salt, key] = parts[..] else {
            return Err("format attendu : scram-sha256$<itérations>$<sel>$<clé>".into());
        };
        if scheme != SCHEME {
            return Err(format!("schéma inconnu « {scheme} » (attendu {SCHEME})"));
        }
        let iterations: u32 = iter.parse().map_err(|_| "nombre d'itérations invalide".to_string())?;
        if !(MIN_ITERATIONS..=MAX_ITERATIONS).contains(&iterations) {
            return Err(format!("itérations hors bornes ({MIN_ITERATIONS} à {MAX_ITERATIONS})"));
        }
        let salt = from_hex(salt).ok_or("sel invalide (hexadécimal)")?;
        if salt.is_empty() || salt.len() > MAX_SALT_LEN {
            return Err(format!("sel de 1 à {MAX_SALT_LEN} octets"));
        }
        let key = from_hex(key).ok_or("clé invalide (hexadécimal)")?;
        let stored_key: [u8; 32] = key.try_into().map_err(|_| "clé de 32 octets attendue".to_string())?;
        Ok(PasswordHash { iterations, salt, stored_key })
    }
}

impl std::fmt::Display for PasswordHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{SCHEME}${}${}${}", self.iterations, to_hex(&self.salt), to_hex(&self.stored_key))
    }
}

pub fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// Nom d'abonné valide : 1 à 32 caractères parmi `A-Z a-z 0-9 . _ -`.
pub fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 32 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pbkdf2_vecteurs_rfc7914() {
        // RFC 7914 §11 : PBKDF2-HMAC-SHA256("passwd", "salt", 1), 32 premiers octets
        assert_eq!(
            to_hex(&pbkdf2_sha256(b"passwd", b"salt", 1)),
            "55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc"
        );
        // RFC 7914 §11 : ("Password", "NaCl", 80000)
        assert_eq!(
            to_hex(&pbkdf2_sha256(b"Password", b"NaCl", 80000)),
            "4ddcd8f60b98be21830cee5ef22701f9641a4418d04c0414aeff08876b34ab56"
        );
    }

    #[test]
    fn preuve_acceptee_seulement_avec_le_bon_mot_de_passe() {
        let h = PasswordHash::new("ouvre-toi sésame", MIN_ITERATIONS);
        let sn = [1u8; SERVER_NONCE_LEN];
        let cn = [2u8; CLIENT_NONCE_LEN];
        let salted = salted_password("ouvre-toi sésame", &h.salt, h.iterations);
        let (proof, k_client) = client_proof(&salted, "gaston", &sn, &cn);
        assert_eq!(verify_proof(&h.stored_key, "gaston", &sn, &cn, &proof), Some(k_client));
        // autre nom, autre nonce, autre mot de passe → refusé
        assert_eq!(verify_proof(&h.stored_key, "raphaël", &sn, &cn, &proof), None);
        assert_eq!(verify_proof(&h.stored_key, "gaston", &[9; SERVER_NONCE_LEN], &cn, &proof), None);
        let bad = salted_password("mauvais", &h.salt, h.iterations);
        let (proof, _) = client_proof(&bad, "gaston", &sn, &cn);
        assert_eq!(verify_proof(&h.stored_key, "gaston", &sn, &cn, &proof), None);
        // la clé stockée seule ne permet pas de fabriquer une preuve valide
        let (forged, _) = client_proof(&h.stored_key, "gaston", &sn, &cn);
        assert_eq!(verify_proof(&h.stored_key, "gaston", &sn, &cn, &forged), None);
    }

    #[test]
    fn format_texte() {
        let h = PasswordHash::with_salt("x", b"0123456789abcdef", 1000);
        let s = h.to_string();
        assert!(s.starts_with("scram-sha256$1000$30313233"));
        assert_eq!(PasswordHash::parse(&s), Ok(h));
        assert!(PasswordHash::parse("md5$1$aa$bb").is_err());
        assert!(PasswordHash::parse("scram-sha256$10$aa$00").is_err(), "trop peu d'itérations");
        assert!(PasswordHash::parse("scram-sha256$1000$zz$00").is_err());
        assert!(PasswordHash::parse("scram-sha256$1000$aa$0011").is_err(), "clé trop courte");
    }

    #[test]
    fn noms() {
        assert!(valid_name("gaston"));
        assert!(valid_name("jean-paul.2_b"));
        assert!(!valid_name(""));
        assert!(!valid_name("avec espace"));
        assert!(!valid_name("élodie"));
        assert!(!valid_name(&"a".repeat(33)));
    }
}
