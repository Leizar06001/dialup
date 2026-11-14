//! Client du central téléphonique (côté PC).
//!
//! On s'enregistre avec un NOM d'abonné et son mot de passe (qui ne circule
//! jamais : voir [`crate::auth`]) ; le central attribue le numéro.
//!
//! Une seule socket UDP sert à la signalisation (enregistrement, keepalive,
//! appels — voir [`crate::signal`]) et à l'audio de l'appel en cours (paquets
//! [`crate::packet`] portant l'identifiant d'appel attribué par le central).
//! Le client émet toujours en premier et vers le seul serveur : il
//! fonctionne derrière un NAT (le keepalive maintient l'association).
//!
//! Tout est piloté par [`CentralClient::poll`] (non bloquant), appelé
//! automatiquement par `recv_block` : on l'utilise exactement comme un
//! [`crate::AudioLink`], en cadençant le modem toutes les 20 ms.
//!
//! ```no_run
//! use v34_net::central::{CentralClient, CentralConfig, CentralEvent};
//! let cfg = CentralConfig::new("dialup.tinyverse.fr:3434", "gaston", "mot de passe");
//! let mut c = CentralClient::connect(cfg).unwrap();
//! let mut rx = [0.0f32; 160];
//! loop {
//!     c.recv_block(&mut rx); // + poll de la signalisation
//!     while let Some(ev) = c.next_event() {
//!         if let CentralEvent::Registered { number, .. } = ev {
//!             println!("mon numéro : {number}");
//!             c.dial("0836154286").unwrap();
//!         }
//!     }
//!     c.send_block(&[0.0; 160]).unwrap();
//!     std::thread::sleep(std::time::Duration::from_millis(20));
//! }
//! ```

use std::collections::VecDeque;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

use crate::auth;
use crate::crypto::random_bytes;
use crate::jitter::{Concealment, FrameKind, JitterBuffer};
use crate::link::NetStats;
use crate::packet::{Codec, Packet, FRAME_SAMPLES};
use crate::port::AudioPort;
use crate::signal::{
    self, AuthFailReason, Dedup, DirEntry, Frame, HangupReason, Msg, Outbox, Seen, SessionKey, CLIENT_NONCE_LEN, SERVER_NONCE_LEN,
};

/// Intervalle de répétition de REGISTER / AUTH sans réponse.
const REGISTER_RETRY: Duration = Duration::from_secs(1);
/// AUTH répétés avant de redemander un défi.
const AUTH_TRIES: u32 = 5;
/// Keepalive par défaut si le serveur n'en annonce pas.
const DEFAULT_KEEPALIVE: Duration = Duration::from_secs(15);
/// Rafraîchissement de l'annuaire (présence des abonnés)
const DIRECTORY_EVERY: Duration = Duration::from_secs(10);
/// Écart minimal entre deux demandes d'annuaire
const DIRECTORY_MIN_GAP: Duration = Duration::from_secs(1);
/// Attente après « trop de tentatives » avant de réessayer.
const RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(60);
/// Serveur par défaut.
pub const DEFAULT_SERVER: &str = "dialup.tinyverse.fr:3434";

#[derive(Clone, Debug, PartialEq)]
pub struct CentralConfig {
    /// Adresse du central (« hote:port »)
    pub server: String,
    /// Nom d'abonné (le numéro est attribué par le central)
    pub user: String,
    /// Mot de passe (jamais transmis)
    pub password: String,
    /// Port UDP local (0 = au hasard)
    pub local_port: u16,
    pub codec: Codec,
    pub jitter_target_ms: u32,
    pub concealment: Concealment,
}

impl CentralConfig {
    pub fn new(server: impl Into<String>, user: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            server: server.into(),
            user: user.into(),
            password: password.into(),
            local_port: 0,
            codec: Codec::Alaw,
            // Modem : tampon profond et sans glissement (le Wi-Fi a facilement
            // 50 ms de gigue ; un glissement de 20 ms fait dérailler le modem).
            jitter_target_ms: 140,
            concealment: Concealment::Modem,
        }
    }
}

