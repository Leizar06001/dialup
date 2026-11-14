//! 3615 ORACLE : une IA qu'on ne joint qu'en composant le 08 36 67 22 53
//! avec son modem, et qui est persuadée qu'on est en 1998.
//!
//! Le modem du central décroche comme pour la ligne de test, puis l'oracle
//! parle le protocole des appels entre abonnés ([`v34_net::bbs`]) : il
//! reçoit les questions en trames de chat et répond en trames de chat. Une
//! appli 0.1.2 sans modification s'en sert donc comme d'un correspondant
//! ordinaire : la MESSAGERIE s'ouvre à CONNECT, on y tape ses questions.
//!
//! Affichage : chaque ligne commence par un retour à la ligne et le texte
//! ne finit jamais par un retour à la ligne. La messagerie, qui assemble le
//! texte distant jusqu'au retour à la ligne, affiche ainsi chaque ligne une
//! seule fois, au bon endroit, et voit les réponses s'écrire au fil de
//! l'eau : elles partent par petits morceaux, à la cadence d'un Minitel
//! (1200 bit/s par défaut, 10 bits par caractère).
//!
//! Les réponses viennent d'OpenRouter ([`ia`]) ; sans clé, au-delà des
//! quotas, en cas de panne ou de délai dépassé, ce sont les cartes
//! ([`persona::cartes`]) qui répondent. Jamais d'appel bloqué : tout est
//! borné par des délais.
//!
//! Vie privée : questions et réponses ne sont jamais journalisées (seuls
//! des compteurs le sont), ni conservées après l'appel (la mémoire de la
//! conversation meurt avec lui ; le petit cache des réponses aux premières
//! questions est en mémoire, sans nom d'abonné, et expire en quelques heures).

pub mod ia;
pub mod persona;
pub mod texte;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use v34_net::bbs::{self, Decoder, Frame, Item};

use crate::config::{self, Annuaire};
use crate::{debug, info, warn};

/// Échanges gardés en mémoire pendant un appel : au-delà de `MEMOIRE_MAX`,
/// on oublie d'un coup les plus anciens pour n'en garder que
/// `MEMOIRE_GARDEE` (le début de la conversation reste ainsi identique
/// plusieurs questions de suite, et le cache du fournisseur sert).
pub const MEMOIRE_MAX: usize = 8;
pub const MEMOIRE_GARDEE: usize = 4;
/// Cache des réponses aux premières questions (identiques après normalisation)
const CACHE_MAX: usize = 256;
const CACHE_DUREE: Duration = Duration::from_secs(6 * 3600);
/// Une file de questions en attente au plus pendant que l'oracle répond
const FILE_MAX: usize = 1;
/// Cadence d'émission des morceaux de texte
const TIC: Duration = Duration::from_millis(100);
/// Caractères au plus par trame (bien moins que `bbs::MAX_PAYLOAD` octets)
const MORCEAU_MAX: usize = 400;
/// Un point d'attente toutes les…
const POINTILLES: Duration = Duration::from_millis(700);
const POINTS_MAX: usize = 24;
/// Pause théâtrale pendant que l'oracle « bat les cartes »
const ATTENTE_CARTES: Duration = Duration::from_millis(1400);
/// Longueur maximale de la clé d'API lue dans le fichier
const CLE_MAX: usize = 512;

/// Compteurs depuis le démarrage (journalisés en mode debug ; jamais de contenu).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Compteurs {
    pub appels: u64,
    pub questions: u64,
    /// réponses de l'IA
    pub ia: u64,
    /// réponses tirées du cache des premières questions
    pub cache: u64,
    /// réponses des cartes (hors ligne, quota global, panne)
    pub cartes: u64,
    /// abonnés au bout de leur quota
    pub refus: u64,
    /// pannes ou délais de l'IA
    pub erreurs: u64,
    pub jetons_prompt: u64,
    pub jetons_caches: u64,
    pub jetons_reponse: u64,
}

/// D'où vient une réponse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Ia,
    Cache,
    Cartes,
    /// Quota de l'abonné épuisé : pas de réponse, l'oracle raccroche
    Refus,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Reponse {
    /// Texte à afficher (une note entre parenthèses peut précéder, sur sa ligne)
    pub texte: String,
    pub source: Source,
}

