//! Facturation et succès des abonnés.
//!
//! Dans le dossier de données du central (celui du fichier des comptes,
//! partagé avec le site) :
//! - `tickets/AAAA-MM.jsonl` : un ticket de communication par ligne, rangé
//!   au mois de la fin de l'appel ; seul l'appelant paie ;
//! - `carnets.json` : par abonné, les succès obtenus et de quoi les
//!   constater (abonnés appelés, jours de connexion, temps en ligne).
//!
//! Le central est le seul à écrire ; le site lit (facture, succès,
//! classement).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use v34_net::succes::{self, Source};
use v34_net::tarif::{self, Categorie};

use crate::warn;

/// Un appel abouti.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ticket {
    /// Décroché (secondes Unix)
    pub debut: u64,
    pub duree_s: u32,
    pub appelant: String,
    pub appelant_num: String,
    /// Numéro composé
    pub numero: String,
    /// Nom de l'abonné appelé, ou du service ("fai", "oracle", "echo")
    pub appele: String,
    /// Service du central qui a répondu (`None` : un abonné)
    #[serde(default)]
    pub service: Option<String>,
    /// Catégorie tarifaire ("local", "vert", "kiosque")
    pub categorie: String,
    pub centimes: u32,
}

impl Ticket {
    /// Ticket d'un appel, avec son prix.
    pub fn nouveau(debut: u64, duree_s: u32, appelant: (&str, &str), numero: &str, appele: &str, service: Option<&str>) -> Ticket {
        let cat = Categorie::de_l_appel(numero, service);
        Ticket {
            debut,
            duree_s,
            appelant: appelant.0.into(),
            appelant_num: appelant.1.into(),
            numero: numero.into(),
            appele: appele.into(),
            service: service.map(Into::into),
            categorie: cat.code().into(),
            centimes: tarif::cout_centimes(cat, debut, duree_s),
        }
    }

    pub fn fin(&self) -> u64 {
        self.debut + self.duree_s as u64
    }
}

/// Ce que le central sait d'un abonné pour ses succès.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Carnet {
    /// Succès obtenus : identifiant → date (secondes Unix)
    pub succes: BTreeMap<String, u64>,
    /// Abonnés appelés
    pub appeles: BTreeSet<String>,
    /// Jours de connexion au central (AAAAMMJJ, heure de Paris)
    pub jours: BTreeSet<u32>,
    /// Temps en ligne, appels passés et reçus (s)
    pub secondes: u64,
    /// Temps passé avec l'oracle (s)
    pub oracle_s: u64,
}

impl Carnet {
    fn gagner(&mut self, id: &str, quand: u64) -> bool {
        if self.succes.contains_key(id) {
            return false;
        }
        self.succes.insert(id.to_string(), quand);
        true
    }
}

/// Facture du mois d'un abonné, pour le message COMPTE.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Releve {
    pub mois: u32,
    pub centimes: u32,
    pub secondes: u32,
    pub succes: Vec<String>,
}

pub struct Facturation {
    dir: PathBuf,
    carnets: BTreeMap<String, Carnet>,
    /// Mois en cours (AAAAMM) et totaux des appelants (centimes, secondes)
    mois: u32,
    totaux: HashMap<String, (u32, u32)>,
}

impl Facturation {
    pub fn ouvrir(dir: PathBuf, maintenant: u64) -> Facturation {
        let carnets = lire_carnets(&dir);
        let mut f = Facturation { dir, carnets, mois: 0, totaux: HashMap::new() };
        f.mois_courant(maintenant);
        f
    }

    /// Change de mois si besoin (les totaux repartent de zéro).
    fn mois_courant(&mut self, maintenant: u64) {
        let m = tarif::mois(maintenant);
        if m == self.mois {
            return;
        }
        self.mois = m;
        self.totaux.clear();
        for t in lire_tickets(&self.dir, m) {
            let e = self.totaux.entry(t.appelant).or_default();
            e.0 = e.0.saturating_add(t.centimes);
            e.1 = e.1.saturating_add(t.duree_s);
        }
    }

