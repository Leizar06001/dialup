//! Numéros cachés : des services du central qui ne figurent ni dans
//! l'annuaire ni dans la configuration. On les trouve grâce aux indices semés
//! un peu partout (l'oracle, l'horloge, la météo, le ministère…), et chacun
//! rapporte un succès (le nom du service, voir `v34_net::succes`).
//!
//! Comme 3615 ORACLE, ce sont de vrais services Minitel (V.23, Vidéotex :
//! [`crate::ecran`]), chacun avec sa page ([`Cadre`]) ; de petits scripts,
//! sans IA. Les touches de fonction du Minitel y ont leur sens (GUIDE,
//! RÉPÉTITION, SUITE…, voir [`Ligne::touches`]), et les commandes tapées à
//! l'ancienne (« /fin ») marchent toujours.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde::Serialize;

use v34_net::videotex::{Touche, BLANC, BLEU, CYAN, JAUNE, VERT};

pub use crate::ecran::Style;
use crate::ecran::{Ecran, Raccroche, Saisie, Taille};
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
    /// Il a appuyé sur CONNEXION/FIN (ou tapé /fin)
    Demande,
    /// Il ne dit plus rien depuis trop longtemps
    Inactivite,
}

impl From<Raccroche> for Fin {
    fn from(_: Raccroche) -> Fin {
        Fin::Raccroche
    }
}

/// La page d'un numéro caché : bandeau de titre, zone de texte, champ de
/// saisie et rappel des touches.
///
/// ```text
///  1 │▒▒ TITRE EN DOUBLE HAUTEUR ▒▒▒▒▒▒▒▒▒▒▒▒▒│  ← bandeau
///  2 │▒▒                                     ▒│
///  3 │ sous-titre
///  4 │ ────────────────────────────────────────
///  5 │ zone de texte
///    │ …
/// 20 │
/// 21 │ ────────────────────────────────────────
/// 22 │ > ......................................   ← champ (2 rangées)
/// 23 │ ........................................
/// 24 │  ENVOI  …                CONNEXION/FIN
/// ```
pub struct Cadre {
    /// En capitales (le Minitel n'accentue pas les majuscules)
    pub titre: &'static str,
    pub sous_titre: String,
    /// Couleur du titre et du bandeau
    pub fg: u8,
    pub fond: u8,
    /// Rappel des touches, rangée 24 (après « ENVOI »)
    pub touches: &'static str,
    /// Invite devant le champ de saisie
    pub invite: &'static str,
}

const ZONE: (u8, u8) = (5, 20);
const RANGEE_CHAMP: u8 = 22;

/// La ligne d'un appel : on écrit à l'écran, on lit ce que l'appelant tape.
pub struct Ligne<'a> {
    pub ecran: Ecran,
    pub abonne: &'a str,
    pub donnees: Option<&'a Path>,
    exploits: Option<&'a Exploits>,
    /// Touches de fonction → commandes du service (« GUIDE » → « AIDE »…)
    touches: Vec<(Touche, &'static str)>,
    /// Le texte de GUIDE (sans commande associée)
    guide: Option<String>,
    invite: String,
}

