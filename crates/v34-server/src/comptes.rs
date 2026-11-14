//! Comptes créés par le site d'inscription (`[portail]`).
//!
//! Ils vivent dans leur propre fichier (par défaut
//! `/var/lib/v34-server/comptes.toml`), écrit par le serveur : la
//! configuration de l'administrateur n'est jamais réécrite. Le central les
//! ajoute à l'annuaire de la configuration, qui garde la priorité.
//!
//! Cycle de vie :
//! 1. inscription : en attente de validation de l'e-mail (48 h), avec
//!    l'empreinte d'un jeton envoyé par e-mail (le jeton lui-même n'est
//!    jamais stocké) ;
//! 2. validation : le compte est créé, **sans accès Internet** ;
//! 3. l'administrateur accorde (ou non) l'accès Internet (FAI).
//!
//! Le fichier contient aussi le secret du site (signature des liens envoyés
//! à l'administrateur et des sessions) : il doit rester lisible par le seul
//! serveur (le service tourne avec `UMask=0077`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use v34_net::auth::{from_hex, to_hex};
use v34_net::crypto::random_bytes;

/// Durée de validité d'une inscription non confirmée (s)
pub const ATTENTE_MAX_S: u64 = 48 * 3600;
/// Inscriptions en attente au maximum (toutes adresses confondues)
const ATTENTE_MAX: usize = 500;

pub fn maintenant() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Un compte validé.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Compte {
    pub nom: String,
    pub numero: String,
    pub mot_de_passe_hash: String,
    pub courriel: String,
    /// Accès Internet (FAI) accordé par l'administrateur
    pub fai: bool,
    /// L'administrateur a déjà répondu à la demande (accordé ou refusé)
    #[serde(default)]
    pub fai_decide: bool,
    /// Création (secondes Unix)
    pub cree: u64,
}

/// Une inscription en attente de validation de l'e-mail.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inscription {
    pub nom: String,
    pub numero: String,
    pub mot_de_passe_hash: String,
    pub courriel: String,
    /// SHA-256 (hex) du jeton envoyé par e-mail
    pub jeton: String,
    pub expire: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fichier {
    /// Secret du site (hex, 32 octets)
    #[serde(default)]
    secret: String,
    #[serde(default)]
    compte: Vec<Compte>,
    #[serde(default)]
    inscription: Vec<Inscription>,
}

pub struct Comptes {
    path: Option<PathBuf>,
    data: Mutex<Fichier>,
    secret: [u8; 32],
    /// Augmente à chaque modification des comptes validés (le central
    /// recharge alors son annuaire)
    generation: AtomicU64,
}

impl std::fmt::Debug for Comptes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Comptes").field("path", &self.path).finish_non_exhaustive()
    }
}

fn empreinte(jeton: &str) -> String {
    to_hex(&Sha256::digest(jeton.as_bytes()))
}

