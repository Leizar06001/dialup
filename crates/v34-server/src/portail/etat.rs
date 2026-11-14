//! Page « État du réseau » : le central publie chaque seconde un petit
//! fichier JSON (`V34_ETAT`, par défaut `/var/lib/v34-server/etat.json`),
//! que le site lit à chaque visite :
//!
//! ```json
//! {"version":"0.1.1","depuis":1763100000,"maj":1763100042,"abonnes":42,
//!  "connectes":3,"appels":1,"modems":{"occupes":1,"max":4},"salon":2}
//! ```
//!
//! Fichier absent, illisible ou pas mis à jour depuis plus de
//! [`PERIME_S`] secondes : le central est considéré hors service.

use std::io::Read;
use std::path::Path;

use serde::Deserialize;

use crate::comptes::maintenant;

use super::{pages, Portail, Reponse};

/// Au-delà, l'état publié par le central est périmé (s)
pub const PERIME_S: u64 = 30;
/// Taille maximale lue (le fichier fait une centaine d'octets)
const TAILLE_MAX: u64 = 64 * 1024;

/// État publié par le central (champs inconnus ignorés : le central peut
/// en ajouter).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Etat {
    /// Version du central
    pub version: String,
    /// Démarrage du central (secondes Unix)
    pub depuis: u64,
    /// Écriture du fichier (secondes Unix)
    pub maj: u64,
    /// Abonnés de l'annuaire
    pub abonnes: u64,
    /// Abonnés enregistrés (connectés)
    pub connectes: u64,
    /// Appels en cours
    pub appels: u64,
    pub modems: Modems,
    /// Présents dans le salon du FAI
    pub salon: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Modems {
    /// Appels en cours vers les numéros spéciaux
    pub occupes: u64,
    /// Modems du central (`modems.appels_max`)
    pub max: u64,
}

/// Lit l'état du central ; `None` s'il est absent, illisible ou périmé à
/// l'instant `maintenant` (secondes Unix).
pub fn lire(path: &Path, maintenant: u64) -> Option<Etat> {
    let mut t = Vec::new();
    std::fs::File::open(path).ok()?.take(TAILLE_MAX).read_to_end(&mut t).ok()?;
    let e: Etat = serde_json::from_slice(&t).ok()?;
    let frais = e.maj + PERIME_S >= maintenant && e.maj <= maintenant + PERIME_S;
    frais.then_some(e)
}

impl Portail {
    pub(super) fn etat(&self) -> Reponse {
        let now = maintenant();
        let etat = lire(&self.r.etat, now);
        // Dernière version de l'appli : d'après le cache (sans attendre GitHub).
        let version = self.version_sans_attendre().map(|v| v.tag.clone());
        let mut r = self.page(if etat.is_some() { 200 } else { 503 }, "État du réseau", &pages::etat(&self.r.nom, etat.as_ref(), version.as_deref(), now));
        // Rafraîchissement automatique, sans JavaScript (en-tête de Netscape 1.1).
        r.entetes.push(("Refresh", "30".into()));
        r.entetes.push(("Cache-Control", "no-store".into()));
        r
    }
}