/// Événements de signalisation (à vider avec [`CentralClient::next_event`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CentralEvent {
    /// Enregistré et authentifié : `number` est le numéro attribué,
    /// `services` les numéros spéciaux du central (numéro, service).
    Registered { number: String, services: Vec<(String, String)> },
    /// Enregistrement refusé. Après `BadCredentials` le client abandonne
    /// (inutile d'insister) ; après `TooManyAttempts` il réessaie dans 1 min.
    AuthFailed { reason: AuthFailReason },
    /// Plus de nouvelles du central : ré-enregistrement automatique en cours
    ServerLost,
    /// Appel entrant (pas encore décroché : voir [`CentralClient::answer`])
    Incoming { call_id: u32, from: String },
    /// Notre appel sonne chez le correspondant
    Ringing { call_id: u32 },
    /// Le correspondant a décroché : l'audio passe
    Answered { call_id: u32 },
    Busy { number: String },
    UnknownNumber { number: String },
    /// Fin de l'appel décidée par l'autre côté ou par le central
    HungUp { call_id: u32, reason: HangupReason },
}

/// État de l'appel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallState {
    Idle,
    /// DIAL envoyé, en attente de RINGING / BUSY / …
    Dialing { number: String },
    /// Ça sonne chez l'appelé
    Ringing { call_id: u32 },
    /// Appel entrant pas encore décroché
    Incoming { call_id: u32, from: String },
    /// Communication établie
    Active { call_id: u32 },
}

impl CallState {
    pub fn call_id(&self) -> Option<u32> {
        match self {
            CallState::Ringing { call_id } | CallState::Incoming { call_id, .. } | CallState::Active { call_id } => {
                Some(*call_id)
            }
            _ => None,
        }
    }
}

enum Reg {
    /// Envoi périodique de REGISTER
    Unregistered { next_at: Instant },
    /// AUTH envoyé, attente de WELCOME
    Authenticating { key: SessionKey, auth: Vec<u8>, next_at: Instant, tries: u32 },
    Registered { session: u32, key: SessionKey },
    /// Refusé : on ne réessaie pas (ou pas avant `until`)
    Refused { until: Option<Instant> },
}

pub struct CentralClient {
    sock: UdpSocket,
    server: SocketAddr,
    cfg: CentralConfig,
    reg: Reg,
    outbox: Outbox,
    dedup: Dedup,
    keepalive_every: Duration,
    next_keepalive: Instant,
    /// Mot de passe salé pour (sel, itérations) : PBKDF2 est lent, on le garde
    salted: Option<(Vec<u8>, u32, [u8; 32])>,
    number: Option<String>,
    services: Vec<(String, String)>,
    /// Annuaire du central (redemandé toutes les DIRECTORY_EVERY)
    directory: Vec<DirEntry>,
    next_directory: Instant,
    last_directory_req: Option<Instant>,
    /// Demande en attente de réponse (mesure de l'aller-retour)
    rtt_probe: Option<Instant>,
    /// Aller-retour vers le central (ms), mesuré sur l'annuaire
    rtt_ms: Option<f32>,
    call: CallState,
    events: VecDeque<CentralEvent>,
    // audio de l'appel
    tx_seq: u32,
    tx_ts: u32,
    tx_acc: Vec<f32>,
    /// Dernières trames émises, répétées dans les paquets suivants
    tx_hist: std::collections::VecDeque<Vec<f32>>,
    /// Essais : V34_PERTES=0.02 jette 2 % des paquets audio, dans les deux sens
    pertes: (f64, u64),
    jb: JitterBuffer,
    rx_fifo: VecDeque<(f32, bool)>,
    epoch: Instant,
    stats: NetStats,
    got_audio: bool,
}