/// Lit les comptes validés d'un fichier, sans rien écrire (pour le
/// central ; seul le site écrit ce fichier). Fichier absent : aucun compte.
pub fn lire_comptes(path: &Path) -> Result<Vec<Compte>, String> {
    match std::fs::read_to_string(path) {
        Ok(t) => toml::from_str::<Fichier>(&t).map(|f| f.compte).map_err(|e| format!("{} : {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("lecture de {} : {e}", path.display())),
    }
}

impl Comptes {
    /// Ouvre (ou crée) le fichier des comptes.
    pub fn ouvrir(path: &Path) -> Result<Arc<Comptes>, String> {
        let mut f: Fichier = match std::fs::read_to_string(path) {
            Ok(t) => toml::from_str(&t).map_err(|e| format!("{} : {e}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Fichier::default(),
            Err(e) => return Err(format!("lecture de {} : {e}", path.display())),
        };
        let neuf = f.secret.is_empty();
        let secret = Self::secret_de(&mut f)?;
        let c = Comptes { path: Some(path.to_path_buf()), data: Mutex::new(f), secret, generation: AtomicU64::new(1) };
        if neuf {
            let d = c.data.lock().unwrap();
            c.ecrire(&d)?;
        }
        Ok(Arc::new(c))
    }

    /// Comptes sans fichier (tests).
    pub fn en_memoire() -> Arc<Comptes> {
        let mut f = Fichier::default();
        let secret = Self::secret_de(&mut f).expect("secret neuf");
        Arc::new(Comptes { path: None, data: Mutex::new(f), secret, generation: AtomicU64::new(1) })
    }

    fn secret_de(f: &mut Fichier) -> Result<[u8; 32], String> {
        if f.secret.is_empty() {
            f.secret = to_hex(&random_bytes::<32>());
        }
        from_hex(&f.secret).and_then(|v| v.try_into().ok()).ok_or_else(|| "secret du fichier des comptes invalide".to_string())
    }

    /// Secret du site (signatures HMAC).
    pub fn secret(&self) -> &[u8; 32] {
        &self.secret
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    /// Copie des comptes validés.
    pub fn liste(&self) -> Vec<Compte> {
        self.data.lock().unwrap().compte.clone()
    }

    pub fn compte(&self, nom: &str) -> Option<Compte> {
        self.data.lock().unwrap().compte.iter().find(|c| c.nom == nom).cloned()
    }

    /// Inscriptions en attente (non expirées).
    pub fn en_attente(&self) -> Vec<Inscription> {
        let now = maintenant();
        self.data.lock().unwrap().inscription.iter().filter(|i| i.expire > now).cloned().collect()
    }

    /// Le nom, le numéro ou l'e-mail est-il déjà pris par un compte du site
    /// ou une inscription en attente ? Renvoie l'élément en conflit.
    pub fn conflit(&self, nom: &str, numero: &str, courriel: &str) -> Option<&'static str> {
        let now = maintenant();
        let d = self.data.lock().unwrap();
        let comptes = d.compte.iter().map(|c| (&c.nom, &c.numero, &c.courriel));
        let attente = d.inscription.iter().filter(|i| i.expire > now).map(|i| (&i.nom, &i.numero, &i.courriel));
        for (n, num, mail) in comptes.chain(attente) {
            if n.eq_ignore_ascii_case(nom) {
                return Some("nom");
            }
            if num == numero {
                return Some("numero");
            }
            if mail.eq_ignore_ascii_case(courriel) {
                return Some("courriel");
            }
        }
        None
    }

    /// Enregistre une inscription en attente ; renvoie le jeton à envoyer
    /// par e-mail. Les vérifications (format, disponibilité) sont faites
    /// avant par l'appelant.
    pub fn inscrire(&self, nom: &str, numero: &str, mot_de_passe_hash: &str, courriel: &str) -> Result<String, String> {
        let jeton = to_hex(&random_bytes::<24>());
        let now = maintenant();
        self.modifier(false, |d| {
            d.inscription.retain(|i| i.expire > now);
            if d.inscription.len() >= ATTENTE_MAX {
                return Err("trop d'inscriptions en attente, réessayez plus tard".to_string());
            }
            d.inscription.push(Inscription {
                nom: nom.into(),
                numero: numero.into(),
                mot_de_passe_hash: mot_de_passe_hash.into(),
                courriel: courriel.into(),
                jeton: empreinte(&jeton),
                expire: now + ATTENTE_MAX_S,
            });
            Ok(())
        })?;
        Ok(jeton)
    }

    /// L'inscription correspondant à un jeton (sans la valider).
    pub fn inscription(&self, jeton: &str) -> Option<Inscription> {
        let e = empreinte(jeton);
        let now = maintenant();
        self.data.lock().unwrap().inscription.iter().find(|i| i.jeton == e && i.expire > now).cloned()
    }

    /// Valide l'e-mail : l'inscription devient un compte (sans accès
    /// Internet). `libre(nom, numero)` revérifie la disponibilité auprès de
    /// l'annuaire de la configuration (qui a pu changer entre-temps).
    pub fn valider(&self, jeton: &str, libre: impl Fn(&str, &str) -> bool) -> Result<Compte, String> {
        let e = empreinte(jeton);
        let now = maintenant();
        self.modifier(true, |d| {
            let i = d.inscription.iter().position(|i| i.jeton == e && i.expire > now).ok_or("lien invalide ou expiré (48 h)")?;
            let ins = d.inscription.remove(i);
            if !libre(&ins.nom, &ins.numero) || d.compte.iter().any(|c| c.nom.eq_ignore_ascii_case(&ins.nom) || c.numero == ins.numero) {
                return Err("ce nom ou ce numéro a été attribué entre-temps : recommencez l'inscription".to_string());
            }
            let c = Compte {
                nom: ins.nom,
                numero: ins.numero,
                mot_de_passe_hash: ins.mot_de_passe_hash,
                courriel: ins.courriel,
                fai: false,
                fai_decide: false,
                cree: now,
            };
            d.compte.push(c.clone());
            Ok(c)
        })
    }

    /// Accorde (ou retire) l'accès Internet.
    pub fn regler_fai(&self, nom: &str, fai: bool) -> Result<Compte, String> {
        self.modifier(true, |d| {
            let c = d.compte.iter_mut().find(|c| c.nom == nom).ok_or("compte inconnu")?;
            c.fai = fai;
            c.fai_decide = true;
            Ok(c.clone())
        })
    }

    /// Change le mot de passe d'un compte du site.
    pub fn changer_mot_de_passe(&self, nom: &str, hash: &str) -> Result<(), String> {
        self.modifier(true, |d| {
            let c = d.compte.iter_mut().find(|c| c.nom == nom).ok_or("compte inconnu")?;
            c.mot_de_passe_hash = hash.into();
            Ok(())
        })
    }

    pub fn supprimer(&self, nom: &str) -> Result<Compte, String> {
        self.modifier(true, |d| {
            let i = d.compte.iter().position(|c| c.nom == nom).ok_or("compte inconnu")?;
            Ok(d.compte.remove(i))
        })
    }

    /// Applique une modification puis réécrit le fichier (de façon
    /// atomique). En cas d'échec d'écriture, la modification est annulée.
    fn modifier<R>(&self, comptes_changes: bool, f: impl FnOnce(&mut Fichier) -> Result<R, String>) -> Result<R, String> {
        let mut d = self.data.lock().unwrap();
        let avant = toml::to_string(&*d).map_err(|e| e.to_string())?;
        let r = f(&mut d)?;
        if let Err(e) = self.ecrire(&d) {
            *d = toml::from_str(&avant).expect("état précédent relisible");
            return Err(e);
        }
        if comptes_changes {
            self.generation.fetch_add(1, Ordering::Relaxed);
        }
        Ok(r)
    }

    fn ecrire(&self, d: &Fichier) -> Result<(), String> {
        let Some(path) = &self.path else { return Ok(()) };
        let mut text = String::from(
            "# Comptes créés par le site d'inscription (fichier écrit par v34-server).\n\
             # Ne pas modifier pendant que le serveur tourne.\n\n",
        );
        text.push_str(&toml::to_string(d).map_err(|e| e.to_string())?);
        let tmp = path.with_extension("toml.tmp");
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{} : {e}", dir.display()))?;
        }
        write_private(&tmp, text.as_bytes()).map_err(|e| format!("écriture de {} : {e}", tmp.display()))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("écriture de {} : {e}", path.display()))
    }
}

/// Écrit un fichier lisible par son propriétaire et son groupe seulement
/// (le central, qui le lit, tourne avec le même utilisateur ou groupe).
fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o640);
    }
    let mut f = o.open(path)?;
    f.write_all(data)?;
    f.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inscription_validation_acces() {
        let dir = std::env::temp_dir().join(format!("v34-comptes-{}", to_hex(&random_bytes::<6>())));
        let path = dir.join("comptes.toml");
        let c = Comptes::ouvrir(&path).unwrap();
        let g0 = c.generation();
        let jeton = c.inscrire("gaston", "0612345678", "scram-sha256$1000$00$00", "gaston@example.org").unwrap();
        assert_eq!(c.conflit("GASTON", "0700000000", "x@y.z"), Some("nom"));
        assert_eq!(c.conflit("lagaffe", "0612345678", "x@y.z"), Some("numero"));
        assert_eq!(c.conflit("lagaffe", "0700000000", "Gaston@Example.org"), Some("courriel"));
        assert_eq!(c.conflit("lagaffe", "0700000000", "x@y.z"), None);
        assert_eq!(c.generation(), g0, "une inscription en attente ne touche pas l'annuaire");
        assert!(c.valider("mauvais-jeton", |_, _| true).is_err());
        assert_eq!(c.inscription(&jeton).unwrap().nom, "gaston");
        let compte = c.valider(&jeton, |_, _| true).unwrap();
        assert!(!compte.fai && !compte.fai_decide, "pas d'accès Internet avant l'accord");
        assert!(c.generation() > g0);
        assert!(c.valider(&jeton, |_, _| true).is_err(), "jeton à usage unique");
        c.regler_fai("gaston", true).unwrap();

        // Relu depuis le disque : même état, même secret, jeton jamais stocké.
        let texte = std::fs::read_to_string(&path).unwrap();
        assert!(!texte.contains(&jeton));
        let c2 = Comptes::ouvrir(&path).unwrap();
        assert_eq!(c2.secret(), c.secret());
        let g = c2.compte("gaston").unwrap();
        assert!(g.fai && g.fai_decide);
        c2.supprimer("gaston").unwrap();
        assert!(Comptes::ouvrir(&path).unwrap().liste().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn validation_refusee_si_numero_pris_entre_temps() {
        let c = Comptes::en_memoire();
        let j = c.inscrire("gaston", "0612345678", "h", "g@example.org").unwrap();
        assert!(c.valider(&j, |_, num| num != "0612345678").is_err());
        assert!(c.liste().is_empty());
    }
}
