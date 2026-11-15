//! Numéros cachés : des services du central qui ne figurent ni dans
//! l'annuaire ni dans la configuration. On les trouve grâce aux indices semés
//! un peu partout (l'oracle, l'horloge, la météo, le ministère…), et chacun
//! rapporte un succès (le nom du service, voir `v34_net::succes`).
//!
//! Comme 3615 ORACLE, ils parlent en trames de chat, à la cadence d'un
//! Minitel ([`crate::ecran`]) ; ce sont de petits scripts, sans IA.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::ecran::{Ecran, Entree};
use crate::{info, warn};

mod an2000;
mod futur;
mod horloge;
mod meteo;
mod ministere;
mod prix;
mod repondeur;
#[cfg(test)]
mod tests;

/// Succès gagnés pendant un appel (abonné, succès) : le central les relève
/// et les note dans les carnets.
pub type Exploits = Arc<Mutex<Vec<(String, &'static str)>>>;

/// Un numéro caché.
pub struct NumeroCache {
    pub numero: &'static str,
    /// Nom du service, qui est aussi l'identifiant de son succès
    pub service: &'static str,
    pub nom: &'static str,
    lancer: fn(&mut Ligne) -> Result<(), Fin>,
}

pub const LISTE: &[NumeroCache] = &[
    NumeroCache { numero: "0836993699", service: "cache-horloge", nom: "horloge parlante", lancer: horloge::lancer },
    NumeroCache { numero: "0836686868", service: "cache-meteo", nom: "météo des lignes", lancer: meteo::lancer },
    NumeroCache { numero: "0800200000", service: "cache-an2000", nom: "commission de l'an 2000", lancer: an2000::lancer },
    NumeroCache { numero: "0145781998", service: "cache-ministere", nom: "ministère des Postes", lancer: ministere::lancer },
    NumeroCache { numero: "0143331212", service: "cache-repondeur", nom: "répondeur de Gérard", lancer: repondeur::lancer },
    NumeroCache { numero: "0836151998", service: "cache-prix", nom: "le Prix en Francs", lancer: prix::lancer },
    // révélé par le dossier rouge du ministère
    NumeroCache { numero: "0836202626", service: "cache-futur", nom: "la ligne de 2026", lancer: futur::lancer },
];

pub fn index(numero: &str) -> Option<u8> {
    LISTE.iter().position(|n| n.numero == numero).map(|i| i as u8)
}

/// Cadence d'affichage (bit/s) : celle d'un Minitel
const DEBIT: u32 = 1200;
/// Sans rien taper pendant ce temps, le service raccroche
const INACTIVITE: Duration = Duration::from_secs(180);
/// Longueur maximale d'une ligne tapée
const LIGNE_MAX: usize = 240;

/// Pourquoi le service s'arrête.
#[derive(Debug, PartialEq, Eq)]
pub enum Fin {
    /// L'appelant a raccroché
    Raccroche,
    /// Il a tapé /fin
    Demande,
    /// Il ne dit plus rien depuis trop longtemps
    Inactivite,
}

/// La ligne d'un appel : on écrit à l'écran, on lit ce que l'appelant tape.
pub struct Ligne<'a> {
    ecran: Ecran,
    entree: Entree,
    rx: &'a Receiver<Vec<u8>>,
    pub abonne: &'a str,
    pub donnees: Option<&'a Path>,
    exploits: Option<&'a Exploits>,
    dernier: Instant,
    /// L'appelant a raccroché : on finit de lire ce qu'il avait tapé
    parti: bool,
}

