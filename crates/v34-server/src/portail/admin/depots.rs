//! Ce que les abonnés laissent sur les numéros cachés (répondeur de
//! Gérard, messages pour 1998, tableau d'honneur du Prix en Francs…) : les
//! petits fichiers JSON que le central écrit dans son dossier de données
//! ([`crate::caches`]).
//!
//! Le site les lit et les réécrit **tels quels** (`serde_json::Value`) : un
//! champ ajouté par une version plus récente du central est conservé, et un
//! nouveau service qui range ses messages dans un fichier JSON du dossier
//! apparaît tout seul (sous le nom de son fichier). Deux formes sont
//! comprises : une liste d'objets (`de`, `quand`, `texte`…) et une table
//! « abonné → valeur ».
//!
//! Chaque suppression prend le verrou du dossier ([`crate::verrou`]), que le
//! central prend aussi pour écrire ces fichiers, et vérifie l'empreinte de
//! l'entrée visée : si la liste a bougé entre l'affichage et le clic, rien
//! n'est effacé par erreur.

use std::path::Path;

use serde_json::Value;
use sha2::{Digest, Sha256};
use v34_net::auth::to_hex;

/// Fichiers connus : (fichier, service du central qui l'écrit)
const CONNUS: &[(&str, &str)] = &[("repondeur.json", "cache-repondeur"), ("futur.json", "cache-futur"), ("prix.json", "cache-prix")];
/// Fichiers JSON du dossier qui ne sont pas des dépôts d'abonnés
const SYSTEME: &[&str] = &["etat.json", "carnets.json"];
/// Taille maximale d'un fichier lu (ils font quelques Ko)
const TAILLE_MAX: u64 = 4 << 20;

/// Un fichier de dépôts.
#[derive(Clone, Debug)]
pub struct Depot {
    pub fichier: String,
    /// « répondeur de Gérard (01 43 33 12 12) », ou le nom du fichier
    pub libelle: String,
    pub entrees: Vec<Entree>,
}

/// Une entrée d'un dépôt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entree {
    /// Rang dans la liste, ou nom de l'abonné dans une table
    pub cle: String,
    /// Empreinte du contenu (contrôle avant suppression)
    pub empreinte: String,
    /// Auteur (champ `de`, ou la clé d'une table)
    pub de: Option<String>,
    /// Date (champ `quand` ou `date`, secondes Unix)
    pub quand: Option<u64>,
    /// Le message, ou la valeur
    pub texte: String,
}

fn empreinte(v: &Value) -> String {
    to_hex(&Sha256::digest(v.to_string().as_bytes()))[..16].to_string()
}

/// Libellé d'un fichier : le service du central qui l'écrit, avec son
/// numéro.
fn libelle(fichier: &str) -> String {
    CONNUS
        .iter()
        .find(|(f, _)| *f == fichier)
        .and_then(|(_, s)| crate::caches::LISTE.iter().find(|n| n.service == *s))
        .map(|n| format!("{} ({})", n.nom, super::super::pages::numero_lisible(n.numero)))
        .unwrap_or_else(|| fichier.to_string())
}

/// Nom de fichier acceptable : un `.json` du dossier lui-même, ni caché, ni
/// réservé au système.
fn fichier_valide(f: &str) -> bool {
    f.ends_with(".json") && !f.starts_with('.') && !f.contains(['/', '\\']) && !SYSTEME.contains(&f)
}

fn lire_valeur(chemin: &Path) -> Option<Value> {
    use std::io::Read;
    let mut t = Vec::new();
    std::fs::File::open(chemin).ok()?.take(TAILLE_MAX).read_to_end(&mut t).ok()?;
    serde_json::from_slice(&t).ok()
}