impl CentralClient {
    /// Ouvre la socket et commence l'enregistrement (non bloquant).
    pub fn connect(cfg: CentralConfig) -> io::Result<Self> {
        if !auth::valid_name(&cfg.user) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "nom d'abonné invalide (1 à 32 caractères A-Z a-z 0-9 . _ -)"));
        }
        if cfg.password.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "mot de passe vide"));
        }
        let server = cfg
            .server
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "adresse du central introuvable"))?;
        let local: SocketAddr = if server.is_ipv4() {
            ([0, 0, 0, 0], cfg.local_port).into()
        } else {
            (std::net::Ipv6Addr::UNSPECIFIED, cfg.local_port).into()
        };
        let sock = UdpSocket::bind(local)?;
        sock.set_nonblocking(true)?;
        let jb = JitterBuffer::new(cfg.jitter_target_ms, cfg.concealment);
        let stats = NetStats { peer: Some(server.to_string()), codec: cfg.codec.name().to_string(), ..NetStats::default() };
        let now = Instant::now();
        let mut c = Self {
            sock,
            server,
            cfg,
            reg: Reg::Unregistered { next_at: now },
            outbox: Outbox::new(),
            dedup: Dedup::default(),
            keepalive_every: DEFAULT_KEEPALIVE,
            next_keepalive: now,
            salted: None,
            number: None,
            services: Vec::new(),
            directory: Vec::new(),
            next_directory: now,
            last_directory_req: None,
            rtt_probe: None,
            rtt_ms: None,
            call: CallState::Idle,
            events: VecDeque::new(),
            tx_seq: 0,
            tx_ts: 0,
            tx_acc: Vec::with_capacity(FRAME_SAMPLES),
            tx_hist: std::collections::VecDeque::new(),
            pertes: (std::env::var("V34_PERTES").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0), 0x9E37_79B9_7F4A_7C15),
            jb,
            rx_fifo: VecDeque::new(),
            epoch: now,
            stats,
            got_audio: false,
        };
        c.timers(now)?;
        Ok(c)
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.sock.local_addr()
    }

    pub fn server_addr(&self) -> SocketAddr {
        self.server
    }

    /// Nom d'abonné.
    pub fn user(&self) -> &str {
        &self.cfg.user
    }

    /// Numéro attribué par le central (une fois enregistré).
    pub fn number(&self) -> Option<&str> {
        self.number.as_deref()
    }

    pub fn is_registered(&self) -> bool {
        matches!(self.reg, Reg::Registered { .. })
    }

    /// Numéros spéciaux annoncés par le central : (numéro, service).
    pub fn services(&self) -> &[(String, String)] {
        &self.services
    }

    /// Numéro du FAI annoncé par le central (s'il en a un).
    pub fn isp_number(&self) -> Option<&str> {
        self.services.iter().find(|(_, k)| k == "fai").map(|(n, _)| n.as_str())
    }

    pub fn call_state(&self) -> &CallState {
        &self.call
    }

    pub fn next_event(&mut self) -> Option<CentralEvent> {
        self.events.pop_front()
    }

    /// Compose un numéro.
    pub fn dial(&mut self, number: &str) -> io::Result<()> {
        if !signal::valid_number(number) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "numéro invalide (1 à 20 chiffres)"));
        }
        if self.call != CallState::Idle {
            return Err(io::Error::other("un appel est déjà en cours"));
        }
        self.send_reliable(Msg::Dial { number: number.to_string() })?;
        self.call = CallState::Dialing { number: number.to_string() };
        Ok(())
    }

    /// Décroche l'appel entrant : l'audio passe immédiatement.
    pub fn answer(&mut self) -> io::Result<()> {
        let CallState::Incoming { call_id, .. } = self.call else {
            return Err(io::Error::other("aucun appel entrant"));
        };
        self.send_reliable(Msg::Answer { call_id })?;
        self.start_audio(call_id);
        Ok(())
    }

    /// Raccroche (ou abandonne l'appel en cours de numérotation).
    pub fn hangup(&mut self) -> io::Result<()> {
        let call = std::mem::replace(&mut self.call, CallState::Idle);
        if let Some(call_id) = call.call_id() {
            self.send_reliable(Msg::Hangup { call_id, reason: HangupReason::Normal })?;
        }
        Ok(())
    }

    /// Se désinscrit du central (au moment de quitter).
    pub fn unregister(&mut self) -> io::Result<()> {
        if self.is_registered() {
            self.hangup()?;
            self.send_reliable(Msg::Unregister)?;
        }
        Ok(())
    }

    /// Lit la socket et fait tourner les temporisateurs (non bloquant).
    pub fn poll(&mut self) -> io::Result<()> {
        let mut buf = [0u8; 2048];
        loop {
            match self.sock.recv_from(&mut buf) {
                Ok((n, from)) => {
                    if from != self.server {
                        self.stats.foreign += 1;
                        continue;
                    }
                    let d = &buf[..n];
                    if signal::is_control(d) {
                        self.on_control(d)?;
                    } else {
                        self.on_audio(d, n);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if is_transient(&e) => continue,
                Err(e) => return Err(e),
            }
        }
        self.timers(Instant::now())
    }

    // ───────────────────────── interne : signalisation ─────────────────────────

    fn send_raw(&self, bytes: &[u8]) -> io::Result<()> {
        match self.sock.send_to(bytes, self.server) {
            Ok(_) => Ok(()),
            Err(e) if is_transient(&e) => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn send_reliable(&mut self, msg: Msg) -> io::Result<()> {
        let Reg::Registered { session, key } = self.reg else {
            return Err(io::Error::new(io::ErrorKind::NotConnected, "pas enregistré auprès du central"));
        };
        let msg_id = self.outbox.alloc_id();
        let bytes = signal::encode(&Frame { session, msg_id, msg }, Some(&key));
        self.outbox.track(msg_id, bytes.clone(), Instant::now());
        self.send_raw(&bytes)
    }

    fn timers(&mut self, now: Instant) -> io::Result<()> {
        match &mut self.reg {
            Reg::Unregistered { next_at } => {
                if now >= *next_at {
                    *next_at = now + REGISTER_RETRY;
                    let f = Frame { session: 0, msg_id: 0, msg: Msg::Register { name: self.cfg.user.clone() } };
                    let bytes = signal::encode(&f, None);
                    self.send_raw(&bytes)?;
                }
            }
            Reg::Refused { until } => {
                if until.is_some_and(|u| now >= u) {
                    self.reg = Reg::Unregistered { next_at: now };
                    return self.timers(now);
                }
            }
            Reg::Authenticating { auth, next_at, tries, .. } => {
                if now >= *next_at {
                    if *tries >= AUTH_TRIES {
                        self.reg = Reg::Unregistered { next_at: now };
                        return self.timers(now);
                    }
                    *tries += 1;
                    *next_at = now + REGISTER_RETRY;
                    let bytes = auth.clone();
                    self.send_raw(&bytes)?;
                }
            }
            Reg::Registered { session, key } => {
                if now >= self.next_directory && self.last_directory_req.is_none_or(|t| now - t >= DIRECTORY_MIN_GAP) {
                    // Non fiable : pas de msg_id ni d'acquittement.
                    let bytes = signal::encode(&Frame { session: *session, msg_id: 0, msg: Msg::DirectoryReq }, Some(key));
                    self.next_directory = now + DIRECTORY_EVERY;
                    self.last_directory_req = Some(now);
                    self.rtt_probe = Some(now);
                    self.send_raw(&bytes)?;
                }
                if now >= self.next_keepalive {
                    self.next_keepalive = now + self.keepalive_every;
                    self.send_reliable(Msg::Keepalive)?;
                }
                let p = self.outbox.poll(now);
                for b in &p.resend {
                    self.send_raw(b)?;
                }
                if !p.failed.is_empty() {
                    self.lose_server();
                    return self.timers(now);
                }
            }
        }
        Ok(())
    }

    /// Le central ne répond plus : on termine l'appel et on se réenregistre.
    fn lose_server(&mut self) {
        let call = std::mem::replace(&mut self.call, CallState::Idle);
        if call != CallState::Idle {
            self.events.push_back(CentralEvent::HungUp {
                call_id: call.call_id().unwrap_or(0),
                reason: HangupReason::PeerLost,
            });
        }
        self.outbox.clear();
        self.directory.clear();
        self.reg = Reg::Unregistered { next_at: Instant::now() };
        self.events.push_back(CentralEvent::ServerLost);
    }

    fn on_control(&mut self, d: &[u8]) -> io::Result<()> {
        match &self.reg {
            Reg::Refused { .. } => {}
            Reg::Unregistered { .. } => match signal::decode(d, None) {
                Ok(Frame { msg: Msg::Challenge { nonce, iterations, salt }, .. }) => self.start_auth(nonce, iterations, salt)?,
                Ok(Frame { msg: Msg::AuthFail { reason }, .. }) => self.refused(reason),
                _ => {}
            },
            Reg::Authenticating { key, .. } => {
                let key = *key;
                match signal::decode(d, Some(&key)) {
                    Ok(Frame { session, msg: Msg::Welcome { number, keepalive_s, services, .. }, .. }) => {
                        self.reg = Reg::Registered { session, key };
                        self.outbox = Outbox::new();
                        self.dedup = Dedup::default();
                        self.keepalive_every = Duration::from_secs(keepalive_s.clamp(5, 300) as u64);
                        self.next_keepalive = Instant::now() + self.keepalive_every;
                        self.number = Some(number.clone());
                        self.services = services.clone();
                        self.next_directory = Instant::now();
                        self.events.push_back(CentralEvent::Registered { number, services });
                    }
                    // Nouveau défi (le serveur a redémarré, AUTH perdu…) : on recommence.
                    Ok(Frame { msg: Msg::Challenge { nonce, iterations, salt }, .. }) => {
                        self.start_auth(nonce, iterations, salt)?
                    }
                    Ok(Frame { msg: Msg::AuthFail { reason }, .. }) => self.refused(reason),
                    _ => {}
                }
            }
            Reg::Registered { session, key } => {
                let (session, key) = (*session, *key);
                let Ok(f) = signal::decode(d, Some(&key)) else {
                    return Ok(());
                };
                if f.session != session {
                    return Ok(());
                }
                if let Msg::Ack { msg_id } = f.msg {
                    self.outbox.ack(msg_id);
                    return Ok(());
                }
                if let Msg::Directory { entries } = f.msg {
                    self.directory = entries;
                    if let Some(t) = self.rtt_probe.take() {
                        let r = t.elapsed().as_secs_f32() * 1000.0;
                        self.rtt_ms = Some(self.rtt_ms.map_or(r, |x| 0.6 * x + 0.4 * r));
                    }
                    return Ok(());
                }
                if !f.msg.is_reliable() {
                    return Ok(()); // WELCOME répété : déjà traité
                }
                let ack = signal::encode(&Frame { session, msg_id: 0, msg: Msg::Ack { msg_id: f.msg_id } }, Some(&key));
                self.send_raw(&ack)?;
                if self.dedup.check(f.msg_id) == Seen::New {
                    self.on_message(f.msg)?;
                }
            }
        }
        Ok(())
    }

    fn refused(&mut self, reason: AuthFailReason) {
        let until = match reason {
            AuthFailReason::BadCredentials => None,
            _ => Some(Instant::now() + RATE_LIMIT_BACKOFF),
        };
        self.reg = Reg::Refused { until };
        self.events.push_back(CentralEvent::AuthFailed { reason });
    }

    fn start_auth(&mut self, server_nonce: [u8; SERVER_NONCE_LEN], iterations: u32, salt: Vec<u8>) -> io::Result<()> {
        if !(auth::MIN_ITERATIONS..=auth::MAX_ITERATIONS).contains(&iterations) {
            return Ok(()); // paramètres absurdes : on ignore ce défi
        }
        let salted = match &self.salted {
            Some((s, i, k)) if *s == salt && *i == iterations => *k,
            _ => {
                let k = auth::salted_password(&self.cfg.password, &salt, iterations);
                self.salted = Some((salt, iterations, k));
                k
            }
        };
        let client_nonce: [u8; CLIENT_NONCE_LEN] = random_bytes();
        let name = self.cfg.user.clone();
        let (proof, key) = auth::client_proof(&salted, &name, &server_nonce, &client_nonce);
        let f = Frame { session: 0, msg_id: 0, msg: Msg::Auth { name, server_nonce, client_nonce, proof } };
        let auth = signal::encode(&f, None);
        self.send_raw(&auth)?;
        self.reg = Reg::Authenticating { key, auth, next_at: Instant::now() + REGISTER_RETRY, tries: 1 };
        Ok(())
    }

    fn on_message(&mut self, msg: Msg) -> io::Result<()> {
        match msg {
            Msg::Ringing { call_id } => {
                if matches!(self.call, CallState::Dialing { .. }) {
                    self.call = CallState::Ringing { call_id };
                    self.events.push_back(CentralEvent::Ringing { call_id });
                }
            }
            Msg::Ring { call_id, from } => {
                if self.call == CallState::Idle {
                    self.call = CallState::Incoming { call_id, from: from.clone() };
                    self.events.push_back(CentralEvent::Incoming { call_id, from });
                } else if self.call.call_id() != Some(call_id) {
                    // Déjà occupé : on refuse ce second appel.
                    self.send_reliable(Msg::Hangup { call_id, reason: HangupReason::Rejected })?;
                }
            }
            Msg::Answer { call_id } => {
                let ours = match &self.call {
                    CallState::Dialing { .. } => true,
                    CallState::Ringing { call_id: c } => *c == call_id,
                    _ => false,
                };
                if ours {
                    self.start_audio(call_id);
                    self.events.push_back(CentralEvent::Answered { call_id });
                }
            }
            Msg::Hangup { call_id, reason } => {
                // En numérotation, un refus peut précéder le RINGING (paquets
                // dans le désordre) : il concerne forcément notre appel.
                if self.call.call_id() == Some(call_id) || matches!(self.call, CallState::Dialing { .. }) {
                    self.call = CallState::Idle;
                    self.events.push_back(CentralEvent::HungUp { call_id, reason });
                }
            }
            Msg::Busy { number } => {
                if matches!(self.call, CallState::Dialing { .. }) {
                    self.call = CallState::Idle;
                    self.events.push_back(CentralEvent::Busy { number });
                }
            }
            Msg::UnknownNumber { number } => {
                if matches!(self.call, CallState::Dialing { .. }) {
                    self.call = CallState::Idle;
                    self.events.push_back(CentralEvent::UnknownNumber { number });
                }
            }
            // Messages destinés au serveur : ignorés.
            _ => {}
        }
        Ok(())
    }

    // ───────────────────────── interne : audio ─────────────────────────

    fn start_audio(&mut self, call_id: u32) {
        self.call = CallState::Active { call_id };
        self.tx_seq = 0;
        self.tx_ts = 0;
        self.tx_acc.clear();
        self.tx_hist.clear();
        self.jb = JitterBuffer::new(self.cfg.jitter_target_ms, self.cfg.concealment);
        self.rx_fifo.clear();
        self.got_audio = false;
    }

    /// Essais (V34_PERTES) : vrai si ce paquet doit être « perdu en route ».
    fn perdu(&mut self) -> bool {
        if self.pertes.0 <= 0.0 {
            return false;
        }
        let x = &mut self.pertes.1;
        *x ^= *x << 13;
        *x ^= *x >> 7;
        *x ^= *x << 17;
        (*x >> 11) as f64 / (1u64 << 53) as f64 <= self.pertes.0
    }

    fn on_audio(&mut self, d: &[u8], n: usize) {
        if self.perdu() {
            return;
        }
        let Some(pkt) = Packet::decode(d) else {
            self.stats.invalid += 1;
            return;
        };
        let CallState::Active { call_id } = self.call else {
            self.stats.foreign += 1;
            return;
        };
        if pkt.call_id != call_id {
            self.stats.foreign += 1;
            return;
        }
        self.stats.packets_received += 1;
        self.stats.bytes_received += n as u64;
        self.got_audio = true;
        let arrival = self.epoch.elapsed().as_secs_f64();
        self.jb.push_with_backup(pkt.seq, pkt.timestamp, pkt.samples, &pkt.backup, arrival);
    }

    /// Envoie des échantillons de l'appel en cours (jetés hors appel).
    pub fn send_block(&mut self, samples: &[f32]) -> io::Result<()> {
        let CallState::Active { call_id } = self.call else {
            return Ok(());
        };
        let mut rest = samples;
        while !rest.is_empty() {
            let take = (FRAME_SAMPLES - self.tx_acc.len()).min(rest.len());
            self.tx_acc.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            if self.tx_acc.len() == FRAME_SAMPLES {
                let frame = std::mem::take(&mut self.tx_acc);
                let pkt = Packet {
                    codec: self.cfg.codec,
                    call_id,
                    seq: self.tx_seq,
                    timestamp: self.tx_ts,
                    backup: self.tx_hist.iter().cloned().collect(),
                    samples: frame.clone(),
                };
                self.tx_hist.push_front(frame);
                self.tx_hist.truncate(crate::packet::BACKUP_FRAMES);
                self.tx_acc.reserve(FRAME_SAMPLES);
                self.tx_seq = self.tx_seq.wrapping_add(1);
                self.tx_ts = self.tx_ts.wrapping_add(FRAME_SAMPLES as u32);
                let bytes = pkt.encode();
                if self.perdu() {
                    continue;
                }
                match self.sock.send_to(&bytes, self.server) {
                    Ok(n) => {
                        self.stats.packets_sent += 1;
                        self.stats.bytes_sent += n as u64;
                    }
                    Err(e) if is_transient(&e) => {}
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(())
    }

    /// Fait tourner la signalisation puis remplit `out` avec l'audio reçu
    /// (silence hors appel). Vrai si tout `out` est du vrai audio.
    pub fn recv_block(&mut self, out: &mut [f32]) -> bool {
        let _ = self.poll();
        if !matches!(self.call, CallState::Active { .. }) {
            out.fill(0.0);
            return false;
        }
        while self.rx_fifo.len() < out.len() {
            let (frame, kind) = self.jb.pop_frame();
            let real = kind == FrameKind::Audio;
            self.rx_fifo.extend(frame.into_iter().map(|s| (s, real)));
        }
        let mut all_real = true;
        for o in out.iter_mut() {
            let (s, real) = self.rx_fifo.pop_front().unwrap_or((0.0, false));
            *o = s;
            all_real &= real;
        }
        all_real
    }

    /// Annuaire du central (vide tant qu'il n'a pas répondu, ou si le
    /// central est trop ancien pour en avoir un).
    pub fn directory(&self) -> &[DirEntry] {
        &self.directory
    }

    /// Temps d'aller-retour vers le central (ms, lissé).
    pub fn rtt_ms(&self) -> Option<f32> {
        self.rtt_ms
    }

    /// Redemande l'annuaire au prochain `poll` (au plus une fois par seconde).
    pub fn refresh_directory(&mut self) {
        self.next_directory = Instant::now();
    }

    /// Vrai dès qu'un paquet audio de l'appel en cours a été reçu.
    pub fn has_peer_audio(&self) -> bool {
        self.got_audio
    }

    /// Niveau du tampon de gigue (trames présentes, cible) pendant un appel,
    /// pour caler la cadence locale sur l'horloge de l'autre côté.
    pub fn jitter_level(&self) -> Option<(usize, usize)> {
        if matches!(self.call, CallState::Active { .. }) {
            self.jb.level()
        } else {
            None
        }
    }

    pub fn stats(&self) -> NetStats {
        let mut s = self.stats.clone();
        s.jitter = self.jb.stats().clone();
        s
    }
}

impl AudioPort for CentralClient {
    fn send_block(&mut self, samples: &[f32]) -> io::Result<()> {
        CentralClient::send_block(self, samples)
    }
    fn recv_block(&mut self, out: &mut [f32]) -> bool {
        CentralClient::recv_block(self, out)
    }
    fn has_peer_audio(&self) -> bool {
        CentralClient::has_peer_audio(self)
    }
    fn stats(&self) -> NetStats {
        CentralClient::stats(self)
    }
}

fn is_transient(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionRefused | io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    )
}
