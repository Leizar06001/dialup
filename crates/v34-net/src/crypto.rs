//! Petits outils cryptographiques pour la signalisation du central.
//!
//! - HMAC-SHA256 (RFC 2104) construit sur `sha2` : quelques lignes, pas
//!   besoin d'une dépendance de plus. Vérifié avec les vecteurs RFC 4231.
//! - Comparaison en temps constant (pour ne pas révéler, par le temps de
//!   réponse, combien d'octets d'un MAC sont corrects).
//! - Aléa du système (`getrandom`) pour les nonces et les identifiants.

use sha2::{Digest, Sha256};

const BLOCK: usize = 64;

/// HMAC-SHA256 de la concaténation de `parts` avec la clé `key`.
pub fn hmac_sha256(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5Cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let mut h = Sha256::new();
    h.update(ipad);
    for p in parts {
        h.update(p);
    }
    let inner = h.finalize();
    let mut h = Sha256::new();
    h.update(opad);
    h.update(inner);
    h.finalize().into()
}

/// Égalité en temps constant (pour la longueur donnée).
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Octets aléatoires du système (panique si le système n'en fournit pas,
/// ce qui n'arrive pas sur les plateformes visées).
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("générateur aléatoire du système indisponible");
    b
}

/// Entier aléatoire non nul (identifiants de session / d'appel).
pub fn random_u32_nonzero() -> u32 {
    loop {
        let v = u32::from_be_bytes(random_bytes::<4>());
        if v != 0 {
            return v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn rfc4231_cas_1_2_6() {
        // Cas 1
        let key = [0x0bu8; 20];
        assert_eq!(
            hex(&hmac_sha256(&key, &[b"Hi There"])),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        // Cas 2 (message découpé en plusieurs morceaux)
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", &[b"what do ya want ", b"for nothing?"])),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // Cas 6 : clé plus longue qu'un bloc
        let key = [0xaau8; 131];
        assert_eq!(
            hex(&hmac_sha256(&key, &[b"Test Using Larger Than Block-Size Key - Hash Key First"])),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn comparaison_temps_constant() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
    }

    #[test]
    fn alea() {
        assert_ne!(random_bytes::<16>(), random_bytes::<16>());
        assert_ne!(random_u32_nonzero(), 0);
    }
}