/// Les entrées d'une valeur lue (liste ou table) ; `None` pour une autre
/// forme.
fn entrees(v: &Value) -> Option<Vec<Entree>> {
    match v {
        Value::Array(l) => Some(
            l.iter()
                .enumerate()
                .map(|(i, e)| {
                    let champ = |k: &str| e.get(k);
                    let de = champ("de").and_then(Value::as_str).map(str::to_string);
                    let quand = champ("quand").or_else(|| champ("date")).and_then(Value::as_u64);
                    let texte = match champ("texte").and_then(Value::as_str) {
                        Some(t) => t.to_string(),
                        None => match e {
                            Value::Object(o) => {
                                let reste: serde_json::Map<String, Value> = o.iter().filter(|(k, _)| !matches!(k.as_str(), "de" | "quand" | "date")).map(|(k, v)| (k.clone(), v.clone())).collect();
                                Value::Object(reste).to_string()
                            }
                            Value::String(s) => s.clone(),
                            autre => autre.to_string(),
                        },
                    };
                    Entree { cle: i.to_string(), empreinte: empreinte(e), de, quand, texte }
                })
                .collect(),
        ),
        Value::Object(o) => Some(
            o.iter()
                .map(|(k, val)| Entree {
                    cle: k.clone(),
                    empreinte: empreinte(val),
                    de: Some(k.clone()),
                    quand: None,
                    texte: match val {
                        Value::String(s) => s.clone(),
                        autre => autre.to_string(),
                    },
                })
                .collect(),
        ),
        _ => None,
    }
}

/// Les dépôts du dossier de données : d'abord les fichiers connus (même
/// absents : vides), puis les autres fichiers JSON qui ont une forme
/// compréhensible.
pub fn lister(dir: &Path) -> Vec<Depot> {
    let mut v: Vec<Depot> = CONNUS
        .iter()
        .map(|(f, _)| Depot { fichier: f.to_string(), libelle: libelle(f), entrees: lire_valeur(&dir.join(f)).as_ref().and_then(entrees).unwrap_or_default() })
        .collect();
    let mut autres: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|f| fichier_valide(f) && !CONNUS.iter().any(|(c, _)| c == f))
        .collect();
    autres.sort();
    for f in autres {
        if let Some(entrees) = lire_valeur(&dir.join(&f)).as_ref().and_then(entrees) {
            v.push(Depot { libelle: libelle(&f), fichier: f, entrees });
        }
    }
    v
}

/// Réécrit un fichier de dépôts (fichier temporaire propre au site, puis
/// renommage).
fn ecrire(dir: &Path, fichier: &str, v: &Value) -> Result<(), String> {
    let tmp = dir.join(format!(".{fichier}.site.tmp"));
    let r = std::fs::write(&tmp, v.to_string()).and_then(|()| std::fs::rename(&tmp, dir.join(fichier)));
    r.map_err(|e| format!("{fichier} non réécrit : {e}"))
}

/// Supprime une entrée (sa clé et son empreinte, telles qu'affichées).
pub fn supprimer(dir: &Path, fichier: &str, cle: &str, empreinte_vue: &str) -> Result<Entree, String> {
    if !fichier_valide(fichier) {
        return Err("fichier invalide".into());
    }
    let _verrou = crate::verrou::exclusif(dir);
    let mut v = lire_valeur(&dir.join(fichier)).ok_or("fichier introuvable ou illisible")?;
    let entree = entrees(&v).unwrap_or_default().into_iter().find(|e| e.cle == cle).ok_or("message introuvable : la liste a changé, rechargez la page")?;
    if entree.empreinte != empreinte_vue {
        return Err("ce message a changé depuis l'affichage de la page : rechargez-la".into());
    }
    match &mut v {
        Value::Array(l) => {
            l.remove(cle.parse::<usize>().map_err(|_| "rang invalide")?);
        }
        Value::Object(o) => {
            o.remove(cle);
        }
        _ => return Err("forme de fichier inconnue".into()),
    }
    ecrire(dir, fichier, &v)?;
    Ok(entree)
}