/// Une conversation (un appel).
#[derive(Debug)]
pub struct Conversation {
    abonne: String,
    /// (question, réponse) tels qu'envoyés à l'IA et reçus d'elle
    echanges: Vec<(String, String)>,
    /// Tirage des cartes
    graine: u64,
}

impl Conversation {
    pub fn new(abonne: &str) -> Conversation {
        let graine = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
        Conversation { abonne: abonne.to_string(), echanges: Vec::new(), graine }
    }

    pub fn echanges(&self) -> &[(String, String)] {
        &self.echanges
    }

    fn retenir(&mut self, question: String, reponse: String) {
        self.echanges.push((question, reponse));
        if self.echanges.len() > MEMOIRE_MAX {
            let oublier = self.echanges.len() - MEMOIRE_GARDEE;
            self.echanges.drain(..oublier);
        }
    }

    fn cartes(&mut self, question: &str) -> String {
        self.graine = self.graine.wrapping_add(0x2545_f491_4f6c_dd1d);
        persona::cartes(question, self.graine)
    }
}

/// Requête prête à partir vers l'IA (place déjà réservée dans les quotas).
pub struct Requete {
    question: String,
    corps: String,
    /// Première question de l'appel : la réponse ira au cache
    premiere: bool,
}

pub enum Preparation {
    /// Réponse connue sans l'IA (cartes, cache, refus)
    Immediate(Reponse),
    Requete(Requete),
}

struct Etat {
    /// Jour (UTC) des compteurs de quotas
    jour: u64,
    par_abonne: HashMap<String, u32>,
    total: u32,
    cache: HashMap<String, (Instant, String)>,
    compteurs: Compteurs,
}

pub struct Oracle {
    cfg: config::Oracle,
    /// Tous les abonnés peuvent appeler (sinon : ceux qui ont l'accès Internet)
    tous: bool,
    agent: ureq::Agent,
    etat: Mutex<Etat>,
    /// Pause pendant que l'oracle « bat les cartes » (nulle dans les tests)
    pub attente_cartes: Duration,
}

fn jour_utc() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86_400)
}

/// Lit la clé d'API : une ligne de caractères ASCII visibles. Rien n'est
/// journalisé de son contenu.
pub fn lire_cle(chemin: &str) -> Result<String, String> {
    if chemin.trim().is_empty() {
        return Err("aucun fichier de clé configuré".into());
    }
    let p = Path::new(chemin.trim());
    let meta = std::fs::metadata(p).map_err(|e| format!("{} : {}", p.display(), e.kind()))?;
    if meta.len() > CLE_MAX as u64 + 64 {
        return Err(format!("{} : fichier trop gros pour une clé", p.display()));
    }
    let s = std::fs::read_to_string(p).map_err(|e| format!("{} : {}", p.display(), e.kind()))?;
    let cle = s.trim();
    if cle.is_empty() {
        return Err(format!("{} : vide", p.display()));
    }
    if cle.len() > CLE_MAX || !cle.chars().all(|c| c.is_ascii_graphic()) {
        return Err(format!("{} : contenu invalide (une seule ligne, sans espace)", p.display()));
    }
    Ok(cle.to_string())
}

impl Oracle {
    pub fn new(cfg: &config::Oracle) -> Result<Oracle, String> {
        let tous = cfg.validate()?;
        Ok(Oracle {
            cfg: cfg.clone(),
            tous,
            agent: ia::agent(),
            etat: Mutex::new(Etat { jour: jour_utc(), par_abonne: HashMap::new(), total: 0, cache: HashMap::new(), compteurs: Compteurs::default() }),
            attente_cartes: ATTENTE_CARTES,
        })
    }

    pub fn config(&self) -> &config::Oracle {
        &self.cfg
    }

    /// L'abonné peut-il appeler l'oracle ?
    pub fn autorise(&self, annuaire: &Annuaire, nom: &str) -> bool {
        if self.tous {
            annuaire.get(nom).is_some()
        } else {
            annuaire.fai_autorise(nom)
        }
    }

