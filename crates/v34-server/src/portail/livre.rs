//! Livre d'or du site : les visiteurs laissent un petit mot, publié après
//! relecture par l'administrateur (section de la page /admin).
//!
//! Les messages vivent dans un fichier TOML (`V34_LIVRE`, par défaut
//! `/var/lib/v34-server/livre-dor.toml`), réécrit de façon atomique et
//! lisible par le seul serveur (0640), comme le fichier des comptes.
//!
//! Au plus [`LIVRE_MAX`] messages : [`ATTENTE_MAX`] en attente de
//! relecture (au-delà, le livre est « plein » jusqu'à la modération), et
//! les plus anciens messages publiés s'effacent pour laisser la place aux
//! nouveaux. Seule l'administration fait donc disparaître un message publié.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use v34_net::auth::to_hex;
use v34_net::crypto::random_bytes;

use crate::comptes::{ecrire_atomique, maintenant};
use crate::info;

use super::{pages, Portail, Reponse, Requete};

/// Messages gardés au plus (publiés + en attente)
pub const LIVRE_MAX: usize = 500;
/// Messages en attente de relecture au plus
pub const ATTENTE_MAX: usize = 50;
/// Longueur maximale d'un pseudo (caractères)
pub const PSEUDO_MAX: usize = 32;
/// Longueur maximale d'un message (caractères)
pub const TEXTE_MAX: usize = 500;
/// Messages par adresse IP et par heure
const PAR_HEURE: usize = 3;
/// Messages affichés par page
const PAR_PAGE: usize = 20;

/// Un message du livre d'or.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Message {
    /// Identifiant aléatoire (hex)
    pub id: String,
    pub pseudo: String,
    pub texte: String,
    /// Dépôt (secondes Unix)
    pub date: u64,
    /// Relu et publié par l'administrateur
    #[serde(default)]
    pub publie: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fichier {
    #[serde(default)]
    message: Vec<Message>,
}

pub struct Livre {
    path: Option<PathBuf>,
    data: Mutex<Fichier>,
}

impl std::fmt::Debug for Livre {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Livre").field("path", &self.path).finish_non_exhaustive()
    }
}

/// Nettoie une saisie : espaces de début et de fin retirés, caractères de
/// contrôle supprimés (sauf les retours à la ligne si `lignes`), fins de
/// ligne normalisées.
fn nettoyer(s: &str, lignes: bool) -> String {
    let s = s.replace("\r\n", "\n");
    let t: String = s.chars().filter(|&c| !c.is_control() || (lignes && c == '\n')).collect();
    t.trim().to_string()
}