impl<'a> Ligne<'a> {
    pub fn new(tx: SyncSender<Vec<u8>>, rx: &'a Receiver<Vec<u8>>, abonne: &'a str, donnees: Option<&'a Path>, debit: u32) -> Ligne<'a> {
        Ligne { ecran: Ecran::new(tx, debit), entree: Entree::default(), rx, abonne, donnees, exploits: None, dernier: Instant::now(), parti: false }
    }

    /// Un succès gagné pendant l'appel (noté par le central).
    pub fn succes(&self, id: &'static str) {
        if let Some(e) = self.exploits {
            e.lock().unwrap_or_else(|p| p.into_inner()).push((self.abonne.to_string(), id));
        }
    }

    /// Nouvelles lignes à l'écran.
    pub fn dire(&mut self, texte: &str) -> Result<(), Fin> {
        self.ecran.lignes(texte).map_err(|_| Fin::Raccroche)
    }

    /// Suite de la ligne en cours.
    pub fn suite(&mut self, texte: &str) -> Result<(), Fin> {
        self.ecran.suite(texte).map_err(|_| Fin::Raccroche)
    }

    /// Une ligne tapée, en attendant au plus `attente` (`None` : rien).
    /// « /fin » raccroche ; trop longtemps sans rien taper aussi.
    pub fn lire(&mut self, attente: Duration) -> Result<Option<String>, Fin> {
        let fin = Instant::now() + attente;
        loop {
            while let Some(brute) = self.entree.file.pop_front() {
                let l = assainir(&brute);
                if l.is_empty() {
                    continue;
                }
                self.dernier = Instant::now();
                if matches!(l.to_lowercase().as_str(), "/fin" | "fin" | "/quitter" | "connexion/fin") {
                    return Err(Fin::Demande);
                }
                return Ok(Some(l));
            }
            if std::mem::take(&mut self.entree.fichier) {
                self.dire("(Ce service ne prend pas les fichiers.)")?;
            }
            if self.parti {
                return Err(Fin::Raccroche);
            }
            if self.dernier.elapsed() > INACTIVITE {
                return Err(Fin::Inactivite);
            }
            let now = Instant::now();
            if now >= fin {
                return Ok(None);
            }
            match self.rx.recv_timeout((fin - now).min(Duration::from_millis(100))) {
                Ok(b) => self.entree.pousser(&b),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => self.parti = true,
            }
            if !self.entree.relever(self.rx) {
                self.parti = true;
            }
        }
    }

    /// Attend une ligne (jusqu'à l'inactivité).
    pub fn attendre(&mut self) -> Result<String, Fin> {
        loop {
            if let Some(l) = self.lire(Duration::from_secs(3600))? {
                return Ok(l);
            }
        }
    }

    /// Laisse passer `d` sans perdre ce que l'appelant tape entre-temps.
    pub fn pause(&mut self, d: Duration) -> Result<(), Fin> {
        let fin = Instant::now() + d;
        loop {
            let now = Instant::now();
            if now >= fin {
                return Ok(());
            }
            if self.parti {
                return Err(Fin::Raccroche);
            }
            match self.rx.recv_timeout((fin - now).min(Duration::from_millis(100))) {
                Ok(b) => self.entree.pousser(&b),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => self.parti = true,
            }
        }
    }
}

/// Une ligne tapée : sans caractères de contrôle, bornée.
fn assainir(s: &str) -> String {
    let t: String = s.chars().filter(|c| !c.is_control()).take(LIGNE_MAX).collect();
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Un appel à un numéro caché, du CONNECT au raccroché.
pub fn session(i: u8, label: &str, abonne: &str, donnees: Option<&Path>, exploits: &Exploits, from_modem: Receiver<Vec<u8>>, to_modem: SyncSender<Vec<u8>>) -> String {
    let n = &LISTE[i as usize];
    let debut = Instant::now();
    let mut l = Ligne::new(to_modem, &from_modem, abonne, donnees, DEBIT);
    l.exploits = Some(exploits);
    let fin = match (n.lancer)(&mut l) {
        Ok(()) => "raccroché par le service",
        Err(Fin::Raccroche) => "raccroché par l'appelant",
        Err(Fin::Demande) => {
            let _ = l.dire("Au revoir ! *clic*");
            "/fin"
        }
        Err(Fin::Inactivite) => {
            let _ = l.dire("Plus personne au bout du fil ? Le service raccroche. *clic*");
            "inactivité"
        }
    };
    info!("{label} : numéro caché « {} », {} s, {fin}", n.nom, debut.elapsed().as_secs());
    fin.into()
}

// ───────────────────────── outils ─────────────────────────

pub fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Un nombre au hasard dans 0..n.
pub fn hasard(n: usize) -> usize {
    (u32::from_le_bytes(v34_net::crypto::random_bytes()) as usize) % n.max(1)
}

pub fn choix<'t>(l: &[&'t str]) -> &'t str {
    l[hasard(l.len())]
}

const JOURS: [&str; 7] = ["dimanche", "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi"];
const MOIS: [&str; 12] = ["janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre", "décembre"];

/// Aujourd'hui, mais en 1998 (heure de Paris) : (jour de la semaine, jour, mois).
pub fn aujourd_hui_1998(t: u64) -> (u32, u32, u32) {
    let h = v34_net::tarif::paris(t);
    let jour = if h.mois == 2 && h.jour == 29 { 28 } else { h.jour };
    (v34_net::tarif::jour_semaine(1998, h.mois, jour), jour, h.mois)
}

/// « samedi 14 novembre 1998 »
pub fn date_1998(t: u64) -> String {
    let (js, j, m) = aujourd_hui_1998(t);
    let j = if j == 1 { "1er".to_string() } else { j.to_string() };
    format!("{} {j} {} 1998", JOURS[js as usize], MOIS[m as usize - 1])
}

/// Petits fichiers JSON du dossier de données (répondeur, scores) ; sans
/// dossier (tests, central sans site), ils restent en mémoire.
static FICHIERS: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());

/// Lit, modifie et réécrit `nom` d'un coup (deux appels simultanés ne se
/// marchent pas dessus, ni la modération du site : verrou du dossier,
/// [`crate::verrou`]). Rend ce que renvoie `f`.
pub fn modifier<T, R>(donnees: Option<&Path>, nom: &str, f: impl FnOnce(&mut T) -> R) -> R
where
    T: Serialize + DeserializeOwned + Default,
{
    let mut memoire = FICHIERS.lock().unwrap_or_else(|e| e.into_inner());
    let _verrou = donnees.map(crate::verrou::exclusif);
    let mut v: T = charger(donnees, nom, &memoire);
    let r = f(&mut v);
    let Ok(t) = serde_json::to_string(&v) else { return r };
    match donnees {
        Some(d) => {
            let tmp = d.join(format!("{nom}.tmp"));
            if let Err(e) = std::fs::write(&tmp, &t).and_then(|()| std::fs::rename(&tmp, d.join(nom))) {
                warn!("{nom} non écrit : {e}");
            }
        }
        None => {
            memoire.insert(nom.to_string(), t);
        }
    }
    r
}

/// Lecture seule.
pub fn lire<T: DeserializeOwned + Default>(donnees: Option<&Path>, nom: &str) -> T {
    let memoire = FICHIERS.lock().unwrap_or_else(|e| e.into_inner());
    charger(donnees, nom, &memoire)
}

fn charger<T: DeserializeOwned + Default>(donnees: Option<&Path>, nom: &str, memoire: &BTreeMap<String, String>) -> T {
    let texte = match donnees {
        Some(d) => std::fs::read_to_string(d.join(nom)).ok(),
        None => memoire.get(nom).cloned(),
    };
    texte.and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}