    /// Note un appel terminé ; rend les abonnés dont le relevé a changé.
    pub fn appel(&mut self, t: Ticket) -> Vec<String> {
        let fin = t.fin();
        self.mois_courant(fin);
        self.ecrire_ticket(&t);
        let e = self.totaux.entry(t.appelant.clone()).or_default();
        e.0 = e.0.saturating_add(t.centimes);
        e.1 = e.1.saturating_add(t.duree_s);
        let facture = e.0;

        let nuit = passe_par_3h(t.debut, fin);
        let c = self.carnets.entry(t.appelant.clone()).or_default();
        c.secondes += t.duree_s as u64;
        c.gagner("premier-appel", fin);
        match t.service.as_deref() {
            None => {
                c.appeles.insert(t.appele.clone());
            }
            Some("fai") => {
                c.gagner("fai", fin);
            }
            Some("oracle") => {
                c.oracle_s += t.duree_s as u64;
                c.gagner("oracle", fin);
            }
            Some("echo") => {
                c.gagner("test", fin);
            }
            // Numéros cachés : le succès porte le nom du service
            Some(s) if s.starts_with("cache-") => {
                c.gagner(s, fin);
                let trouves = c.succes.keys().filter(|k| k.starts_with("cache-")).count();
                if trouves >= succes::EXPLORATEUR {
                    c.gagner("explorateur", fin);
                }
                if trouves >= succes::LISTE.iter().filter(|s| s.id.starts_with("cache-")).count() {
                    c.gagner("phreaker", fin);
                }
            }
            Some(_) => {}
        }
        if c.appeles.len() >= succes::SOCIABLE {
            c.gagner("sociable", fin);
        }
        if c.appeles.len() >= succes::STANDARDISTE {
            c.gagner("standardiste", fin);
        }
        if c.oracle_s >= succes::DISCIPLE_S {
            c.gagner("disciple", fin);
        }
        for (seuil, id) in [(1000, "facture-10"), (10_000, "facture-100"), (100_000, "facture-1000")] {
            if facture >= seuil {
                c.gagner(id, fin);
            }
        }
        communs(c, &t, nuit, fin);
        let mut changes = vec![t.appelant.clone()];
        if t.service.is_none() {
            let c = self.carnets.entry(t.appele.clone()).or_default();
            c.secondes += t.duree_s as u64;
            c.gagner("premier-appel", fin);
            c.gagner("recu", fin);
            communs(c, &t, nuit, fin);
            changes.push(t.appele.clone());
        }
        self.sauver();
        changes
    }

    /// Connexion au central (un jour de plus pour « Abonné fidèle »).
    pub fn connexion(&mut self, nom: &str, maintenant: u64) -> bool {
        let h = tarif::paris(maintenant);
        let jour = h.cle_mois() * 100 + h.jour;
        let c = self.carnets.entry(nom.to_string()).or_default();
        if !c.jours.insert(jour) {
            return false;
        }
        if c.jours.len() >= succes::FIDELE_JOURS {
            c.gagner("fidele", maintenant);
        }
        self.sauver();
        true
    }

    /// Succès signalés par l'appli : seuls ceux qu'elle seule peut constater.
    pub fn exploits(&mut self, nom: &str, ids: &[String], maintenant: u64) -> bool {
        let c = self.carnets.entry(nom.to_string()).or_default();
        let mut change = false;
        for id in ids {
            if succes::trouver(id).is_some_and(|s| s.source == Source::Appli) {
                change |= c.gagner(id, maintenant);
            }
        }
        if change {
            self.sauver();
        }
        change
    }

    /// Succès gagné dans un service du central (numéro caché) : seuls ceux
    /// que le central constate.
    pub fn serveur(&mut self, nom: &str, id: &str, maintenant: u64) -> bool {
        if !succes::trouver(id).is_some_and(|s| s.source == Source::Central) {
            return false;
        }
        let c = self.carnets.entry(nom.to_string()).or_default();
        if !c.gagner(id, maintenant) {
            return false;
        }
        self.sauver();
        true
    }