    /// L'accès suit-il celui du FAI (et se retire avec lui) ?
    pub fn reserve_au_fai(&self) -> bool {
        !self.tous
    }

    /// La clé d'API, relue à chaque appel (l'administrateur peut la poser
    /// sans redémarrer le central).
    pub fn cle(&self) -> Result<String, String> {
        lire_cle(&self.cfg.fichier_cle)
    }

    pub fn compteurs(&self) -> Compteurs {
        self.etat.lock().map(|e| e.compteurs.clone()).unwrap_or_default()
    }

    fn etat(&self) -> std::sync::MutexGuard<'_, Etat> {
        self.etat.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Nouveau jour : les quotas repartent à zéro.
    fn au_jour(e: &mut Etat, jour: u64) {
        if e.jour != jour {
            e.jour = jour;
            e.par_abonne.clear();
            e.total = 0;
        }
    }

    /// Questions à l'IA qu'il reste aujourd'hui à cet abonné.
    pub fn restantes(&self, abonne: &str) -> u32 {
        self.restantes_le(abonne, jour_utc())
    }

    fn restantes_le(&self, abonne: &str, jour: u64) -> u32 {
        let mut e = self.etat();
        Self::au_jour(&mut e, jour);
        self.cfg.questions_par_jour.saturating_sub(e.par_abonne.get(abonne).copied().unwrap_or(0))
    }

    /// Réserve une question dans les quotas : `Err(true)` = quota de
    /// l'abonné épuisé, `Err(false)` = plafond global du jour atteint.
    fn reserver(&self, abonne: &str, jour: u64) -> Result<(), bool> {
        let mut e = self.etat();
        Self::au_jour(&mut e, jour);
        let n = e.par_abonne.get(abonne).copied().unwrap_or(0);
        if n >= self.cfg.questions_par_jour {
            return Err(true);
        }
        if e.total >= self.cfg.questions_par_jour_total {
            return Err(false);
        }
        e.par_abonne.insert(abonne.to_string(), n + 1);
        e.total += 1;
        Ok(())
    }

    /// Rend une question réservée (l'IA n'a rien produit, sans doute rien facturé).
    fn rendre(&self, abonne: &str) {
        let mut e = self.etat();
        if let Some(n) = e.par_abonne.get_mut(abonne) {
            *n = n.saturating_sub(1);
        }
        e.total = e.total.saturating_sub(1);
    }

    fn cache_lire(&self, question: &str) -> Option<String> {
        let mut e = self.etat();
        let cle = texte::normaliser(question);
        match e.cache.get(&cle) {
            Some((t, r)) if t.elapsed() < CACHE_DUREE => Some(r.clone()),
            Some(_) => {
                e.cache.remove(&cle);
                None
            }
            None => None,
        }
    }

    fn cache_ecrire(&self, question: &str, reponse: &str) {
        let mut e = self.etat();
        if e.cache.len() >= CACHE_MAX {
            e.cache.retain(|_, (t, _)| t.elapsed() < CACHE_DUREE);
        }
        while e.cache.len() >= CACHE_MAX {
            let plus_vieux = e.cache.iter().min_by_key(|(_, (t, _))| *t).map(|(k, _)| k.clone());
            match plus_vieux {
                Some(k) => e.cache.remove(&k),
                None => break,
            };
        }
        e.cache.insert(texte::normaliser(question), (Instant::now(), reponse.to_string()));
    }

    fn compter(&self, f: impl FnOnce(&mut Compteurs)) {
        f(&mut self.etat().compteurs);
    }

    /// Première étape d'une question : réponse immédiate (cartes, cache,
    /// refus) ou requête à envoyer à l'IA. `cle` : None = hors ligne.
    pub fn preparer(&self, conv: &mut Conversation, question: &str, cle: Option<&str>) -> Preparation {
        self.preparer_le(conv, question, cle, jour_utc())
    }

