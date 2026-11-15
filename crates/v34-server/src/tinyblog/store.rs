//! Rangement des Tinyblogs, dans le dossier de données du central :
//! `tinyblog/<abonné>.json` (le blog, ses articles et leurs commentaires),
//! `tinyblog/img/<abonné>-<n>.gif` (les images, déjà réduites) et
//! `tinyblog/coms.json` (combien de commentaires chacun a laissés).
//!
//! Le central (les abonnés, par le modem) et le site (la modération, dans
//! `/admin`) écrivent ces fichiers : chaque modification prend le verrou
//! du dossier ([`crate::verrou`]), commun aux deux processus.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::warn;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Blog {
    pub titre: String,
    pub description: String,
    pub theme: String,
    pub cree: u64,
    pub articles: Vec<Article>,
    /// Prochain numéro d'article
    pub suivant: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Article {
    pub id: u32,
    pub date: u64,
    pub titre: String,
    pub texte: String,
    /// Nom du fichier image (dans `img/`)
    pub image: Option<String>,
    /// Abonnés qui kiffent
    pub kiffs: Vec<String>,
    pub coms: Vec<Com>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Com {
    pub de: String,
    pub date: u64,
    pub texte: String,
}

impl Blog {
    pub fn kiffs(&self) -> usize {
        self.articles.iter().map(|a| a.kiffs.len()).sum()
    }
    pub fn dernier(&self) -> u64 {
        self.articles.iter().map(|a| a.date).max().unwrap_or(self.cree)
    }
}

pub struct Store {
    dir: PathBuf,
    verrou: Mutex<()>,
}

/// Nom d'abonné utilisable comme nom de fichier (ceux du central le sont).
pub fn nom_valide(nom: &str) -> bool {
    v34_net::auth::valid_name(nom) && !nom.contains(['/', '\\', '.'])
}

impl Store {
    pub fn new(dir: PathBuf) -> Store {
        Store { dir, verrou: Mutex::new(()) }
    }

    fn chemin(&self, nom: &str) -> PathBuf {
        self.dir.join(format!("{nom}.json"))
    }

    pub fn dossier_images(&self) -> PathBuf {
        self.dir.join("img")
    }

    fn charger(&self, nom: &str) -> Option<Blog> {
        if !nom_valide(nom) {
            return None;
        }
        let t = std::fs::read(self.chemin(nom)).ok()?;
        serde_json::from_slice(&t).ok()
    }

    pub fn lire(&self, nom: &str) -> Option<Blog> {
        let _v = self.verrou.lock().unwrap_or_else(|e| e.into_inner());
        self.charger(nom)
    }

    /// Lit (ou crée), modifie et réécrit le blog de `nom`.
    pub fn modifier<R>(&self, nom: &str, f: impl FnOnce(&mut Blog) -> R) -> Option<R> {
        if !nom_valide(nom) {
            return None;
        }
        let _v = self.verrou.lock().unwrap_or_else(|e| e.into_inner());
        let _f = crate::verrou::exclusif(&self.dir);
        let mut b = self.charger(nom).unwrap_or_default();
        let r = f(&mut b);
        ecrire(&self.chemin(nom), &b);
        Some(r)
    }

    /// Comme [`Store::modifier`], seulement si le blog existe déjà (la
    /// modération ne crée pas de blog).
    pub fn modifier_existant<R>(&self, nom: &str, f: impl FnOnce(&mut Blog) -> R) -> Option<R> {
        if !nom_valide(nom) {
            return None;
        }
        let _v = self.verrou.lock().unwrap_or_else(|e| e.into_inner());
        let _f = crate::verrou::exclusif(&self.dir);
        let mut b = self.charger(nom)?;
        let r = f(&mut b);
        ecrire(&self.chemin(nom), &b);
        Some(r)
    }

    /// Supprime le blog de `nom` et ses images ; rend le blog supprimé.
    pub fn supprimer_blog(&self, nom: &str) -> Option<Blog> {
        if !nom_valide(nom) {
            return None;
        }
        let _v = self.verrou.lock().unwrap_or_else(|e| e.into_inner());
        let _f = crate::verrou::exclusif(&self.dir);
        let b = self.charger(nom)?;
        if let Err(e) = std::fs::remove_file(self.chemin(nom)) {
            warn!("tinyblog : {} non supprimé : {e}", self.chemin(nom).display());
            return None;
        }
        // Les images de ses articles, et celles qui traîneraient encore
        // (`<nom>-<n>.gif`)
        for f in b.articles.iter().filter_map(|a| a.image.as_deref()) {
            self.supprimer_image(f);
        }
        let prefixe = format!("{nom}-");
        if let Ok(rd) = std::fs::read_dir(self.dossier_images()) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                let a_lui = n.strip_prefix(&prefixe).and_then(|r| r.strip_suffix(".gif")).is_some_and(|id| !id.is_empty() && id.bytes().all(|c| c.is_ascii_digit()));
                if a_lui {
                    self.supprimer_image(&n);
                }
            }
        }
        Some(b)
    }

    /// Tous les blogs (nom, blog).
    pub fn tous(&self) -> Vec<(String, Blog)> {
        let _v = self.verrou.lock().unwrap_or_else(|e| e.into_inner());
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        let mut v: Vec<(String, Blog)> = rd
            .flatten()
            .filter_map(|e| {
                let n = e.file_name().into_string().ok()?;
                let nom = n.strip_suffix(".json")?;
                if nom == "coms" {
                    return None;
                }
                Some((nom.to_string(), self.charger(nom)?))
            })
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    /// Un commentaire de plus pour `nom` ; rend son total.
    pub fn compter_com(&self, nom: &str) -> u32 {
        let _v = self.verrou.lock().unwrap_or_else(|e| e.into_inner());
        let _f = crate::verrou::exclusif(&self.dir);
        let p = self.dir.join("coms.json");
        let mut m: BTreeMap<String, u32> = std::fs::read(&p).ok().and_then(|t| serde_json::from_slice(&t).ok()).unwrap_or_default();
        let n = m.entry(nom.to_string()).or_default();
        *n += 1;
        let total = *n;
        ecrire(&p, &m);
        total
    }

    pub fn enregistrer_image(&self, fichier: &str, donnees: &[u8]) -> bool {
        let d = self.dossier_images();
        let r = std::fs::create_dir_all(&d).and_then(|()| std::fs::write(d.join(fichier), donnees));
        if let Err(e) = &r {
            warn!("tinyblog : image {fichier} non écrite : {e}");
        }
        r.is_ok()
    }

    pub fn lire_image(&self, fichier: &str) -> Option<Vec<u8>> {
        if fichier.contains(['/', '\\']) || fichier.starts_with('.') {
            return None;
        }
        std::fs::read(self.dossier_images().join(fichier)).ok()
    }

    pub fn supprimer_image(&self, fichier: &str) {
        if !fichier.contains(['/', '\\']) {
            let _ = std::fs::remove_file(self.dossier_images().join(fichier));
        }
    }
}

fn ecrire<T: Serialize>(p: &Path, v: &T) {
    let r = (|| -> std::io::Result<()> {
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(v).map_err(std::io::Error::other)?)?;
        std::fs::rename(&tmp, p)
    })();
    if let Err(e) = r {
        warn!("tinyblog : {} non écrit : {e}", p.display());
    }
}