    /// Relevé du mois en cours.
    pub fn releve(&mut self, nom: &str, maintenant: u64) -> Releve {
        self.mois_courant(maintenant);
        let (centimes, secondes) = self.totaux.get(nom).copied().unwrap_or_default();
        let succes = self.carnets.get(nom).map(|c| c.succes.keys().cloned().collect()).unwrap_or_default();
        Releve { mois: self.mois, centimes, secondes, succes }
    }

    fn ecrire_ticket(&self, t: &Ticket) {
        let p = chemin_tickets(&self.dir, tarif::mois(t.fin()));
        let r = (|| -> std::io::Result<()> {
            if let Some(d) = p.parent() {
                std::fs::create_dir_all(d)?;
            }
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&p)?;
            let mut ligne = serde_json::to_vec(t).map_err(std::io::Error::other)?;
            ligne.push(b'\n');
            f.write_all(&ligne)
        })();
        if let Err(e) = r {
            warn!("ticket non écrit dans {} : {e}", p.display());
        }
    }

    fn sauver(&self) {
        let p = self.dir.join("carnets.json");
        let tmp = self.dir.join("carnets.json.tmp");
        let r = serde_json::to_vec(&self.carnets)
            .map_err(std::io::Error::other)
            .and_then(|b| std::fs::write(&tmp, b))
            .and_then(|()| std::fs::rename(&tmp, &p));
        if let Err(e) = r {
            warn!("carnets non écrits dans {} : {e}", p.display());
        }
    }
}

/// Succès communs à l'appelant et à l'abonné appelé.
fn communs(c: &mut Carnet, t: &Ticket, nuit: bool, fin: u64) {
    if t.duree_s >= succes::MARATHON_S {
        c.gagner("marathon", fin);
    }
    if nuit {
        c.gagner("nuit-blanche", fin);
    }
    if c.secondes >= succes::ACCRO_S {
        c.gagner("accro", fin);
    }
}

/// L'appel était-il en cours à un moment entre 3 h et 4 h (heure de Paris) ?
fn passe_par_3h(debut: u64, fin: u64) -> bool {
    if tarif::paris(debut).heure == 3 {
        return true;
    }
    let mut h = (debut / 3600 + 1) * 3600;
    while h < fin {
        if tarif::paris(h).heure == 3 {
            return true;
        }
        h += 3600;
    }
    false
}

pub fn chemin_tickets(dir: &Path, mois: u32) -> PathBuf {
    dir.join("tickets").join(format!("{}-{:02}.jsonl", mois / 100, mois % 100))
}