    fn preparer_le(&self, conv: &mut Conversation, question: &str, cle: Option<&str>, jour: u64) -> Preparation {
        self.compter(|c| c.questions += 1);
        if cle.is_none() {
            self.compter(|c| c.cartes += 1);
            return Preparation::Immediate(Reponse { texte: conv.cartes(question), source: Source::Cartes });
        }
        let premiere = conv.echanges.is_empty();
        if premiere {
            if let Some(r) = self.cache_lire(question) {
                self.compter(|c| c.cache += 1);
                conv.retenir(question.to_string(), r.clone());
                return Preparation::Immediate(Reponse { texte: r, source: Source::Cache });
            }
        }
        match self.reserver(&conv.abonne, jour) {
            Err(true) => {
                self.compter(|c| c.refus += 1);
                Preparation::Immediate(Reponse { texte: persona::QUOTA_ABONNE.into(), source: Source::Refus })
            }
            Err(false) => {
                self.compter(|c| c.cartes += 1);
                Preparation::Immediate(Reponse { texte: format!("{}\n{}", persona::QUOTA_GLOBAL, conv.cartes(question)), source: Source::Cartes })
            }
            Ok(()) => Preparation::Requete(Requete {
                question: question.to_string(),
                corps: ia::corps(&self.cfg.modele, self.cfg.jetons_max, &conv.echanges, question),
                premiere,
            }),
        }
    }

    /// Délai d'une requête à l'IA.
    pub fn delai(&self) -> Duration {
        Duration::from_secs(self.cfg.delai_s)
    }

    /// Envoie la requête (bloquant, borné par le délai de la configuration).
    pub fn appeler(&self, corps: &str, cle: &str) -> Result<ia::Reponse, ia::Erreur> {
        ia::appeler(&self.agent, &self.cfg.url, cle, corps, self.delai())
    }

    /// Dernière étape : réponse de l'IA nettoyée et retenue, ou les cartes.
    pub fn conclure(&self, conv: &mut Conversation, req: Requete, res: Result<ia::Reponse, ia::Erreur>) -> Reponse {
        let res = res.and_then(|r| {
            let t = texte::couper(&texte::assainir_reponse(&r.texte), texte::REPONSE_MAX, r.coupee);
            if t.is_empty() {
                Err(ia::Erreur::Invalide("réponse vide une fois nettoyée"))
            } else {
                Ok((t, r.usage))
            }
        });
        match res {
            Ok((t, usage)) => {
                let total = {
                    let mut e = self.etat();
                    let c = &mut e.compteurs;
                    c.ia += 1;
                    if let Some(u) = usage {
                        c.jetons_prompt += u.prompt;
                        c.jetons_caches += u.caches;
                        c.jetons_reponse += u.reponse;
                    }
                    c.clone()
                };
                if let Some(u) = usage {
                    debug!(
                        "ORACLE : jetons {} (dont {} en cache) + {} en réponse ; depuis le démarrage : {} (dont {} en cache), {} en réponse, {} réponse(s) de l'IA",
                        u.prompt, u.caches, u.reponse, total.jetons_prompt, total.jetons_caches, total.jetons_reponse, total.ia
                    );
                }
                if req.premiere {
                    self.cache_ecrire(&req.question, &t);
                }
                conv.retenir(req.question, t.clone());
                Reponse { texte: t, source: Source::Ia }
            }
            Err(e) => {
                // Délai dépassé : la requête a pu être facturée, on la garde
                // dans les quotas. Sinon on la rend.
                if e != ia::Erreur::Delai {
                    self.rendre(&conv.abonne);
                }
                warn!("ORACLE : IA indisponible ({e}), les cartes répondent");
                self.compter(|c| {
                    c.erreurs += 1;
                    c.cartes += 1;
                });
                let note = if e == ia::Erreur::Delai { persona::NOTE_DELAI } else { persona::NOTE_PANNE };
                Reponse { texte: format!("{note}\n{}", conv.cartes(&req.question)), source: Source::Cartes }
            }
        }
    }

    /// Question → réponse, d'un bloc (tests, outils).
    pub fn repondre(&self, conv: &mut Conversation, question: &str, cle: Option<&str>) -> Reponse {
        match self.preparer(conv, question, cle) {
            Preparation::Immediate(r) => r,
            Preparation::Requete(req) => {
                let res = self.appeler(&req.corps, cle.unwrap_or_default());
                self.conclure(conv, req, res)
            }
        }
    }
}

