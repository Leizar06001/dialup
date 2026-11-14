//! Le central téléphonique : une socket UDP, une boucle.
//!
//! - signalisation ([`v34_net::signal`]) : enregistrement par nom d'abonné
//!   et défi-réponse façon SCRAM ([`v34_net::auth`]), numéro ATTRIBUÉ par
//!   l'annuaire, keepalive, DIAL / RING / ANSWER / HANGUP / BUSY /
//!   UNKNOWN_NUMBER ;
//! - relais des paquets audio ([`v34_net::packet`]) entre les deux bouts
//!   d'un appel, d'après l'identifiant d'appel de l'en-tête et l'adresse
//!   de l'émetteur (qui doit être celle d'un abonné authentifié de l'appel) ;
//! - appels vers les numéros spéciaux : remis à un modem ([`crate::services`]).
//!
//! Sans authentification valide, rien n'est relayé : seuls REGISTER (→
//! CHALLENGE) et AUTH (→ AUTH_FAIL) reçoivent une réponse, toujours plus
//! petite que la requête. Les échecs sont limités par adresse IP. Le
//! central ne décode pas l'audio, sauf si un simulateur de ligne est
//! configuré (`central.simulateur_ligne`).

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use v34_core::LineSim;
use v34_net::auth::{self, DEFAULT_ITERATIONS};
use v34_net::crypto::{ct_eq, hmac_sha256, random_bytes, random_u32_nonzero};
use v34_net::packet::{self, Packet};
use v34_net::signal::{
    self, AuthFailReason, Dedup, DirEntry, Frame, HangupReason, Msg, Outbox, Presence, Seen, SessionKey, CLIENT_NONCE_LEN,
    SERVER_NONCE_LEN,
};

use crate::comptes::Comptes;
use crate::config::{Annuaire, Config, Service};
use crate::facturation::{Facturation, Ticket};
use crate::oracle::Oracle;
use crate::services::{self, ModemCall, ModemInput, ModemNotice};
use crate::tunnel::TunnelPolicy;
use crate::{debug, error, info, warn};

/// Validité d'un défi (s).
const CHALLENGE_TTL: u64 = 30;
const HOUSEKEEPING: Duration = Duration::from_millis(50);
/// Fréquence de vérification du fichier de configuration.
const RELOAD_CHECK: Duration = Duration::from_secs(2);
/// Taille maximale d'un paquet audio relayé (4 trames L16).
const MAX_AUDIO: usize = packet::HEADER_LEN + 4 * packet::FRAME_SAMPLES * 2;
/// Adresses IP suivies au maximum par la limitation des échecs.
const MAX_TRACKED_IPS: usize = 100_000;

/// Compteurs exposés (tests, supervision).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub registrations: usize,
    pub calls: usize,
    /// Appels en cours vers un numéro spécial (modem du central)
    pub modem_calls: usize,
    pub relayed_packets: u64,
    pub dropped_packets: u64,
    pub auth_failures: u64,
}

struct Reg {
    name: String,
    number: String,
    key: SessionKey,
    client_nonce: [u8; CLIENT_NONCE_LEN],
    addr: SocketAddr,
    last_seen: Instant,
    outbox: Outbox,
    dedup: Dedup,
    call: Option<u32>,
    /// Dernier annuaire envoyé (au plus un par seconde)
    last_directory: Option<Instant>,
}