/// Supprime tout ce que `nom` a laissé, dans tous les dépôts ; rend le
/// nombre d'entrées effacées.
pub fn supprimer_de(dir: &Path, nom: &str) -> Result<usize, String> {
    let _verrou = crate::verrou::exclusif(dir);
    let mut n = 0;
    for d in lister(dir) {
        let Some(mut v) = lire_valeur(&dir.join(&d.fichier)) else { continue };
        let avant = n;
        match &mut v {
            Value::Array(l) => l.retain(|e| {
                let sien = e.get("de").and_then(Value::as_str) == Some(nom);
                n += usize::from(sien);
                !sien
            }),
            Value::Object(o) => {
                if o.remove(nom).is_some() {
                    n += 1;
                }
            }
            _ => {}
        }
        if n > avant {
            ecrire(dir, &d.fichier, &v)?;
        }
    }
    Ok(n)
}

/// Nombre d'entrées laissées par `nom`.
pub fn compter_de(depots: &[Depot], nom: &str) -> usize {
    depots.iter().flat_map(|d| &d.entrees).filter(|e| e.de.as_deref() == Some(nom)).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn liste_table_et_fichiers_inconnus() {
        let dir = std::env::temp_dir().join(format!("v34-depots-{}", to_hex(&v34_net::crypto::random_bytes::<6>())));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("repondeur.json"), r#"[{"de":"gaston","quand":1794315600,"texte":"Allô Gérard ?"},{"de":"lagaffe","quand":1794315700,"texte":"M'enfin !","nouveau":1}]"#).unwrap();
        std::fs::write(dir.join("prix.json"), r#"{"gaston":1200,"lagaffe":800}"#).unwrap();
        std::fs::write(dir.join("annonces.json"), r#"[{"de":"lagaffe","texte":"Vends gaffophone"}]"#).unwrap();
        std::fs::write(dir.join("etat.json"), r#"{"maj":1}"#).unwrap();
        std::fs::write(dir.join("carnets.json"), r#"{}"#).unwrap();
        let d = lister(&dir);
        let noms: Vec<&str> = d.iter().map(|d| d.fichier.as_str()).collect();
        assert_eq!(noms, ["repondeur.json", "futur.json", "prix.json", "annonces.json"], "fichiers système exclus, inconnus à la fin");
        assert!(d[0].libelle.contains("répondeur") && d[0].libelle.contains("01 43 33 12 12"), "{}", d[0].libelle);
        assert_eq!(d[0].entrees[1].de.as_deref(), Some("lagaffe"));
        assert_eq!(d[0].entrees[1].texte, "M'enfin !");
        assert_eq!(d[2].entrees[0].texte, "1200");
        assert_eq!(compter_de(&d, "lagaffe"), 3);

        // Empreinte périmée : rien n'est effacé
        let e = d[0].entrees[0].clone();
        assert!(supprimer(&dir, "repondeur.json", &e.cle, "0000").is_err());
        assert!(supprimer(&dir, "etat.json", "maj", "x").is_err(), "fichier système");
        assert!(supprimer(&dir, "../comptes.json", "0", "x").is_err());
        assert_eq!(supprimer(&dir, "repondeur.json", &e.cle, &e.empreinte).unwrap().texte, "Allô Gérard ?");
        let t = std::fs::read_to_string(dir.join("repondeur.json")).unwrap();
        assert!(!t.contains("Gérard") && t.contains("\"nouveau\":1"), "le reste est gardé tel quel : {t}");
        // Tout ce que lagaffe a laissé
        assert_eq!(supprimer_de(&dir, "lagaffe").unwrap(), 3);
        assert_eq!(compter_de(&lister(&dir), "lagaffe"), 0);
        assert_eq!(std::fs::read_to_string(dir.join("prix.json")).unwrap(), r#"{"gaston":1200}"#);
        let _ = std::fs::remove_dir_all(dir);
    }
}