// ───────────────────────── l'écran ─────────────────────────

/// L'appelant a raccroché.
#[derive(Debug)]
pub struct Raccroche;

/// Écrit sur la ligne, en trames de chat, à la cadence d'un Minitel.
pub struct Ecran {
    sortie: SyncSender<Vec<u8>>,
    /// caractères par seconde
    cps: f64,
    credit: f64,
    prochain: Instant,
}

impl Ecran {
    /// `bit_s` : cadence d'affichage, 10 bits par caractère.
    pub fn new(sortie: SyncSender<Vec<u8>>, bit_s: u32) -> Ecran {
        Ecran { sortie, cps: bit_s.max(10) as f64 / 10.0, credit: 0.0, prochain: Instant::now() }
    }

    /// Nouvelles lignes (texte de une ou plusieurs lignes, séparées par
    /// `\n`), chacune précédée d'un retour à la ligne : elle ferme la ligne
    /// en cours chez l'appelant, même celle d'un appel précédent.
    pub fn lignes(&mut self, texte: &str) -> Result<(), Raccroche> {
        let mut s = String::with_capacity(texte.len() + 8);
        for l in texte.split('\n') {
            s.push('\n');
            s.push_str(l);
        }
        self.taper(&s)
    }

    /// Suite de la ligne en cours (points d'attente…).
    pub fn suite(&mut self, texte: &str) -> Result<(), Raccroche> {
        self.taper(texte)
    }

    /// Envoie le texte par petits morceaux, à la cadence choisie.
    fn taper(&mut self, s: &str) -> Result<(), Raccroche> {
        let cs: Vec<char> = s.chars().collect();
        let mut i = 0;
        let maintenant = Instant::now();
        if maintenant > self.prochain + TIC {
            // l'écran était au repos : on repart d'ici, sans rattrapage
            self.prochain = maintenant;
            self.credit = 0.0;
        }
        while i < cs.len() {
            let now = Instant::now();
            if self.prochain > now {
                std::thread::sleep(self.prochain - now);
            }
            self.prochain += TIC;
            self.credit += self.cps * TIC.as_secs_f64();
            let n = (self.credit.floor() as usize).min(cs.len() - i).min(MORCEAU_MAX);
            if n == 0 {
                continue;
            }
            self.credit -= n as f64;
            let morceau: String = cs[i..i + n].iter().collect();
            i += n;
            self.sortie.send(bbs::encode(&Frame::Chat(morceau))).map_err(|_| Raccroche)?;
        }
        Ok(())
    }
}

// ───────────────────────── la consultation ─────────────────────────

enum Commande {
    Fin,
    Aide,
    Sommaire,
    Inconnue,
}

fn commande(q: &str) -> Option<Commande> {
    let t = q.trim().to_lowercase();
    let mot = t.strip_prefix('/').unwrap_or(&t);
    let c = match mot {
        "fin" | "quitter" | "connexion/fin" => Commande::Fin,
        "aide" | "guide" | "?" => Commande::Aide,
        "sommaire" => Commande::Sommaire,
        _ if t.starts_with('/') => Commande::Inconnue,
        _ => return None,
    };
    Some(c)
}

/// Ce que l'appelant envoie : questions (trames de chat, ou lignes de texte
/// brut d'un terminal à l'ancienne) et fichiers (refusés poliment).
struct Entree {
    dec: Decoder,
    brut: String,
    file: VecDeque<String>,
    fichier: bool,
}

impl Entree {
    fn pousser(&mut self, octets: &[u8]) {
        for it in self.dec.push(octets) {
            match it {
                Item::Frame(Frame::Chat(t)) => self.file.push_back(t),
                Item::Frame(Frame::FileOffer { .. }) => self.fichier = true,
                Item::Raw(r) => {
                    self.brut.push_str(&String::from_utf8_lossy(&r));
                    while let Some(i) = self.brut.find(['\r', '\n']) {
                        let l: String = self.brut.drain(..=i).collect();
                        if !l.trim().is_empty() {
                            self.file.push_back(l);
                        }
                    }
                    if self.brut.len() > 4096 {
                        self.brut.clear();
                    }
                }
                // visiophone, morceaux de fichiers… : ignorés
                _ => {}
            }
        }
    }

