//! Réglages du site modifiables depuis `/admin/site` : inscriptions
//! ouvertes ou fermées, et nouvelles ajoutées à la rubrique « Quoi de
//! neuf ? » de l'accueil (en plus de celles écrites dans le code).
//!
//! Fichier `portail.toml` du dossier de données (à côté des comptes), écrit
//! par le site seul, de façon atomique et lisible par le seul serveur. Il
//! ne contient aucune donnée personnelle.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use v34_net::auth::to_hex;
use v34_net::crypto::random_bytes;

use crate::comptes::ecrire_atomique;
use crate::warn;

/// Nom du fichier, dans le dossier de données
pub const FICHIER: &str = "portail.toml";
/// Nouvelles ajoutées gardées au plus
pub const NOUVELLES_MAX: usize = 20;
/// Longueur maximale du titre d'une nouvelle (caractères)
pub const TITRE_MAX: usize = 200;

/// Une nouvelle ajoutée depuis l'administration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NouvelleAjoutee {
    /// Identifiant aléatoire (hex)
    pub id: String,
    /// AAAA-MM-JJ
    pub date: String,
    pub titre: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReglagesSite {
    /// Inscriptions fermées par l'administrateur (même si les e-mails
    /// marchent)
    pub inscriptions_fermees: bool,
    /// Nouvelles ajoutées, de la plus récente à la plus ancienne
    pub nouvelle: Vec<NouvelleAjoutee>,
}

pub struct Site {
    path: Option<PathBuf>,
    data: Mutex<ReglagesSite>,
}

impl std::fmt::Debug for Site {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Site").field("path", &self.path).finish_non_exhaustive()
    }
}

/// « AAAA-MM-JJ » plausible ?
pub fn date_valide(d: &str) -> bool {
    let b = d.as_bytes();
    let n = |r: std::ops::Range<usize>| d.get(r).and_then(|s| s.parse::<u32>().ok());
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
        && n(0..4).is_some_and(|a| (1990..=2100).contains(&a))
        && n(5..7).is_some_and(|m| (1..=12).contains(&m))
        && n(8..10).is_some_and(|j| (1..=31).contains(&j))
}

impl Site {
    /// Lit les réglages (fichier absent : valeurs par défaut ; illisible :
    /// valeurs par défaut aussi, avec un avertissement, pour que le site
    /// démarre quand même).
    pub fn ouvrir(path: &Path) -> Site {
        let data = match std::fs::read_to_string(path) {
            Ok(t) => toml::from_str(&t).unwrap_or_else(|e| {
                warn!("{} illisible ({e}) : réglages du site par défaut", path.display());
                ReglagesSite::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => ReglagesSite::default(),
            Err(e) => {
                warn!("lecture de {} : {e} ; réglages du site par défaut", path.display());
                ReglagesSite::default()
            }
        };
        Site { path: Some(path.to_path_buf()), data: Mutex::new(data) }
    }

    /// Réglages sans fichier (site sans dossier de données).
    pub fn en_memoire() -> Site {
        Site { path: None, data: Mutex::new(ReglagesSite::default()) }
    }

    pub fn lire(&self) -> ReglagesSite {
        self.data.lock().unwrap().clone()
    }

    pub fn fermer_inscriptions(&self, fermees: bool) -> Result<(), String> {
        self.modifier(|r| {
            r.inscriptions_fermees = fermees;
            Ok(())
        })
    }

    /// Ajoute une nouvelle (date AAAA-MM-JJ, titre d'une ligne).
    pub fn ajouter_nouvelle(&self, date: &str, titre: &str) -> Result<NouvelleAjoutee, String> {
        let titre: String = titre.chars().filter(|c| !c.is_control()).collect::<String>().trim().to_string();
        if !date_valide(date) {
            return Err("Date invalide : AAAA-MM-JJ, par exemple 2026-11-15.".into());
        }
        if titre.is_empty() || titre.chars().count() > TITRE_MAX {
            return Err(format!("Nouvelle : 1 à {TITRE_MAX} caractères."));
        }
        let n = NouvelleAjoutee { id: to_hex(&random_bytes::<6>()), date: date.into(), titre };
        self.modifier(|r| {
            if r.nouvelle.len() >= NOUVELLES_MAX {
                return Err(format!("{NOUVELLES_MAX} nouvelles au plus : supprimez-en une ancienne d'abord."));
            }
            r.nouvelle.push(n.clone());
            r.nouvelle.sort_by(|a, b| b.date.cmp(&a.date));
            Ok(n)
        })
    }

    pub fn supprimer_nouvelle(&self, id: &str) -> Result<NouvelleAjoutee, String> {
        self.modifier(|r| {
            let i = r.nouvelle.iter().position(|n| n.id == id).ok_or("nouvelle introuvable")?;
            Ok(r.nouvelle.remove(i))
        })
    }

    /// Applique une modification puis réécrit le fichier ; en cas d'échec
    /// d'écriture, la modification est annulée.
    fn modifier<R>(&self, f: impl FnOnce(&mut ReglagesSite) -> Result<R, String>) -> Result<R, String> {
        let mut d = self.data.lock().unwrap();
        let avant = d.clone();
        let r = f(&mut d)?;
        if let Some(path) = &self.path {
            let mut text = String::from("# Réglages du site modifiés depuis /admin (fichier écrit par v34-portail).\n\n");
            let ecrit = toml::to_string(&*d).map_err(|e| e.to_string()).and_then(|t| {
                text.push_str(&t);
                ecrire_atomique(path, text.as_bytes())
            });
            if let Err(e) = ecrit {
                *d = avant;
                return Err(e);
            }
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nouvelles_et_inscriptions_relues_depuis_le_disque() {
        let dir = std::env::temp_dir().join(format!("v34-site-{}", to_hex(&random_bytes::<6>())));
        let path = dir.join(FICHIER);
        let s = Site::ouvrir(&path);
        assert_eq!(s.lire(), ReglagesSite::default());
        assert!(s.ajouter_nouvelle("2026-13-01", "x").is_err());
        assert!(s.ajouter_nouvelle("15/11/2026", "x").is_err());
        assert!(s.ajouter_nouvelle("2026-11-15", "  \u{7} ").is_err());
        assert!(s.ajouter_nouvelle("2026-11-15", &"é".repeat(201)).is_err());
        s.ajouter_nouvelle("2026-11-10", "Ancienne").unwrap();
        let n = s.ajouter_nouvelle("2026-11-15", " Le Morse arrive\n ").unwrap();
        assert_eq!(n.titre, "Le Morse arrive");
        s.fermer_inscriptions(true).unwrap();
        let relu = Site::ouvrir(&path).lire();
        assert!(relu.inscriptions_fermees);
        assert_eq!(relu.nouvelle.iter().map(|n| n.titre.as_str()).collect::<Vec<_>>(), ["Le Morse arrive", "Ancienne"], "la plus récente d'abord");
        s.supprimer_nouvelle(&n.id).unwrap();
        assert!(s.supprimer_nouvelle(&n.id).is_err());
        assert_eq!(Site::ouvrir(&path).lire().nouvelle.len(), 1);
        // Fichier abîmé : le site démarre quand même
        std::fs::write(&path, "pas = [du toml").unwrap();
        assert_eq!(Site::ouvrir(&path).lire(), ReglagesSite::default());
        let _ = std::fs::remove_dir_all(dir);
    }
}