enum Callee {
    Client(u32),
    Modem { service: Service, answer_at: Instant, tx: Option<Sender<ModemInput>> },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CallState {
    Ringing,
    Active,
}

struct Call {
    caller: u32,
    callee: Callee,
    state: CallState,
    created: Instant,
    /// Pour le ticket : appelant (nom, numéro), numéro composé, appelé (nom
    /// ou service), et le décroché (horloge, secondes Unix)
    appelant: (String, String),
    numero: String,
    appele: String,
    debut: Option<(Instant, u64)>,
    /// Simulateurs de ligne : [appelant → appelé, appelé → appelant]
    sims: Option<Box<[LineSim; 2]>>,
}

/// Seau percé : chaque échec ajoute 1, le score fond de `echecs_max` par
/// fenêtre ; bloqué tant que le score atteint `echecs_max`.
struct Penalty {
    score: f64,
    last: Instant,
}

struct ConfigWatch {
    path: PathBuf,
    modified: Option<SystemTime>,
    next_check: Instant,
}

pub struct Server {
    sock: UdpSocket,
    cfg: Arc<Config>,
    annuaire: Annuaire,
    policy: Arc<TunnelPolicy>,
    /// 3615 ORACLE : quotas, cache et client HTTP partagés par ses appels
    oracle: Arc<Oracle>,
    server_key: [u8; 32],
    regs: HashMap<u32, Reg>,
    by_name: HashMap<String, u32>,
    by_number: HashMap<String, u32>,
    calls: HashMap<u32, Call>,
    penalties: HashMap<IpAddr, Penalty>,
    notice_tx: Sender<ModemNotice>,
    notice_rx: Receiver<ModemNotice>,
    modem_threads: Vec<JoinHandle<()>>,
    watch: Option<ConfigWatch>,
    /// Comptes créés par le site d'inscription (ajoutés à l'annuaire)
    comptes: Option<ComptesSource>,
    /// Fichier d'état pour le site (« État du réseau »), réécrit chaque seconde
    etat: Option<(PathBuf, Instant)>,
    /// Tickets de communication et succès (dossier partagé avec le site)
    facturation: Option<Facturation>,
    /// Ce dossier (répondeur, scores des numéros cachés…)
    donnees: Option<PathBuf>,
    /// Succès gagnés dans les numéros cachés, à noter dans les carnets
    exploits: crate::caches::Exploits,
    /// Démarrage (secondes Unix)
    depuis: u64,
    stop: Arc<AtomicBool>,
    stats: Arc<Mutex<Stats>>,
    counters: Stats,
}

/// Serveur lancé dans un thread (tests, intégration).
pub struct ServerHandle {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    stats: Arc<Mutex<Stats>>,
    join: Option<JoinHandle<()>>,
}

impl ServerHandle {
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn stats(&self) -> Stats {
        self.stats.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Server {
    /// Ouvre la socket UDP (adresse `serveur.ecoute` de la configuration).
    pub fn bind(cfg: Config) -> io::Result<Server> {
        let invalid = |e: String| io::Error::new(io::ErrorKind::InvalidInput, e);
        cfg.validate().map_err(invalid)?;
        let addr = cfg.listen_addr().map_err(invalid)?;
        let annuaire = cfg.annuaire().map_err(invalid)?;
        let policy = TunnelPolicy::from_configs(&cfg.fai, &cfg.web1998).map_err(invalid)?;
        let oracle = Oracle::new(&cfg.oracle).map_err(invalid)?;
        let sock = UdpSocket::bind(addr)?;
        sock.set_read_timeout(Some(Duration::from_millis(10)))?;
        if let Some((num, _)) = cfg.services().into_iter().find(|(_, k)| k == "oracle") {
            match oracle.cle() {
                Ok(_) => info!("3615 ORACLE au {num} : modèle {}, clé d'API présente", cfg.oracle.modele),
                Err(e) => info!("3615 ORACLE au {num} : pas de clé d'API ({e}), les cartes répondront"),
            }
        }
        let (notice_tx, notice_rx) = mpsc::channel();
        Ok(Server {
            sock,
            cfg: Arc::new(cfg),
            annuaire,
            policy: Arc::new(policy),
            oracle: Arc::new(oracle),
            server_key: random_bytes(),
            regs: HashMap::new(),
            by_name: HashMap::new(),
            by_number: HashMap::new(),
            calls: HashMap::new(),
            penalties: HashMap::new(),
            notice_tx,
            notice_rx,
            modem_threads: Vec::new(),
            watch: None,
            comptes: None,
            etat: None,
            facturation: None,
            donnees: None,
            exploits: Default::default(),
            depuis: unix_now(),
            stop: Arc::new(AtomicBool::new(false)),
            stats: Arc::new(Mutex::new(Stats::default())),
            counters: Stats::default(),
        })
    }

    /// Surveille le fichier de configuration : quand il change (par exemple
    /// après `v34-server ajouter-abonne`), l'annuaire est rechargé à chaud.
    /// Les autres sections demandent un redémarrage.
    pub fn watch_config(&mut self, path: PathBuf) {
        let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        self.watch = Some(ConfigWatch { path, modified, next_check: Instant::now() + RELOAD_CHECK });
    }

    /// Ajoute à l'annuaire les comptes créés par le site d'inscription (et
    /// suit leurs modifications : validation, accès Internet, suppression).
    pub fn with_comptes(mut self, comptes: Arc<Comptes>) -> Server {
        self.comptes = Some(ComptesSource::Memoire { comptes, generation: 0 });
        self.check_comptes(Instant::now());
        self
    }

    /// Comme [`Server::with_comptes`], depuis le fichier écrit par le site
    /// (`v34-portail`, éventuellement dans un conteneur) : relu quand il
    /// change.
    pub fn with_comptes_file(mut self, path: PathBuf) -> Server {
        self.comptes = Some(ComptesSource::Fichier { path, modified: None, next_check: Instant::now(), liste: Vec::new() });
        self.check_comptes(Instant::now());
        self
    }

    /// Publie l'état du central dans ce fichier JSON (chaque seconde) :
    /// la page « État du réseau » du site le lit.
    /// Tickets de communication et succès dans `dir` (voir [`crate::facturation`]).
    pub fn with_facturation(mut self, dir: PathBuf) -> Server {
        self.facturation = Some(Facturation::ouvrir(dir.clone(), unix_now()));
        // les Tinyblogs des abonnés, servis par le FAI (avant tout appel :
        // la politique n'est pas encore partagée)
        if let Some(p) = Arc::get_mut(&mut self.policy) {
            p.tinyblog = Some(crate::tinyblog::Tinyblog::new(dir.join("tinyblog"), self.exploits.clone()));
        }
        self.donnees = Some(dir);
        self
    }

    pub fn with_status_file(mut self, path: PathBuf) -> Server {
        self.etat = Some((path, Instant::now()));
        self
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.sock.local_addr()
    }

    /// Drapeau d'arrêt (la boucle le consulte toutes les 10 ms).
    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }

    /// Lance la boucle dans un thread.
    pub fn spawn(self) -> io::Result<ServerHandle> {
        let addr = self.local_addr()?;
        let stop = self.stop.clone();
        let stats = self.stats.clone();
        let join = std::thread::Builder::new().name("central".into()).spawn(move || self.run())?;
        Ok(ServerHandle { addr, stop, stats, join: Some(join) })
    }

    /// Boucle principale (bloquante jusqu'à l'arrêt).
    pub fn run(mut self) {
        let mut buf = [0u8; 2048];
        let mut next_hk = Instant::now();
        while !self.stop.load(Ordering::Relaxed) {
            match self.sock.recv_from(&mut buf) {
                Ok((n, from)) => self.on_datagram(&buf[..n], from),
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
                Err(e) if is_transient(&e) => {}
                Err(e) => {
                    error!("socket UDP : {e}");
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            let now = Instant::now();
            if now >= next_hk {
                self.housekeeping(now);
                next_hk = now + HOUSEKEEPING;
            }
        }
        info!("arrêt du central");
        let ids: Vec<u32> = self.calls.keys().copied().collect();
        for id in ids {
            self.end_call(id, None, HangupReason::Shutdown);
        }
        for h in self.modem_threads.drain(..) {
            let _ = h.join();
        }
    }

    fn send_to(&self, bytes: &[u8], addr: SocketAddr) {
        if let Err(e) = self.sock.send_to(bytes, addr) {
            if !is_transient(&e) {
                debug!("envoi vers {addr} : {e}");
            }
        }
    }

    fn on_datagram(&mut self, buf: &[u8], from: SocketAddr) {
        if buf.len() >= 2 && buf[0..2] == packet::MAGIC {
            self.on_audio(buf, from);
        } else if signal::is_control(buf) {
            self.on_control(buf, from);
        }
        // tout le reste : ignoré sans réponse
    }

    // ───────────────────────── authentification ─────────────────────────

    /// Défi sans état : horodatage + HMAC(clé serveur, horodatage, adresse, nom).
    fn make_challenge(&self, from: SocketAddr, name: &str, now: u64) -> [u8; SERVER_NONCE_LEN] {
        let mut n = [0u8; SERVER_NONCE_LEN];
        n[..8].copy_from_slice(&now.to_be_bytes());
        let mac = hmac_sha256(&self.server_key, &[b"V34-CHAL\0", &n[..8], from.to_string().as_bytes(), b"\0", name.as_bytes()]);
        n[8..].copy_from_slice(&mac[..SERVER_NONCE_LEN - 8]);
        n
    }

    fn challenge_valid(&self, nonce: &[u8; SERVER_NONCE_LEN], from: SocketAddr, name: &str) -> bool {
        let ts = u64::from_be_bytes(nonce[..8].try_into().expect("8 octets"));
        let now = unix_now();
        if ts > now + 5 || now.saturating_sub(ts) > CHALLENGE_TTL {
            return false;
        }
        ct_eq(&self.make_challenge(from, name, ts), nonce)
    }

    /// Sel et itérations à annoncer pour `name`. Pour un nom inconnu, un sel
    /// factice mais stable : la réponse ne révèle pas qui est abonné.
    fn salt_for(&self, name: &str) -> (Vec<u8>, u32) {
        match self.annuaire.get(name) {
            Some((_, h)) => (h.salt.clone(), h.iterations),
            None => {
                let fake = hmac_sha256(&self.server_key, &[b"V34-FAKE-SALT\0", name.as_bytes()]);
                (fake[..auth::SALT_LEN].to_vec(), DEFAULT_ITERATIONS)
            }
        }
    }

    fn penalty_score(&self, ip: IpAddr, now: Instant) -> f64 {
        let c = &self.cfg.central;
        let decay = c.echecs_max as f64 / c.echecs_fenetre_s as f64;
        self.penalties.get(&ip).map(|p| (p.score - now.duration_since(p.last).as_secs_f64() * decay).max(0.0)).unwrap_or(0.0)
    }

    fn is_blocked(&self, ip: IpAddr, now: Instant) -> bool {
        self.penalty_score(ip, now) >= self.cfg.central.echecs_max as f64
    }

    fn record_failure(&mut self, ip: IpAddr, now: Instant) {
        self.counters.auth_failures += 1;
        let score = self.penalty_score(ip, now) + 1.0;
        if self.penalties.len() >= MAX_TRACKED_IPS {
            self.penalties.clear(); // borne mémoire (attaque massive)
        }
        self.penalties.insert(ip, Penalty { score, last: now });
        if score >= self.cfg.central.echecs_max as f64 {
            warn!("trop d'échecs d'authentification depuis {ip} : bloqué temporairement");
        }
    }

    fn auth_fail(&self, to: SocketAddr, reason: AuthFailReason) {
        let f = Frame { session: 0, msg_id: 0, msg: Msg::AuthFail { reason } };
        self.send_to(&signal::encode(&f, None), to);
    }

    fn on_control(&mut self, buf: &[u8], from: SocketAddr) {
        let Some(session) = signal::peek_session(buf) else { return };
        if session == 0 {
            let now = Instant::now();
            match signal::decode(buf, None) {
                Ok(Frame { msg: Msg::Register { name }, .. }) => {
                    if self.is_blocked(from.ip(), now) {
                        return self.auth_fail(from, AuthFailReason::TooManyAttempts);
                    }
                    let nonce = self.make_challenge(from, &name, unix_now());
                    let (salt, iterations) = self.salt_for(&name);
                    let f = Frame { session: 0, msg_id: 0, msg: Msg::Challenge { nonce, iterations, salt } };
                    self.send_to(&signal::encode(&f, None), from);
                }
                Ok(Frame { msg: Msg::Auth { name, server_nonce, client_nonce, proof }, .. }) => {
                    if self.is_blocked(from.ip(), now) {
                        return self.auth_fail(from, AuthFailReason::TooManyAttempts);
                    }
                    self.on_auth(from, name, server_nonce, client_nonce, proof);
                }
                _ => {}
            }
            return;
        }
        let now = Instant::now();
        let Some(reg) = self.regs.get_mut(&session) else { return };
        let Ok(f) = signal::decode(buf, Some(&reg.key)) else {
            debug!("message de contrôle mal authentifié depuis {from}");
            return;
        };
        reg.last_seen = now;
        if reg.addr != from {
            info!("{} : nouvelle adresse {from} (était {})", reg.name, reg.addr);
            reg.addr = from;
        }
        if let Msg::Ack { msg_id } = f.msg {
            reg.outbox.ack(msg_id);
            return;
        }
        if matches!(f.msg, Msg::DirectoryReq) {
            if reg.last_directory.is_some_and(|t| now - t < Duration::from_millis(900)) {
                return;
            }
            reg.last_directory = Some(now);
            let (key, addr) = (reg.key, reg.addr);
            let msg = Msg::Directory { entries: self.directory() };
            let bytes = signal::encode(&Frame { session, msg_id: 0, msg }, Some(&key));
            self.send_to(&bytes, addr);
            self.send_compte(session);
            return;
        }
        if let Msg::Exploits { ids } = &f.msg {
            let nom = reg.name.clone();
            if self.facturation.as_mut().is_some_and(|fa| fa.exploits(&nom, ids, unix_now())) {
                info!("{nom} : succès {}", ids.join(", "));
                self.send_compte(session);
            }
            return;
        }
        if !f.msg.is_reliable() {
            return;
        }
        let ack = signal::encode(&Frame { session, msg_id: 0, msg: Msg::Ack { msg_id: f.msg_id } }, Some(&reg.key));
        let seen = reg.dedup.check(f.msg_id);
        self.send_to(&ack, from);
        if seen == Seen::New {
            self.on_message(session, f.msg);
        }
    }

    fn on_auth(
        &mut self,
        from: SocketAddr,
        name: String,
        server_nonce: [u8; SERVER_NONCE_LEN],
        client_nonce: [u8; CLIENT_NONCE_LEN],
        proof: [u8; 32],
    ) {
        // Défi périmé (serveur redémarré…) : silence, le client redemandera un défi.
        if !self.challenge_valid(&server_nonce, from, &name) {
            debug!("AUTH de {from} : défi invalide ou expiré");
            return;
        }
        let checked = self.annuaire.get(&name).and_then(|(number, h)| {
            auth::verify_proof(&h.stored_key, &name, &server_nonce, &client_nonce, &proof).map(|k| (number.clone(), k))
        });
        let Some((number, key)) = checked else {
            self.record_failure(from.ip(), Instant::now());
            info!("authentification refusée pour « {name} » depuis {from}");
            return self.auth_fail(from, AuthFailReason::BadCredentials);
        };
        // AUTH répété (WELCOME perdu) : même session, on renvoie WELCOME.
        if let Some(&s) = self.by_name.get(&name) {
            if self.regs.get(&s).is_some_and(|r| r.client_nonce == client_nonce) {
                let reg = self.regs.get_mut(&s).expect("session présente");
                reg.addr = from;
                reg.last_seen = Instant::now();
                self.send_welcome(s);
                return;
            }
            self.remove_registration(s, "remplacé par une nouvelle connexion");
        }
        if self.regs.len() >= self.cfg.central.connectes_max {
            warn!("connexion de {name} refusée : limite de {} abonnés connectés", self.cfg.central.connectes_max);
            return self.auth_fail(from, AuthFailReason::ServerFull);
        }
        let session = loop {
            let s = random_u32_nonzero();
            if !self.regs.contains_key(&s) {
                break s;
            }
        };
        self.regs.insert(
            session,
            Reg {
                name: name.clone(),
                number: number.clone(),
                key,
                client_nonce,
                addr: from,
                last_seen: Instant::now(),
                outbox: Outbox::new(),
                dedup: Dedup::default(),
                call: None,
                last_directory: None,
            },
        );
        self.by_name.insert(name.clone(), session);
        self.by_number.insert(number.clone(), session);
        info!("{name} enregistré au {number} depuis {from}");
        if let Some(f) = self.facturation.as_mut() {
            f.connexion(&name, unix_now());
        }
        self.send_welcome(session);
    }

    /// Facture du mois et succès (sans acquittement : repart avec chaque
    /// annuaire ; les clients d'avant le 0.1.7 l'ignorent).
    fn send_compte(&mut self, session: u32) {
        let (Some(f), Some(reg)) = (self.facturation.as_mut(), self.regs.get(&session)) else { return };
        let r = f.releve(&reg.name, unix_now());
        let msg = Msg::Compte { mois: r.mois, centimes: r.centimes, secondes: r.secondes, succes: r.succes };
        let b = signal::encode(&Frame { session, msg_id: 0, msg }, Some(&reg.key));
        self.send_to(&b, reg.addr);
    }

    fn send_welcome(&self, session: u32) {
        let Some(reg) = self.regs.get(&session) else { return };
        let msg = Msg::Welcome {
            number: reg.number.clone(),
            keepalive_s: self.cfg.central.keepalive_s,
            expiry_s: self.cfg.central.expiration_s,
            services: self.cfg.services(),
        };
        let b = signal::encode(&Frame { session, msg_id: 0, msg }, Some(&reg.key));
        self.send_to(&b, reg.addr);
    }

    /// Annuaire : les abonnés de la configuration et leur présence.
    fn directory(&self) -> Vec<DirEntry> {
        self.annuaire
            .entrees()
            .into_iter()
            .map(|(nom, numero)| {
                let status = match self.by_number.get(numero).and_then(|s| self.regs.get(s)) {
                    Some(r) if r.call.is_some() => Presence::Busy,
                    Some(_) => Presence::Online,
                    None => Presence::Absent,
                };
                DirEntry { name: nom.to_string(), number: numero.to_string(), status }
            })
            .collect()
    }

    fn send_reliable(&mut self, session: u32, msg: Msg) {
        let Some(reg) = self.regs.get_mut(&session) else { return };
        let msg_id = reg.outbox.alloc_id();
        let bytes = signal::encode(&Frame { session, msg_id, msg }, Some(&reg.key));
        reg.outbox.track(msg_id, bytes.clone(), Instant::now());
        let addr = reg.addr;
        self.send_to(&bytes, addr);
    }

    fn remove_registration(&mut self, session: u32, why: &str) {
        let Some(reg) = self.regs.remove(&session) else { return };
        if self.by_name.get(&reg.name) == Some(&session) {
            self.by_name.remove(&reg.name);
        }
        if self.by_number.get(&reg.number) == Some(&session) {
            self.by_number.remove(&reg.number);
        }
        if let Some(c) = reg.call {
            self.end_call(c, Some(session), HangupReason::PeerLost);
        }
        info!("{} ({}) déconnecté : {why}", reg.name, reg.number);
    }

    // ───────────────────────── appels ─────────────────────────

    fn on_message(&mut self, session: u32, msg: Msg) {
        match msg {
            Msg::Keepalive => {}
            Msg::Unregister => self.remove_registration(session, "désinscription"),
            Msg::Dial { number } => self.on_dial(session, number),
            Msg::Answer { call_id } => self.on_answer(session, call_id),
            Msg::Hangup { call_id, .. } => {
                if self.calls.get(&call_id).is_some_and(|c| c.caller == session || matches!(c.callee, Callee::Client(s) if s == session)) {
                    self.end_call(call_id, Some(session), HangupReason::Normal);
                }
            }
            other => debug!("message inattendu d'un abonné : {}", other.name()),
        }
    }

    fn new_call_id(&self) -> u32 {
        loop {
            let id = random_u32_nonzero();
            if !self.calls.contains_key(&id) {
                return id;
            }
        }
    }

    fn line_sims(&self) -> Option<Box<[LineSim; 2]>> {
        self.cfg.line_params().ok().flatten().map(|p| Box::new([LineSim::new(p.clone()), LineSim::new(p)]))
    }

    fn modem_calls(&self) -> usize {
        self.calls.values().filter(|c| matches!(c.callee, Callee::Modem { .. })).count()
    }

    fn on_dial(&mut self, session: u32, number: String) {
        let Some(reg) = self.regs.get(&session) else { return };
        let caller = format!("{} ({})", reg.name, reg.number);
        if reg.call.is_some() || self.calls.len() >= self.cfg.central.appels_max {
            return self.send_reliable(session, Msg::Busy { number });
        }
        let now = Instant::now();
        if let Some(service) = self.cfg.service_for(&number) {
            let refus = match service {
                Service::Fai => !self.annuaire.fai_autorise(&reg.name),
                Service::Oracle => !self.oracle.autorise(&self.annuaire, &reg.name),
                Service::Echo | Service::Cache(_) => false,
            };
            if refus {
                // Appel aussitôt terminé : « accès non autorisé ». Les anciens
                // clients y voient un échec ordinaire.
                info!("{caller} → {number} ({}) : accès Internet non activé pour ce compte", service.name());
                let id = self.new_call_id();
                self.send_reliable(session, Msg::Ringing { call_id: id });
                return self.send_reliable(session, Msg::Hangup { call_id: id, reason: HangupReason::Forbidden });
            }
            let n = self.modem_calls();
            if n >= self.cfg.modems.appels_max {
                info!("{caller} → {number} ({}) : occupé ({n} modems en ligne)", service.name());
                return self.send_reliable(session, Msg::Busy { number });
            }
            let id = self.new_call_id();
            let answer_at = now + Duration::from_millis(self.cfg.modems.delai_decroche_ms);
            let sims = self.line_sims();
            let callee = Callee::Modem { service, answer_at, tx: None };
            let appelant = (reg.name.clone(), reg.number.clone());
            let appele = service.name().to_string();
            self.calls.insert(id, Call { caller: session, callee, state: CallState::Ringing, created: now, sims, appelant, numero: number.clone(), appele, debut: None });
            if let Some(r) = self.regs.get_mut(&session) {
                r.call = Some(id);
            }
            info!("{caller} appelle le {number} ({}, appel {id:08x})", service.name());
            return self.send_reliable(session, Msg::Ringing { call_id: id });
        }
        let Some(&callee) = self.by_number.get(&number) else {
            info!("{caller} → {number} : numéro inconnu ou abonné absent");
            return self.send_reliable(session, Msg::UnknownNumber { number });
        };
        if callee == session || self.regs.get(&callee).is_some_and(|r| r.call.is_some()) {
            info!("{caller} → {number} : occupé");
            return self.send_reliable(session, Msg::Busy { number });
        }
        let id = self.new_call_id();
        let sims = self.line_sims();
        let appelant = self.regs.get(&session).map(|r| (r.name.clone(), r.number.clone())).unwrap_or_default();
        let appele = self.regs.get(&callee).map(|r| r.name.clone()).unwrap_or_default();
        self.calls.insert(id, Call { caller: session, callee: Callee::Client(callee), state: CallState::Ringing, created: now, sims, appelant, numero: number.clone(), appele, debut: None });
        let caller_number = self.regs.get(&session).map(|r| r.number.clone()).unwrap_or_default();
        for s in [session, callee] {
            if let Some(r) = self.regs.get_mut(&s) {
                r.call = Some(id);
            }
        }
        info!("{caller} appelle le {number} (appel {id:08x})");
        self.send_reliable(callee, Msg::Ring { call_id: id, from: caller_number });
        self.send_reliable(session, Msg::Ringing { call_id: id });
    }

    fn on_answer(&mut self, session: u32, call_id: u32) {
        let Some(call) = self.calls.get_mut(&call_id) else { return };
        if !matches!(call.callee, Callee::Client(s) if s == session) || call.state != CallState::Ringing {
            return;
        }
        call.state = CallState::Active;
        call.debut = Some((Instant::now(), unix_now()));
        let caller = call.caller;
        info!("appel {call_id:08x} : décroché");
        self.send_reliable(caller, Msg::Answer { call_id });
    }

    /// Termine un appel ; prévient toutes les parties sauf `by`.
    fn end_call(&mut self, call_id: u32, by: Option<u32>, reason: HangupReason) {
        let Some(call) = self.calls.remove(&call_id) else { return };
        let service = match &call.callee {
            Callee::Modem { service, .. } => Some(service.name()),
            Callee::Client(_) => None,
        };
        let mut parties = vec![call.caller];
        match call.callee {
            Callee::Client(s) => parties.push(s),
            Callee::Modem { tx: Some(tx), .. } => {
                let _ = tx.send(ModemInput::Hangup);
            }
            Callee::Modem { tx: None, .. } => {}
        }
        for s in parties {
            if let Some(r) = self.regs.get_mut(&s) {
                if r.call == Some(call_id) {
                    r.call = None;
                }
            }
            if Some(s) != by {
                self.send_reliable(s, Msg::Hangup { call_id, reason });
            }
        }
        info!("appel {call_id:08x} terminé ({})", reason.describe());
        // Ticket de communication (appel abouti seulement) ; seul l'appelant paie
        if let (Some((t0, debut)), Some(f)) = (call.debut, self.facturation.as_mut()) {
            let (nom, num) = (&call.appelant.0, &call.appelant.1);
            let t = Ticket::nouveau(debut, t0.elapsed().as_secs() as u32, (nom, num), &call.numero, &call.appele, service);
            info!("ticket : {nom} → {} ({}), {} s, {}", call.numero, call.appele, t.duree_s, v34_net::tarif::francs(t.centimes as u64));
            for n in f.appel(t) {
                if let Some(&s) = self.by_name.get(&n) {
                    self.send_compte(s);
                }
            }
        }
    }

    fn start_modem(&mut self, call_id: u32) {
        let Some(call) = self.calls.get(&call_id) else { return };
        let Callee::Modem { service, .. } = call.callee else { return };
        let Some(reg) = self.regs.get(&call.caller) else { return };
        let (tx, rx) = mpsc::channel();
        let sock = match self.sock.try_clone() {
            Ok(s) => s,
            Err(e) => {
                error!("modem : clonage de la socket impossible : {e}");
                return self.end_call(call_id, None, HangupReason::Failure);
            }
        };
        let modem_call = ModemCall {
            call_id,
            service,
            caller: reg.name.clone(),
            caller_number: reg.number.clone(),
            peer: reg.addr,
            sock,
            max_rate: self.cfg.modems.debit_max,
            v90: self.cfg.modems.v90,
            max_rate_pcm: self.cfg.modems.debit_max_56k,
            carrier_timeout: Duration::from_secs(self.cfg.modems.delai_porteuse_s.max(10)),
            line: self.cfg.line_params().ok().flatten(),
            policy: self.policy.clone(),
            oracle: self.oracle.clone(),
            donnees: self.donnees.clone(),
            exploits: self.exploits.clone(),
        };
        match services::spawn(modem_call, rx, self.notice_tx.clone()) {
            Ok(h) => {
                self.modem_threads.push(h);
                let call = self.calls.get_mut(&call_id).expect("appel présent");
                if let Callee::Modem { tx: slot, .. } = &mut call.callee {
                    *slot = Some(tx);
                }
                call.state = CallState::Active;
                call.debut = Some((Instant::now(), unix_now()));
                let caller = call.caller;
                self.send_reliable(caller, Msg::Answer { call_id });
            }
            Err(e) => {
                error!("impossible de lancer le modem : {e}");
                self.end_call(call_id, None, HangupReason::Failure);
            }
        }
    }

    // ───────────────────────── audio ─────────────────────────

    fn on_audio(&mut self, buf: &[u8], from: SocketAddr) {
        if buf.len() < packet::HEADER_LEN || buf.len() > MAX_AUDIO {
            self.counters.dropped_packets += 1;
            return;
        }
        let call_id = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
        let Some(call) = self.calls.get_mut(&call_id) else {
            self.counters.dropped_packets += 1;
            return;
        };
        if call.state != CallState::Active {
            self.counters.dropped_packets += 1;
            return;
        }
        let addr_of = |s: u32| self.regs.get(&s).map(|r| r.addr);
        let caller_addr = addr_of(call.caller);
        let callee_addr = match call.callee {
            Callee::Client(s) => addr_of(s),
            Callee::Modem { .. } => None,
        };
        // sens 0 : appelant → appelé ; sens 1 : appelé → appelant
        let dir = if caller_addr == Some(from) {
            0
        } else if callee_addr == Some(from) {
            1
        } else {
            self.counters.dropped_packets += 1;
            return;
        };
        let to_modem = dir == 0 && matches!(call.callee, Callee::Modem { .. });
        let need_decode = to_modem || call.sims.is_some();
        let mut pkt = if need_decode {
            match Packet::decode(buf) {
                Some(p) => Some(p),
                None => {
                    self.counters.dropped_packets += 1;
                    return;
                }
            }
        } else {
            None
        };
        if let (Some(sims), Some(p)) = (call.sims.as_mut(), pkt.as_mut()) {
            let input = std::mem::take(&mut p.samples);
            let mut output = vec![0.0f32; input.len()];
            sims[dir].process(&input, &mut output);
            p.samples = output;
            // les copies des trames précédentes n'ont pas traversé le
            // simulateur (qui a une mémoire) : on ne les transmet pas
            p.backup.clear();
        }
        self.counters.relayed_packets += 1;
        if to_modem {
            if let (Callee::Modem { tx: Some(tx), .. }, Some(p)) = (&call.callee, pkt) {
                let _ = tx.send(ModemInput::Audio { pkt: p, from });
            }
            return;
        }
        let dest = if dir == 0 { callee_addr } else { caller_addr };
        let Some(dest) = dest else { return };
        match pkt {
            Some(p) => self.send_to(&p.encode(), dest),
            None => self.send_to(buf, dest),
        }
    }

    // ───────────────────────── temporisateurs ─────────────────────────

    fn housekeeping(&mut self, now: Instant) {
        // succès gagnés dans les numéros cachés
        let gagnes: Vec<(String, &'static str)> = std::mem::take(&mut *self.exploits.lock().unwrap_or_else(|e| e.into_inner()));
        for (nom, id) in gagnes {
            if self.facturation.as_mut().is_some_and(|f| f.serveur(&nom, id, unix_now())) {
                info!("{nom} : succès {id}");
                if let Some(&s) = self.by_name.get(&nom) {
                    self.send_compte(s);
                }
            }
        }
        // retransmissions et expirations
        let expiry = Duration::from_secs(self.cfg.central.expiration_s as u64);
        let mut lost = Vec::new();
        let mut resend = Vec::new();
        for (&s, reg) in self.regs.iter_mut() {
            let p = reg.outbox.poll(now);
            resend.extend(p.resend.into_iter().map(|b| (b, reg.addr)));
            if !p.failed.is_empty() {
                lost.push((s, "injoignable"));
            } else if now.duration_since(reg.last_seen) > expiry {
                lost.push((s, "keepalive expiré"));
            }
        }
        for (b, addr) in resend {
            self.send_to(&b, addr);
        }
        for (s, why) in lost {
            self.remove_registration(s, why);
        }
        // sonneries trop longues, décroché des modems
        let ring_timeout = self.cfg.ring_timeout();
        let mut no_answer = Vec::new();
        let mut modem_answer = Vec::new();
        for (&id, c) in &self.calls {
            if c.state != CallState::Ringing {
                continue;
            }
            match c.callee {
                Callee::Modem { answer_at, .. } if now >= answer_at => modem_answer.push(id),
                Callee::Client(_) if now.duration_since(c.created) > ring_timeout => no_answer.push(id),
                _ => {}
            }
        }
        for id in no_answer {
            self.end_call(id, None, HangupReason::NoAnswer);
        }
        for id in modem_answer {
            self.start_modem(id);
        }
        // fins d'appels des modems
        while let Ok(ModemNotice::Ended { call_id, reason, detail }) = self.notice_rx.try_recv() {
            if self.calls.contains_key(&call_id) {
                debug!("appel {call_id:08x} : le modem raccroche ({detail})");
                self.end_call(call_id, None, reason);
            }
        }
        self.modem_threads.retain(|h| !h.is_finished());
        // limitation des échecs : oubli des adresses revenues à zéro
        let c = &self.cfg.central;
        let decay = c.echecs_max as f64 / c.echecs_fenetre_s as f64;
        self.penalties.retain(|_, p| p.score - now.duration_since(p.last).as_secs_f64() * decay > 0.0);
        self.check_reload(now);
        self.check_comptes(now);
        self.write_status(now);
        // statistiques
        self.counters.registrations = self.regs.len();
        self.counters.calls = self.calls.len();
        self.counters.modem_calls = self.modem_calls();
        if let Ok(mut s) = self.stats.lock() {
            *s = self.counters.clone();
        }
    }

    /// Recharge l'annuaire si le fichier de configuration a changé.
    fn check_reload(&mut self, now: Instant) {
        let Some(w) = self.watch.as_mut() else { return };
        if now < w.next_check {
            return;
        }
        w.next_check = now + RELOAD_CHECK;
        let modified = std::fs::metadata(&w.path).and_then(|m| m.modified()).ok();
        if modified == w.modified {
            return;
        }
        w.modified = modified;
        let path = w.path.clone();
        let new = match Config::load(&path).and_then(|c| c.annuaire().map(|a| (c, a))) {
            Ok(x) => x,
            Err(e) => {
                warn!("configuration modifiée mais invalide, annuaire conservé : {e}");
                return;
            }
        };
        let (cfg, annuaire) = new;
        let mut c = (*self.cfg).clone();
        c.abonnes = cfg.abonnes;
        self.cfg = Arc::new(c);
        self.install_annuaire(annuaire);
    }

    /// Les comptes du site ont changé : annuaire reconstruit.
    fn check_comptes(&mut self, now: Instant) {
        let Some(src) = self.comptes.as_mut() else { return };
        if !src.refresh(now) {
            return;
        }
        match self.cfg.annuaire() {
            Ok(a) => self.install_annuaire(a),
            Err(e) => warn!("annuaire de la configuration invalide : {e}"),
        }
    }

    /// Installe un nouvel annuaire (celui de la configuration, auquel on
    /// ajoute les comptes du site).
    fn install_annuaire(&mut self, mut annuaire: Annuaire) {
        if let Some(c) = &self.comptes {
            for w in annuaire.ajouter_comptes(&c.liste()) {
                warn!("{w}");
            }
        }
        // Abonnés retirés ou dont le numéro a changé : déconnectés (ils se
        // réenregistreront et recevront leur nouveau numéro).
        let stale: Vec<u32> = self
            .regs
            .iter()
            .filter(|(_, r)| annuaire.get(&r.name).is_none_or(|(n, _)| *n != r.number))
            .map(|(&s, _)| s)
            .collect();
        for s in stale {
            self.remove_registration(s, "retiré ou renuméroté dans l'annuaire");
        }
        // Accès Internet retiré pendant un appel au FAI (ou à l'oracle, s'il
        // lui est réservé) : on raccroche.
        let oracle_fai = self.oracle.reserve_au_fai();
        let revoked: Vec<u32> = self
            .calls
            .iter()
            .filter(|(_, c)| match c.callee {
                Callee::Modem { service: Service::Fai, .. } => true,
                Callee::Modem { service: Service::Oracle, .. } => oracle_fai,
                _ => false,
            })
            .filter(|(_, c)| self.regs.get(&c.caller).is_some_and(|r| !annuaire.fai_autorise(&r.name)))
            .map(|(&id, _)| id)
            .collect();
        for id in revoked {
            self.end_call(id, None, HangupReason::Forbidden);
        }
        self.annuaire = annuaire;
        info!("annuaire rechargé : {} abonné(s)", self.annuaire.len());
    }
}

impl Server {
    fn write_status(&mut self, now: Instant) {
        let Some((path, next)) = self.etat.as_mut() else { return };
        if now < *next {
            return;
        }
        *next = now + Duration::from_secs(1);
        let path = path.clone();
        let etat = serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "depuis": self.depuis,
            "maj": unix_now(),
            "abonnes": self.annuaire.len(),
            "connectes": self.regs.len(),
            "appels": self.calls.len(),
            "modems": { "occupes": self.modem_calls(), "max": self.cfg.modems.appels_max },
            "salon": self.policy.salon.as_ref().map_or(0, |s| s.present().len()),
        });
        let tmp = path.with_extension("json.tmp");
        let res = std::fs::write(&tmp, etat.to_string()).and_then(|()| std::fs::rename(&tmp, &path));
        if let Err(e) = res {
            // une fois par minute au plus dans le journal
            if let Some((_, next)) = self.etat.as_mut() {
                *next = now + Duration::from_secs(60);
            }
            warn!("état du central non publié dans {} : {e}", path.display());
        }
    }
}

/// D'où viennent les comptes du site d'inscription.
enum ComptesSource {
    /// Dans le même processus (tests)
    Memoire { comptes: Arc<Comptes>, generation: u64 },
    /// Fichier écrit par `v34-portail`, relu quand il change
    Fichier { path: PathBuf, modified: Option<SystemTime>, next_check: Instant, liste: Vec<crate::comptes::Compte> },
}

impl ComptesSource {
    /// Vrai si les comptes ont changé depuis la dernière fois.
    fn refresh(&mut self, now: Instant) -> bool {
        match self {
            ComptesSource::Memoire { comptes, generation } => {
                let g = comptes.generation();
                std::mem::replace(generation, g) != g
            }
            ComptesSource::Fichier { path, modified, next_check, liste } => {
                if now < *next_check {
                    return false;
                }
                *next_check = now + RELOAD_CHECK;
                let m = std::fs::metadata(&*path).and_then(|m| m.modified()).ok();
                if m == *modified && m.is_some() {
                    return false;
                }
                match crate::comptes::lire_comptes(path) {
                    Ok(l) => {
                        *modified = m;
                        let changed = *liste != l;
                        *liste = l;
                        changed || m.is_none()
                    }
                    Err(e) => {
                        // fichier en cours d'écriture ou abîmé : on garde l'ancien
                        warn!("comptes du site illisibles, annuaire conservé : {e}");
                        false
                    }
                }
            }
        }
    }

    fn liste(&self) -> Vec<crate::comptes::Compte> {
        match self {
            ComptesSource::Memoire { comptes, .. } => comptes.liste(),
            ComptesSource::Fichier { liste, .. } => liste.clone(),
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn is_transient(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionRefused | io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    )
}