    /// Lit ce qui est arrivé, sans attendre. Faux si l'appelant a raccroché.
    fn relever(&mut self, rx: &Receiver<Vec<u8>>) -> bool {
        loop {
            match rx.try_recv() {
                Ok(b) => self.pousser(&b),
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => return false,
            }
        }
    }
}

/// Statistiques d'un appel (journalisées à la fin, sans contenu).
#[derive(Default)]
struct Bilan {
    questions: u32,
    ia: u32,
    cache: u32,
    cartes: u32,
}

impl Bilan {
    fn noter(&mut self, s: Source) {
        self.questions += 1;
        match s {
            Source::Ia => self.ia += 1,
            Source::Cache => self.cache += 1,
            Source::Cartes => self.cartes += 1,
            Source::Refus => {}
        }
    }
}

/// Une consultation, du CONNECT au raccroché : `from_modem` / `to_modem`
/// sont les deux bouts d'un [`v34_net::tunnel::ModemPipe`]. Renvoie la
/// raison de la fin (journalisée par l'appelant).
pub fn session(oracle: Arc<Oracle>, label: &str, abonne: &str, from_modem: Receiver<Vec<u8>>, to_modem: SyncSender<Vec<u8>>) -> String {
    oracle.compter(|c| c.appels += 1);
    let debut = Instant::now();
    let mut bilan = Bilan::default();
    let fin = consultation(&oracle, label, abonne, &from_modem, to_modem, debut, &mut bilan);
    info!(
        "{label} : oracle, {} question(s) : {} de l'IA, {} du cache, {} des cartes ; {} s",
        bilan.questions,
        bilan.ia,
        bilan.cache,
        bilan.cartes,
        debut.elapsed().as_secs()
    );
    fin
}

fn consultation(
    oracle: &Arc<Oracle>,
    label: &str,
    abonne: &str,
    rx: &Receiver<Vec<u8>>,
    tx: SyncSender<Vec<u8>>,
    debut: Instant,
    bilan: &mut Bilan,
) -> String {
    const RACCROCHE: &str = "raccroché par l'appelant";
    let cle = match oracle.cle() {
        Ok(c) => Some(c),
        Err(e) => {
            info!("{label} : oracle sans clé d'API ({e}) : les cartes répondent");
            None
        }
    };
    let mut ecran = Ecran::new(tx, oracle.cfg.debit_affichage);
    let mut conv = Conversation::new(abonne);
    let restantes = cle.as_ref().map(|_| oracle.restantes(abonne));
    if ecran.lignes(&persona::accueil(abonne, restantes)).is_err() {
        return RACCROCHE.into();
    }
    if restantes == Some(0) {
        oracle.compter(|c| c.refus += 1);
        let _ = ecran.lignes(persona::QUOTA_ABONNE);
        return "quota du jour épuisé".into();
    }
    let mut entree = Entree { dec: Decoder::default(), brut: String::new(), file: VecDeque::new(), fichier: false };
    let mut fichier_dit = false;
    let mut actif = Instant::now();
    let inactivite = Duration::from_secs(oracle.cfg.inactivite_s);
    let au_revoir = |ecran: &mut Ecran| {
        let _ = ecran.lignes(&persona::au_revoir(debut.elapsed().as_secs()));
    };
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(b) => entree.pousser(&b),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return RACCROCHE.into(),
        }
        if !entree.relever(rx) {
            return RACCROCHE.into();
        }
        if entree.fichier && !fichier_dit {
            fichier_dit = true;
            if ecran.lignes(persona::FICHIER).is_err() {
                return RACCROCHE.into();
            }
        }
        let Some(brute) = entree.file.pop_front() else {
            if actif.elapsed() > inactivite {
                let _ = ecran.lignes(persona::INACTIVITE);
                au_revoir(&mut ecran);
                return "raccroché par l'oracle (inactivité)".into();
            }
            continue;
        };
        actif = Instant::now();
        let r = match commande(&brute) {
            Some(Commande::Fin) => {
                au_revoir(&mut ecran);
                return "consultation terminée (/fin)".into();
            }
            Some(Commande::Aide) => ecran.lignes(persona::GUIDE),
            Some(Commande::Sommaire) => ecran.lignes(&persona::accueil(abonne, cle.as_ref().map(|_| oracle.restantes(abonne)))),
            Some(Commande::Inconnue) => ecran.lignes(persona::COMMANDE_INCONNUE),
            None => {
                let q = texte::assainir_question(&brute);
                if q.is_empty() {
                    continue;
                }
                match consulter(oracle, &mut conv, &q, cle.as_deref(), &mut ecran, rx, &mut entree) {
                    Ok(rep) => {
                        bilan.noter(rep.source);
                        debug!("{label} : question n° {} ({:?})", bilan.questions, rep.source);
                        if rep.source == Source::Refus {
                            return "quota du jour épuisé".into();
                        }
                        ecran.lignes(&persona::compteur(debut.elapsed().as_secs()))
                    }
                    Err(Raccroche) => return RACCROCHE.into(),
                }
            }
        };
        if r.is_err() {
            return RACCROCHE.into();
        }
        actif = Instant::now();
        if entree.file.len() > FILE_MAX {
            entree.file.truncate(FILE_MAX);
            if ecran.lignes(persona::UNE_A_LA_FOIS).is_err() {
                return RACCROCHE.into();
            }
        }
    }
}