impl<'a> Ligne<'a> {
    pub fn new(tx: SyncSender<Vec<u8>>, rx: Receiver<Vec<u8>>, abonne: &'a str, donnees: Option<&'a Path>, debit: u32) -> Ligne<'a> {
        Ligne { ecran: Ecran::new(tx, rx, debit), abonne, donnees, exploits: None, touches: Vec::new(), guide: None, invite: ">".into() }
    }

    /// Un succès gagné pendant l'appel (noté par le central).
    pub fn succes(&self, id: &'static str) {
        if let Some(e) = self.exploits {
            e.lock().unwrap_or_else(|p| p.into_inner()).push((self.abonne.to_string(), id));
        }
    }

    /// Ce que valent les touches de fonction pour ce service : la commande
    /// que [`Ligne::lire`] rendra, comme si elle avait été tapée.
    pub fn touches(&mut self, t: &[(Touche, &'static str)]) {
        self.touches = t.to_vec();
    }

    /// Le texte que montre GUIDE (si GUIDE n'est pas une commande du service).
    pub fn guide(&mut self, texte: &str) {
        self.guide = Some(texte.to_string());
    }

    /// Dessine la page du service.
    pub fn cadre(&mut self, c: &Cadre) -> Result<(), Fin> {
        let e = &mut self.ecran;
        e.effacer()?;
        e.barre(1, 1, 40, c.fond)?;
        e.barre(2, 1, 40, c.fond)?;
        e.ecrire(2, 2, Style::fg(c.fg).fond(c.fond).taille(Taille::DoubleHauteur), &format!(" {}", c.titre))?;
        e.ecrire(3, 2, Style::fg(CYAN), &c.sous_titre)?;
        e.filet(4, BLEU)?;
        e.filet(21, BLEU)?;
        e.ecrire(24, 1, Style::fg(BLANC).inverse(), " ENVOI ")?;
        e.ecrire(24, 8, Style::NORMAL, &format!(" {}", c.touches))?;
        e.zone(ZONE.0, ZONE.1, Style::NORMAL);
        self.invite = c.invite.to_string();
        self.dessiner_champ()
    }

    fn dessiner_champ(&mut self) -> Result<(), Fin> {
        let l = v34_net::videotex::largeur(&self.invite) as u8;
        self.ecran.ecrire(RANGEE_CHAMP, 1, Style::fg(VERT), &format!("{} ", self.invite))?;
        let col = l + 2;
        self.ecran.champ(RANGEE_CHAMP, col, 80 - col as usize + 1, Style::NORMAL)?;
        Ok(())
    }

    /// Change l'invite devant le champ de saisie (« SLP:/DIRECTION> »).
    pub fn invite(&mut self, invite: &str) -> Result<(), Fin> {
        if invite != self.invite {
            let blanc = " ".repeat(v34_net::videotex::largeur(&self.invite) + 1);
            self.ecran.ecrire(RANGEE_CHAMP, 1, Style::NORMAL, &blanc)?;
            self.invite = invite.to_string();
            self.dessiner_champ()?;
        }
        Ok(())
    }

    /// L'invite actuelle.
    pub fn invite_actuelle(&self) -> &str {
        &self.invite
    }

    /// Nouvelles lignes à l'écran.
    pub fn dire(&mut self, texte: &str) -> Result<(), Fin> {
        Ok(self.ecran.dire(texte)?)
    }

    /// Nouvelles lignes, dans ce style.
    pub fn dire_style(&mut self, texte: &str, style: Style) -> Result<(), Fin> {
        Ok(self.ecran.dire_style(texte, style)?)
    }

    /// Suite de la ligne en cours.
    pub fn suite(&mut self, texte: &str) -> Result<(), Fin> {
        Ok(self.ecran.suite(texte)?)
    }

    /// Une ligne envoyée (ENVOI) ou une touche de fonction du service, en
    /// attendant au plus `attente` (`None` : rien). CONNEXION/FIN (ou /fin
    /// tapé) raccroche ; trop longtemps sans rien taper aussi.
    pub fn lire(&mut self, attente: Duration) -> Result<Option<String>, Fin> {
        let fin = Instant::now() + attente;
        loop {
            let reste = fin.saturating_duration_since(Instant::now());
            match self.ecran.lire(reste)? {
                Some(Saisie::Ligne(brute)) => {
                    let l = assainir(&brute);
                    if l.is_empty() {
                        continue;
                    }
                    if matches!(l.to_lowercase().as_str(), "/fin" | "fin" | "/quitter" | "connexion/fin") {
                        return Err(Fin::Demande);
                    }
                    return Ok(Some(l));
                }
                Some(Saisie::Touche(Touche::ConnexionFin)) => return Err(Fin::Demande),
                Some(Saisie::Touche(t)) => {
                    if let Some((_, cmd)) = self.touches.iter().find(|(k, _)| *k == t) {
                        return Ok(Some(cmd.to_string()));
                    }
                    match (t, self.guide.clone()) {
                        (Touche::Guide, Some(g)) => self.dire_style(&g, Style::fg(JAUNE))?,
                        // une touche sans effet ici : le Minitel bipe
                        _ => self.ecran.bip()?,
                    }
                }
                None => {
                    if self.ecran.dernier_clavier.elapsed() > INACTIVITE {
                        return Err(Fin::Inactivite);
                    }
                    if Instant::now() >= fin {
                        return Ok(None);
                    }
                }
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
        Ok(self.ecran.pause(d)?)
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
    let mut l = Ligne::new(to_modem, from_modem, abonne, donnees, DEBIT);
    l.exploits = Some(exploits);
    let fin = match (n.lancer)(&mut l) {
        Ok(()) => "raccroché par le service",
        Err(Fin::Raccroche) => "raccroché par l'appelant",
        Err(Fin::Demande) => {
            let _ = l.ecran.sans_champ();
            let _ = l.dire_style("Au revoir ! *clic*", Style::fg(JAUNE));
            "CONNEXION/FIN"
        }
        Err(Fin::Inactivite) => {
            let _ = l.ecran.sans_champ();
            let _ = l.dire_style("Plus personne au bout du fil ? Le service raccroche. *clic*", Style::fg(JAUNE));
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