/// Tickets d'un mois (lignes illisibles ignorées).
pub fn lire_tickets(dir: &Path, mois: u32) -> Vec<Ticket> {
    let Ok(t) = std::fs::read_to_string(chemin_tickets(dir, mois)) else { return Vec::new() };
    t.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// Mois qui ont des tickets (AAAAMM), du plus récent au plus ancien.
pub fn mois_factures(dir: &Path) -> Vec<u32> {
    let mut v: Vec<u32> = std::fs::read_dir(dir.join("tickets"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().into_string().ok()?;
            let (a, m) = n.strip_suffix(".jsonl")?.split_once('-')?;
            Some(a.parse::<u32>().ok()? * 100 + m.parse::<u32>().ok()?)
        })
        .collect();
    v.sort_unstable_by(|a, b| b.cmp(a));
    v
}

pub fn lire_carnets(dir: &Path) -> BTreeMap<String, Carnet> {
    std::fs::read(dir.join("carnets.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mardi 10 novembre 2026, 14 h à Paris
    const MARDI: u64 = 1_794_315_600;

    fn dossier(nom: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("v34-facturation-{nom}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn tickets_totaux_et_succes() {
        let d = dossier("tickets");
        let mut f = Facturation::ouvrir(d.clone(), MARDI);
        let t = Ticket::nouveau(MARDI, 600, ("raziel", "0100000001"), "0100000002", "gaston", None);
        assert_eq!(t.centimes, 74 + 7 * 28);
        assert_eq!(f.appel(t.clone()), ["raziel", "gaston"]);
        let r = f.releve("raziel", MARDI + 700);
        assert_eq!((r.mois, r.centimes, r.secondes), (202611, 270, 600));
        assert!(r.succes.contains(&"premier-appel".to_string()));
        // l'appelé ne paie pas, mais il a « Ça sonne ! »
        let g = f.releve("gaston", MARDI + 700);
        assert_eq!(g.centimes, 0);
        assert!(g.succes.contains(&"recu".to_string()));
        // 3615 ORACLE pendant une heure : 133,80 F, « Marathon », « Gros consommateur »…
        let o = Ticket::nouveau(MARDI + 1000, 3600, ("raziel", "0100000001"), "0836672253", "oracle", Some("oracle"));
        f.appel(o);
        let r = f.releve("raziel", MARDI + 5000);
        assert_eq!(r.centimes, 270 + 13_380);
        for id in ["oracle", "marathon", "facture-10", "facture-100", "disciple"] {
            assert!(r.succes.contains(&id.to_string()), "{id}");
        }
        // relu depuis les fichiers (redémarrage du central)
        let mut f2 = Facturation::ouvrir(d.clone(), MARDI + 6000);
        assert_eq!(f2.releve("raziel", MARDI + 6000), r);
        assert_eq!(lire_tickets(&d, 202611).len(), 2);
        assert_eq!(mois_factures(&d), [202611]);
        // le mois suivant repart de zéro
        let dec = f2.releve("raziel", MARDI + 30 * 86_400);
        assert_eq!((dec.mois, dec.centimes), (202612, 0));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn exploits_et_fidelite() {
        let d = dossier("exploits");
        let mut f = Facturation::ouvrir(d.clone(), MARDI);
        // l'appli ne peut pas s'attribuer un succès du central
        assert!(f.exploits("raziel", &["v90".into(), "facture-1000".into()], MARDI));
        let s = f.releve("raziel", MARDI).succes;
        assert!(s.contains(&"v90".to_string()) && !s.contains(&"facture-1000".to_string()));
        assert!(!f.exploits("raziel", &["v90".into()], MARDI));
        for j in 0..7 {
            f.connexion("raziel", MARDI + j * 86_400);
        }
        assert!(!f.connexion("raziel", MARDI + 6 * 86_400 + 60));
        assert!(f.releve("raziel", MARDI).succes.contains(&"fidele".to_string()));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn numeros_caches() {
        let d = dossier("caches");
        let mut f = Facturation::ouvrir(d.clone(), MARDI);
        let caches: Vec<&str> = succes::LISTE.iter().map(|s| s.id).filter(|id| id.starts_with("cache-")).collect();
        for (k, id) in caches.iter().enumerate() {
            let t = Ticket::nouveau(MARDI + k as u64 * 100, 30, ("raziel", "0100000001"), "0836993699", "x", Some(id));
            f.appel(t);
            let s = f.releve("raziel", MARDI).succes;
            assert!(s.contains(&id.to_string()));
            assert_eq!(s.contains(&"explorateur".to_string()), k + 1 >= succes::EXPLORATEUR, "après {} numéros", k + 1);
            assert_eq!(s.contains(&"phreaker".to_string()), k + 1 == caches.len());
        }
        // 08 36 : 30 s au kiosque
        assert_eq!(lire_tickets(&d, 202611)[0].centimes, 112);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn nuit_blanche() {
        // 2 h 30 → 3 h 10 à Paris (1 h 30 UTC, hiver)
        let t = 1_794_274_200;
        assert_eq!(tarif::paris(t).heure, 2);
        assert!(passe_par_3h(t, t + 40 * 60));
        assert!(!passe_par_3h(t, t + 20 * 60));
    }
}