/// Pose une question et affiche la réponse ; les questions qui arrivent
/// pendant l'attente vont dans la file d'`entree`.
fn consulter(
    oracle: &Arc<Oracle>,
    conv: &mut Conversation,
    question: &str,
    cle: Option<&str>,
    ecran: &mut Ecran,
    rx: &Receiver<Vec<u8>>,
    entree: &mut Entree,
) -> Result<Reponse, Raccroche> {
    let rep = match oracle.preparer(conv, question, cle) {
        Preparation::Immediate(rep) => {
            if rep.source == Source::Cartes && !oracle.attente_cartes.is_zero() {
                ecran.lignes(persona::ATTENTE_CARTES)?;
                let fin = Instant::now() + oracle.attente_cartes;
                while Instant::now() < fin {
                    std::thread::sleep(POINTILLES.min(oracle.attente_cartes));
                    if !entree.relever(rx) {
                        return Err(Raccroche);
                    }
                    ecran.suite(".")?;
                }
            }
            rep
        }
        Preparation::Requete(req) => {
            ecran.lignes(persona::ATTENTE_ASTRES)?;
            let (rtx, rrx) = mpsc::channel();
            let o = oracle.clone();
            let corps = req.corps.clone();
            let cle = cle.unwrap_or_default().to_string();
            let lance = std::thread::Builder::new().name("oracle-ia".into()).spawn(move || {
                let _ = rtx.send(o.appeler(&corps, &cle));
            });
            let res = match lance {
                Err(_) => Err(ia::Erreur::Reseau("thread".into())),
                Ok(_) => {
                    // le délai de ureq borne la requête ; celui-ci borne l'attente quoi qu'il arrive
                    let limite = Instant::now() + oracle.delai() + Duration::from_secs(2);
                    let mut points = 0;
                    loop {
                        match rrx.recv_timeout(POINTILLES) {
                            Ok(r) => break r,
                            Err(RecvTimeoutError::Disconnected) => break Err(ia::Erreur::Reseau("thread".into())),
                            Err(RecvTimeoutError::Timeout) => {}
                        }
                        if !entree.relever(rx) {
                            return Err(Raccroche);
                        }
                        if Instant::now() > limite {
                            break Err(ia::Erreur::Delai);
                        }
                        if points < POINTS_MAX {
                            points += 1;
                            ecran.suite(".")?;
                        }
                    }
                }
            };
            oracle.conclure(conv, req, res)
        }
    };
    ecran.lignes(&rep.texte)?;
    Ok(rep)
}