impl Livre {
    /// Lit le livre d'or (fichier absent : livre vide, créé au premier
    /// message).
    pub fn ouvrir(path: &Path) -> Result<Livre, String> {
        let f: Fichier = match std::fs::read_to_string(path) {
            Ok(t) => toml::from_str(&t).map_err(|e| format!("{} : {e}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Fichier::default(),
            Err(e) => return Err(format!("lecture de {} : {e}", path.display())),
        };
        Ok(Livre { path: Some(path.to_path_buf()), data: Mutex::new(f) })
    }

    /// Livre sans fichier (tests).
    pub fn en_memoire() -> Livre {
        Livre { path: None, data: Mutex::new(Fichier::default()) }
    }

    /// Messages publiés, du plus récent au plus ancien (à la même seconde,
    /// le dernier arrivé d'abord).
    pub fn publies(&self) -> Vec<Message> {
        let mut v: Vec<Message> = self.data.lock().unwrap().message.iter().rev().filter(|m| m.publie).cloned().collect();
        v.sort_by(|a, b| b.date.cmp(&a.date));
        v
    }

    /// Messages en attente de relecture, du plus ancien au plus récent.
    pub fn en_attente(&self) -> Vec<Message> {
        let mut v: Vec<Message> = self.data.lock().unwrap().message.iter().filter(|m| !m.publie).cloned().collect();
        v.sort_by_key(|m| m.date);
        v
    }

    /// Vérifie et nettoie un message ; renvoie (pseudo, texte) ou les
    /// erreurs.
    pub fn verifier(pseudo: &str, texte: &str) -> Result<(String, String), Vec<String>> {
        let (p, t) = (nettoyer(pseudo, false), nettoyer(texte, true));
        let mut e = Vec::new();
        if p.is_empty() || p.chars().count() > PSEUDO_MAX {
            e.push(format!("Pseudo : 1 à {PSEUDO_MAX} caractères."));
        }
        if t.is_empty() || t.chars().count() > TEXTE_MAX {
            e.push(format!("Message : 1 à {TEXTE_MAX} caractères (on n'est pas sur un forum !)."));
        }
        if e.is_empty() {
            Ok((p, t))
        } else {
            Err(e)
        }
    }

    /// Ajoute un message en attente de relecture.
    pub fn ajouter(&self, pseudo: &str, texte: &str) -> Result<Message, String> {
        let (pseudo, texte) = Self::verifier(pseudo, texte).map_err(|e| e.join(" "))?;
        let m = Message { id: to_hex(&random_bytes::<6>()), pseudo, texte, date: maintenant(), publie: false };
        self.modifier(|d| {
            if d.message.iter().filter(|m| !m.publie).count() >= ATTENTE_MAX {
                return Err("Le livre d'or déborde de messages à relire : revenez un peu plus tard !".to_string());
            }
            d.message.push(m.clone());
            Ok(m)
        })
    }

    /// Publie un message en attente (les plus vieux messages publiés
    /// s'effacent au-delà de la place disponible).
    pub fn publier(&self, id: &str) -> Result<Message, String> {
        self.modifier(|d| {
            let m = d.message.iter_mut().find(|m| m.id == id).ok_or("message introuvable")?;
            m.publie = true;
            let m = m.clone();
            let place = LIVRE_MAX - ATTENTE_MAX;
            let mut publies: Vec<(u64, String)> = d.message.iter().filter(|m| m.publie).map(|m| (m.date, m.id.clone())).collect();
            if publies.len() > place {
                publies.sort();
                let trop: Vec<String> = publies[..publies.len() - place].iter().map(|(_, id)| id.clone()).collect();
                d.message.retain(|m| !trop.contains(&m.id));
            }
            Ok(m)
        })
    }

    pub fn supprimer(&self, id: &str) -> Result<Message, String> {
        self.modifier(|d| {
            let i = d.message.iter().position(|m| m.id == id).ok_or("message introuvable")?;
            Ok(d.message.remove(i))
        })
    }

    /// Applique une modification puis réécrit le fichier ; en cas d'échec
    /// d'écriture, la modification est annulée.
    fn modifier<R>(&self, f: impl FnOnce(&mut Fichier) -> Result<R, String>) -> Result<R, String> {
        let mut d = self.data.lock().unwrap();
        let avant = d.message.clone();
        let r = f(&mut d)?;
        if let Some(path) = &self.path {
            let mut text = String::from("# Livre d'or du site d'inscription (fichier écrit par v34-portail).\n\n");
            let ecrit = toml::to_string(&*d).map_err(|e| e.to_string()).and_then(|t| {
                text.push_str(&t);
                ecrire_atomique(path, text.as_bytes())
            });
            if let Err(e) = ecrit {
                d.message = avant;
                return Err(e);
            }
        }
        Ok(r)
    }
}

impl Portail {
    /// Page du livre d'or (page `p` des messages publiés).
    pub(super) fn livre_page(&self, q: &Requete) -> Reponse {
        let page = q.param("p").parse::<usize>().unwrap_or(1).max(1);
        self.livre_formulaire(page, "", "", &[], 200)
    }

    fn livre_formulaire(&self, page: usize, pseudo: &str, texte: &str, erreurs: &[String], statut: u16) -> Reponse {
        let publies = self.livre.publies();
        let pages_max = publies.len().div_ceil(PAR_PAGE).max(1);
        let page = page.min(pages_max);
        let extrait = &publies[(page - 1) * PAR_PAGE..(page * PAR_PAGE).min(publies.len())];
        let f = pages::FormulaireLivre { pseudo, texte, erreurs, jeton: &self.jeton_temps("livre") };
        self.page(statut, "Livre d'or", &pages::livre(&f, extrait, publies.len(), page, pages_max))
    }

    /// Signature du livre d'or.
    pub(super) fn livre_signer(&self, q: &Requete) -> Reponse {
        let (pseudo, texte) = (q.champ("pseudo"), q.champ("message"));
        if let Err(e) = self.controle_robot(q, "livre") {
            return self.livre_formulaire(1, &pseudo, &texte, &[e], 400);
        }
        if let Err(e) = Livre::verifier(&pseudo, &texte) {
            return self.livre_formulaire(1, &pseudo, &texte, &e, 400);
        }
        if !self.autoriser(&format!("livre:{}", q.ip), PAR_HEURE, 3600) {
            return self.livre_formulaire(1, &pseudo, &texte, &["Vous avez déjà beaucoup écrit cette heure-ci : laissez refroidir le clavier !".into()], 429);
        }
        match self.livre.ajouter(&pseudo, &texte) {
            Ok(m) => {
                info!("livre d'or : message de « {} » en attente de relecture ({} car.) depuis {}", m.pseudo, m.texte.chars().count(), q.ip);
                self.page(200, "Livre d'or", &pages::livre_merci(&m.pseudo))
            }
            Err(e) => self.livre_formulaire(1, &pseudo, &texte, &[e], 503),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moderation_et_place_limitee() {
        let dir = std::env::temp_dir().join(format!("v34-livre-{}", to_hex(&random_bytes::<6>())));
        let path = dir.join("livre-dor.toml");
        let l = Livre::ouvrir(&path).unwrap();
        assert!(l.publies().is_empty() && !path.exists(), "pas de fichier avant le premier message");
        assert!(l.ajouter("  ", "texte").is_err());
        assert!(l.ajouter(&"x".repeat(33), "texte").is_err());
        assert!(l.ajouter("gaston", &"é".repeat(501)).is_err());
        let m = l.ajouter(" gaston\u{7} ", "Super site !\r\nVive le 56k.\u{0}").unwrap();
        assert_eq!((m.pseudo.as_str(), m.texte.as_str()), ("gaston", "Super site !\nVive le 56k."));
        assert!(l.publies().is_empty() && l.en_attente().len() == 1, "en attente de relecture");
        l.publier(&m.id).unwrap();
        assert_eq!(l.publies().len(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o640);
        }
        // Relu depuis le disque.
        let l2 = Livre::ouvrir(&path).unwrap();
        assert_eq!(l2.publies(), l.publies());
        assert!(l2.supprimer(&m.id).is_ok() && l2.publies().is_empty());
        assert!(l2.supprimer(&m.id).is_err());

        // Liste d'attente pleine, puis place limitée des messages publiés.
        let l = Livre::en_memoire();
        for i in 0..ATTENTE_MAX {
            l.ajouter("robot", &format!("message {i}")).unwrap();
        }
        assert!(l.ajouter("robot", "un de trop").is_err());
        for m in l.en_attente() {
            l.publier(&m.id).unwrap();
        }
        {
            let mut d = l.data.lock().unwrap();
            for (i, m) in d.message.iter_mut().enumerate() {
                m.date = i as u64;
            }
            for i in 0..(LIVRE_MAX - ATTENTE_MAX - 1) {
                d.message.push(Message { id: format!("vieux{i}"), pseudo: "x".into(), texte: "y".into(), date: 1000 + i as u64, publie: true });
            }
        }
        assert_eq!(l.publies().len(), LIVRE_MAX - 1);
        let nouveau = l.ajouter("gaston", "le dernier").unwrap();
        l.publier(&nouveau.id).unwrap();
        let p = l.publies();
        assert_eq!(p.len(), LIVRE_MAX - ATTENTE_MAX);
        assert_eq!(p[0].id, nouveau.id, "le plus récent en premier");
        assert!(!p.iter().any(|m| m.texte == "message 0"), "le plus ancien s'efface");
        let _ = std::fs::remove_dir_all(dir);
    }
}
