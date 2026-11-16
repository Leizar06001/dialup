//! Passerelle SIP du central (protocole : [`crate::sip`]).
//!
//! Un poste SIP (une ligne d'adaptateur téléphonique) est un abonné comme
//! un autre : il a une session, un numéro, et participe aux appels du
//! central. Seule la façon de lui parler change : au lieu des messages
//! `VC`, ce module traduit
//! - les messages du central vers le poste (RING, RINGING, ANSWER,
//!   HANGUP, BUSY…) en requêtes et réponses SIP ([`Server::sip_deliver`]) ;
//! - les requêtes du poste (REGISTER, INVITE, ACK, BYE, CANCEL) en
//!   enregistrement, numérotation, décroché et raccrochage ;
//! - l'audio : RTP ↔ trames du central, une à une, sans transcoder
//!   ([`Server::on_rtp`], [`Server::sip_send_audio`]). Vers le poste, les
//!   trames passent par un tampon et partent au rythme du poste lui-même :
//!   une pour chaque trame reçue de lui. L'ATA voit un flux parfaitement
//!   régulier, quoi que fasse le réseau de l'appli d'en face.
//!
//! Un poste a au plus un appel à la fois, comme un abonné : son « dialogue »
//! SIP en cours (une [`Leg`]) se retrouve par sa session. Les dialogues
//! qui se terminent restent un moment, pour répondre aux retransmissions.

use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use v34_net::crypto::{ct_eq, hmac_sha256, random_bytes};
use v34_net::packet::BACKUP_FRAMES;
use v34_net::signal::{valid_number, HangupReason, Msg};
use v34_net::{Codec, JitterBuffer, Packet, FRAME_SAMPLES};

use super::{is_transient, unix_now, Audio, CallState, Callee, Lien, Reg, Server};
use crate::config::Config;
use crate::sip::message::{self, Message};
use crate::sip::{agent, digest, http_date, rtp, sdp};
use crate::{debug, info, warn};

/// Temporisateurs de RFC 3261 (transport non fiable)
const T1: Duration = Duration::from_millis(500);
const T2: Duration = Duration::from_secs(4);
/// 64 × T1 : abandon d'une transaction
const TIMEOUT: Duration = Duration::from_secs(32);
/// Validité d'un nonce d'authentification (s) ; au-delà : « stale »
const NONCE_TTL: u64 = 300;
/// Marge après l'expiration d'un enregistrement
const GRACE: Duration = Duration::from_secs(10);
const ALLOW: &str = "INVITE, ACK, CANCEL, BYE, OPTIONS, NOTIFY, INFO";
/// Un défi (401/407) ou un refus d'authentification peut dépasser la
/// requête d'autant (sinon un REGISTER sobre n'aurait jamais de défi) ;
/// les autres réponses à une source inconnue, jamais.
const CHALLENGE_SLACK: usize = 256;

/// Tampon des trames vers un poste quand elles viennent d'une appli, par
/// Internet (Wi-Fi : pointes de 50 à 150 ms). Un adaptateur téléphonique a
/// un tampon de gigue adaptatif : des paquets en retard, puis la rafale qui
/// suit une coupure, il les joue tous, en retard, et tout le reste de
/// l'appel glisse d'autant (un Cisco ATA : +520 ms après une coupure de
/// 520 ms). L'écho de sa prise se décale, le modem d'en face réapprend
/// tout. On absorbe donc la gigue ici, et un paquet trop en retard devient
/// un trou à sa place, jamais un glissement. Plus profond, il rallonge
/// d'autant l'aller-retour, que la phase 2 d'un vrai modem supporte mal
/// (au-delà d'une seconde, un U.S. Robotics ne voit plus notre ton B).
const TX_DEPTH_APPLI_MS: u32 = 200;
/// Le rythme de l'ATA est extrapolé depuis sa dernière trame reçue pendant
/// au plus ce temps : son flux montant peut caler (0,5 à 0,7 s mesurés)
/// sans que le nôtre, vers lui, s'arrête. Au-delà (détection de silence de
/// l'ATA), les trames partent au rythme de l'autre bout.
const UPLINK_STALE: Duration = Duration::from_millis(1000);
/// Rattrapage maximal d'un coup (trames)
const MAX_CATCHUP: i32 = 50;

/// Ce qu'un abonné SIP a de particulier (voir [`Lien::Sip`]).
pub(super) struct Poste {
    /// URI de contact donnée à l'enregistrement : où l'appeler
    pub contact: String,
    pub expires: Instant,
}

impl Poste {
    pub(super) fn expired(&self, now: Instant) -> bool {
        now > self.expires + GRACE
    }
}

pub(super) struct SipState {
    realm: String,
    public_ip: Option<IpAddr>,
    listen: SocketAddr,
    max_expires: u32,
    legs: Vec<Leg>,
    /// Réponses récentes aux BYE et CANCEL, pour leurs retransmissions
    recent: Vec<Recent>,
    warned_private: bool,
    /// Réponses tues (anti-amplification) : dernier avis par IP
    muted: std::collections::HashMap<IpAddr, Instant>,
}

struct Recent {
    call_id: String,
    cseq: u32,
    method: String,
    bytes: Vec<u8>,
    at: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    /// Le poste appelle (nous répondons à son INVITE)
    Uas,
    /// On appelle le poste (notre INVITE)
    Uac,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// INVITE du poste reçu, l'appel sonne au central
    Incoming,
    /// Notre INVITE envoyé, pas encore de réponse
    Calling,
    /// Le poste a répondu « ça sonne »
    Proceeding,
    /// Décroché (200) envoyé au poste, en attente de son ACK
    WaitAck,
    /// Dialogue établi : l'audio passe
    Confirmed,
    /// Fin en cours (l'appel n'existe plus au central)
    Ending,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Invite,
    Ok,
    Final,
    Bye,
    Cancel,
    Reinvite,
}

/// Message répété tant que l'autre côté ne l'a pas acquitté.
struct Retrans {
    kind: Kind,
    bytes: Vec<u8>,
    to: SocketAddr,
    next: Instant,
    interval: Duration,
    deadline: Instant,
}

impl Retrans {
    fn new(kind: Kind, bytes: Vec<u8>, to: SocketAddr, now: Instant) -> Retrans {
        Retrans { kind, bytes, to, next: now + T1, interval: T1, deadline: now + TIMEOUT }
    }
}

/// Un dialogue SIP avec un poste, rattaché à un appel du central.
struct Leg {
    session: u32,
    name: String,
    /// Appel du central (None : terminé, ou pas encore créé)
    call: Option<u32>,
    role: Role,
    state: State,
    sip_call_id: String,
    local_tag: String,
    /// En-têtes From / To de NOS requêtes dans ce dialogue
    local_hdr: String,
    remote_hdr: String,
    /// URI à laquelle adresser nos requêtes (Contact du poste)
    remote_target: String,
    local_cseq: u32,
    invite_cseq: u32,
    /// Adresse de signalisation du poste
    sig: SocketAddr,
    /// L'INVITE qui a créé le dialogue (le sien ou le nôtre)
    invite: Message,
    last_response: Option<Vec<u8>>,
    retrans: Option<Retrans>,
    cancel_pending: bool,
    bye_pending: bool,
    /// Dernier re-INVITE traité et notre réponse
    reinvite: Option<(u32, Vec<u8>)>,
    law: Codec,
    pt: u8,
    /// Où envoyer le RTP : deviné d'après le SDP, puis calé sur la source
    /// réelle du premier paquet reçu (traversée du NAT de l'abonné)
    rtp_dest: Option<SocketAddr>,
    /// Destination déduite du dernier SDP du poste
    sdp_dest: Option<SocketAddr>,
    latched: bool,
    /// Trames vers le poste, en attente de leur créneau
    tx_jb: Option<JitterBuffer>,
    /// Numéro de notre prochaine trame vers le poste (le nôtre : régulier)
    tx_out: u32,
    /// Dernière trame reçue du poste, et quand : son horloge cadence l'envoi
    up_last: Option<u32>,
    up_at: Option<Instant>,
    /// Trame du poste jusqu'à laquelle on a déjà envoyé la nôtre
    tx_mark: u32,
    /// Envoi cadencé par le poste (pas avec un modem du central, qui
    /// envoie lui-même son RTP à la cadence du poste)
    paced: bool,
    reframer: rtp::Reframer,
    numbering: rtp::Numbering,
    /// Trames reçues récemment (copies de redondance vers l'appli)
    history: VecDeque<(u32, Vec<f32>)>,
    sdp_id: u64,
    sdp_version: u64,
    warned_ptime: bool,
    /// Création, ou début de la fin
    since: Instant,
}

impl Leg {
    fn new(session: u32, name: String, role: Role, invite: Message, sig: SocketAddr, now: Instant) -> Leg {
        Leg {
            session,
            name,
            call: None,
            role,
            state: if role == Role::Uas { State::Incoming } else { State::Calling },
            sip_call_id: invite.call_id().unwrap_or_default().to_string(),
            local_tag: rand_hex(10),
            local_hdr: String::new(),
            remote_hdr: String::new(),
            remote_target: String::new(),
            local_cseq: 1,
            invite_cseq: invite.cseq().map_or(1, |c| c.0),
            sig,
            invite,
            last_response: None,
            retrans: None,
            cancel_pending: false,
            bye_pending: false,
            reinvite: None,
            law: Codec::Alaw,
            pt: 8,
            rtp_dest: None,
            sdp_dest: None,
            latched: false,
            tx_jb: None,
            tx_out: 0,
            up_last: None,
            up_at: None,
            tx_mark: 0,
            paced: false,
            reframer: rtp::Reframer::default(),
            numbering: rtp::Numbering::new(8),
            history: VecDeque::new(),
            sdp_id: u64::from_be_bytes(random_bytes::<8>()) >> 1,
            sdp_version: 1,
            warned_ptime: false,
            since: now,
        }
    }

    fn active(&self) -> bool {
        self.state != State::Ending
    }

    fn media_live(&self) -> bool {
        matches!(self.state, State::WaitAck | State::Confirmed) && self.call.is_some()
    }
}

impl SipState {
    pub(super) fn new(cfg: &Config, listen: SocketAddr) -> SipState {
        SipState {
            realm: cfg.sip.domaine.trim().to_string(),
            public_ip: cfg.sip.public_ip().ok().flatten(),
            listen,
            max_expires: cfg.sip.enregistrement_max_s,
            legs: Vec::new(),
            recent: Vec::new(),
            warned_private: false,
            muted: Default::default(),
        }
    }

    /// Adresse IP à annoncer à ce poste (Via, Contact, SDP).
    fn local_ip(&mut self, peer: SocketAddr) -> IpAddr {
        if let Some(ip) = self.public_ip {
            return ip;
        }
        if !self.listen.ip().is_unspecified() {
            return self.listen.ip();
        }
        let any: SocketAddr = if peer.is_ipv4() { ([0, 0, 0, 0], 0).into() } else { (std::net::Ipv6Addr::UNSPECIFIED, 0).into() };
        let ip = UdpSocket::bind(any).and_then(|s| s.connect(peer).and_then(|()| s.local_addr())).map(|a| a.ip()).unwrap_or(self.listen.ip());
        if is_private(ip) && !is_private(peer.ip()) && !self.warned_private {
            self.warned_private = true;
            warn!("SIP : l'adresse annoncée pour l'audio ({ip}) est privée ; si le VPS est derrière un NAT, réglez [sip] adresse_publique");
        }
        ip
    }
}

fn rand_hex(n: usize) -> String {
    let mut s = digest::hex(&random_bytes::<16>());
    s.truncate(n.min(32));
    s
}

fn host(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v) => v.to_string(),
        IpAddr::V6(v) => format!("[{v}]"),
    }
}

/// Adresse non routable sur Internet (réseau local, NAT de l'opérateur…).
fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let o = v.octets();
            v.is_private() || v.is_link_local() || v.is_loopback() || v.is_unspecified() || (o[0] == 100 && o[1] & 0xC0 == 64)
        }
        IpAddr::V6(v) => {
            let s = v.segments()[0];
            v.is_loopback() || v.is_unspecified() || s & 0xFE00 == 0xFC00 || s & 0xFFC0 == 0xFE80
        }
    }
}

/// Destination RTP d'après le SDP : une adresse privée (le poste est
/// derrière un NAT) est remplacée par celle d'où vient la signalisation.
fn guess_rtp(audio: Option<(Option<IpAddr>, u16)>, sig: SocketAddr) -> Option<SocketAddr> {
    let (ip, port) = audio?;
    let ip = ip.filter(|ip| !is_private(*ip)).unwrap_or(sig.ip());
    Some(SocketAddr::new(ip, port))
}

fn send_sip(sock: &UdpSocket, msg: &Message, to: SocketAddr) -> Vec<u8> {
    let bytes = msg.to_bytes();
    debug!("SIP → {to} : {}\n{}", msg.summary(), String::from_utf8_lossy(&bytes).trim_end());
    if let Err(e) = sock.send_to(&bytes, to) {
        if !is_transient(&e) {
            debug!("envoi SIP vers {to} : {e}");
        }
    }
    bytes
}

/// Réponse à `req` : Via (avec l'adresse vue), From, To (+ notre tag),
/// Call-ID, CSeq.
fn reply(req: &Message, code: u16, to_tag: Option<&str>, from: SocketAddr) -> Message {
    let mut r = Message::response(code);
    for (i, v) in req.headers("Via").enumerate() {
        r.push("Via", if i == 0 { via_received(v, from) } else { v.to_string() });
    }
    if let Some(f) = req.header("From") {
        r.push("From", f);
    }
    if let Some(t) = req.header("To") {
        let t = match to_tag {
            Some(tag) if code > 100 && message::param(t, "tag").is_none() => format!("{t};tag={tag}"),
            _ => t.to_string(),
        };
        r.push("To", t);
    }
    if let Some(c) = req.call_id() {
        r.push("Call-ID", c);
    }
    if let Some(c) = req.header("CSeq") {
        r.push("CSeq", c);
    }
    r.push("Server", agent());
    r
}

/// Premier Via complété de l'adresse d'où vient vraiment la requête
/// (`received`, et `rport` si le poste l'a demandé : RFC 3581).
fn via_received(line: &str, from: SocketAddr) -> String {
    let line = line.trim();
    let first = message::first_value(line);
    let rest = line.get(first.len()..).unwrap_or("");
    let mut parts = first.split(';');
    let mut out = parts.next().unwrap_or("").trim().to_string();
    let mut rport = false;
    for p in parts {
        let k = p.split('=').next().unwrap_or("").trim();
        if k.eq_ignore_ascii_case("rport") {
            rport = true;
        } else if !k.eq_ignore_ascii_case("received") {
            out.push(';');
            out.push_str(p.trim());
        }
    }
    out.push_str(&format!(";received={}", from.ip()));
    if rport {
        out.push_str(&format!(";rport={}", from.port()));
    }
    out + rest
}

/// Requête dans un dialogue établi (BYE, ACK d'un 200).
fn dialog_request(leg: &mut Leg, method: &str, ip: IpAddr, port: u16) -> Message {
    let mut m = Message::request(method, &leg.remote_target);
    m.push("Via", format!("SIP/2.0/UDP {}:{port};branch=z9hG4bK{};rport", host(ip), rand_hex(16)));
    m.push("Max-Forwards", "70");
    m.push("From", leg.local_hdr.clone());
    m.push("To", leg.remote_hdr.clone());
    m.push("Call-ID", leg.sip_call_id.clone());
    let n = if method == "ACK" {
        leg.invite_cseq
    } else {
        leg.local_cseq += 1;
        leg.local_cseq
    };
    m.push("CSeq", format!("{n} {method}"));
    m.push("User-Agent", agent());
    m
}

/// Requête de la même transaction que notre INVITE (CANCEL, ACK d'un refus).
fn invite_transaction_request(inv: &Message, method: &str, to: &str) -> Message {
    let mut m = Message::request(method, inv.uri().unwrap_or_default());
    if let Some(v) = inv.header("Via") {
        m.push("Via", v);
    }
    m.push("Max-Forwards", "70");
    m.push("From", inv.header("From").unwrap_or_default());
    m.push("To", to);
    m.push("Call-ID", inv.call_id().unwrap_or_default());
    m.push("CSeq", format!("{} {method}", inv.cseq().map_or(1, |c| c.0)));
    m.push("User-Agent", agent());
    m
}

impl Server {
    // ───────────────────────── entrée ─────────────────────────

    pub(super) fn on_sip(&mut self, buf: &[u8], from: SocketAddr) {
        let now = Instant::now();
        if self.sip.is_none() || self.is_blocked(from.ip(), now) {
            return;
        }
        let Some(msg) = Message::parse(buf) else {
            debug!("SIP ← {from} : message illisible ({} octets)", buf.len());
            return;
        };
        debug!("SIP ← {from} : {}\n{}", msg.summary(), String::from_utf8_lossy(buf).trim_end());
        if msg.code().is_some() {
            self.sip_response(&msg, now);
        } else {
            self.sip_request(&msg, buf.len(), from, now);
        }
    }

    fn sip_request(&mut self, req: &Message, len: usize, from: SocketAddr, now: Instant) {
        let method = req.method().unwrap_or_default();
        let (Some(call_id), Some((cseq, cseq_method))) = (req.call_id(), req.cseq()) else { return };
        if req.header("From").is_none() || req.header("To").is_none() || req.header("Via").is_none() {
            return;
        }
        if cseq_method != method {
            return self.sip_guarded(reply(req, 400, Some(&rand_hex(10)), from), len, from);
        }
        let again = self.sip.as_ref().and_then(|st| st.recent.iter().find(|r| r.call_id == call_id && r.cseq == cseq && r.method == method));
        if let Some(r) = again {
            let b = r.bytes.clone();
            return self.send_to(&b, from);
        }
        match method {
            "REGISTER" => self.sip_register(req, len, from, now),
            "INVITE" if req.to_tag().is_some() => self.sip_reinvite(req, len, from, now),
            "INVITE" => self.sip_invite(req, len, from, now),
            "ACK" => self.sip_ack(req, now),
            "BYE" => self.sip_bye(req, len, from, now),
            "CANCEL" => self.sip_cancel(req, len, from, now),
            // keepalive NAT de l'adaptateur ($NOTIFY), sondes, infos
            "OPTIONS" | "NOTIFY" | "INFO" | "UPDATE" => {
                let mut r = reply(req, 200, Some(&rand_hex(10)), from);
                r.push("Allow", ALLOW);
                self.sip_guarded(r, len, from);
            }
            _ => {
                let mut r = reply(req, 405, Some(&rand_hex(10)), from);
                r.push("Allow", ALLOW);
                self.sip_guarded(r, len, from);
            }
        }
    }

    /// Réponse vers une source peut-être inconnue : jamais plus grosse que
    /// la requête (le central ne sert pas d'amplificateur), sauf vers
    /// l'adresse d'un poste enregistré, et à `slack` octets près pour les
    /// défis d'authentification.
    fn sip_guarded(&mut self, r: Message, req_len: usize, to: SocketAddr) {
        self.sip_guarded_slack(r, req_len, 0, to);
    }

    fn sip_guarded_slack(&mut self, r: Message, req_len: usize, slack: usize, to: SocketAddr) {
        let known = self.regs.values().any(|reg| matches!(reg.lien, Lien::Sip(_)) && reg.addr.ip() == to.ip());
        let len = r.to_bytes().len();
        if known || len <= req_len + slack {
            send_sip(&self.sock, &r, to);
            return;
        }
        // Tue, mais pas en silence : un adaptateur mal réglé se voit au
        // journal (un avis par minute et par adresse au plus).
        let Some(st) = self.sip.as_mut() else { return };
        let now = Instant::now();
        if st.muted.get(&to.ip()).is_none_or(|t| now.duration_since(*t) > Duration::from_secs(60)) {
            if st.muted.len() > 1000 {
                st.muted.clear();
            }
            st.muted.insert(to.ip(), now);
            info!("SIP : pas de réponse « {} » à {to} ({len} octets pour une requête de {req_len})", r.summary());
        }
    }

    // ───────────────────────── authentification ─────────────────────────

    /// Nonce sans état : horodatage + HMAC(clé du serveur, horodatage, IP).
    fn sip_nonce(&self, ip: IpAddr, ts: u64) -> String {
        let mac = hmac_sha256(&self.server_key, &[b"V34-SIP-NONCE\0", &ts.to_be_bytes(), ip.to_string().as_bytes()]);
        format!("{ts:016x}{}", digest::hex(&mac[..12]))
    }

    /// Âge (s) d'un nonce émis par ce central pour cette IP.
    fn sip_nonce_age(&self, nonce: &str, ip: IpAddr) -> Option<u64> {
        let ts = u64::from_str_radix(nonce.get(..16)?, 16).ok()?;
        (nonce.len() == 40 && ct_eq(self.sip_nonce(ip, ts).as_bytes(), nonce.as_bytes())).then(|| unix_now().saturating_sub(ts))
    }

    fn sip_challenge(&mut self, req: &Message, code: u16, stale: bool, len: usize, from: SocketAddr) {
        let Some(st) = self.sip.as_ref() else { return };
        let nonce = self.sip_nonce(from.ip(), unix_now());
        let mut r = reply(req, code, Some(&rand_hex(10)), from);
        let h = if code == 401 { "WWW-Authenticate" } else { "Proxy-Authenticate" };
        let stale = if stale { ", stale=TRUE" } else { "" };
        r.push(h, format!("Digest realm=\"{}\", nonce=\"{nonce}\", algorithm=MD5, qop=\"auth\"{stale}", st.realm));
        self.sip_guarded_slack(r, len, CHALLENGE_SLACK, from);
    }

    /// Nom du poste qui a signé cette requête ; sinon, défi (pas de
    /// signature, nonce inconnu ou périmé) ou refus (403).
    fn sip_auth(&mut self, req: &Message, method: &str, challenge: u16, len: usize, from: SocketAddr, now: Instant) -> Option<String> {
        let realm = self.sip.as_ref()?.realm.clone();
        let creds = req.headers("Authorization").chain(req.headers("Proxy-Authorization")).filter_map(digest::parse).find(|c| c.realm == realm);
        let Some(c) = creds else {
            self.sip_challenge(req, challenge, false, len, from);
            return None;
        };
        match self.sip_nonce_age(&c.nonce, from.ip()) {
            None => {
                self.sip_challenge(req, challenge, false, len, from);
                return None;
            }
            Some(age) if age > NONCE_TTL => {
                self.sip_challenge(req, challenge, true, len, from);
                return None;
            }
            Some(_) => {}
        }
        if self.annuaire.poste(&c.username).is_some_and(|(_, ha1)| digest::verify(ha1, method, &c)) {
            return Some(c.username);
        }
        self.record_failure(from.ip(), now);
        info!("SIP : authentification refusée pour « {} » depuis {from}", c.username);
        self.sip_guarded_slack(reply(req, 403, Some(&rand_hex(10)), from), len, CHALLENGE_SLACK, from);
        None
    }

    /// Session du poste SIP enregistré sous ce nom.
    fn sip_session(&self, name: &str) -> Option<u32> {
        self.by_name.get(name).copied().filter(|s| matches!(self.regs.get(s).map(|r| &r.lien), Some(Lien::Sip(_))))
    }

    // ───────────────────────── enregistrement ─────────────────────────

    fn sip_register(&mut self, req: &Message, len: usize, from: SocketAddr, now: Instant) {
        let Some(name) = self.sip_auth(req, "REGISTER", 401, len, from, now) else { return };
        let max = self.sip.as_ref().map_or(120, |s| s.max_expires);
        let contact = req.header("Contact");
        let requested: u32 =
            contact.and_then(|c| message::param(c, "expires")).or_else(|| req.header("Expires")).and_then(|v| v.trim().parse().ok()).unwrap_or(3600);
        let current = self.sip_session(&name);
        let tag = rand_hex(10);
        let Some(contact) = contact else {
            // simple consultation des enregistrements
            let mut r = reply(req, 200, Some(&tag), from);
            if let Some(Reg { lien: Lien::Sip(p), .. }) = current.and_then(|s| self.regs.get(&s)) {
                r.push("Contact", format!("<{}>;expires={}", p.contact, p.expires.saturating_duration_since(now).as_secs()));
            }
            send_sip(&self.sock, &r, from);
            return;
        };
        if contact.trim() == "*" || requested == 0 {
            if let Some(s) = current {
                self.remove_registration(s, "désinscription SIP");
            }
            send_sip(&self.sock, &reply(req, 200, Some(&tag), from), from);
            return;
        }
        let expires = requested.clamp(10, max);
        let uri = message::addr_uri(contact).to_string();
        let until = now + Duration::from_secs(expires as u64);
        match current {
            Some(s) => {
                let reg = self.regs.get_mut(&s).expect("session présente");
                if reg.addr != from {
                    info!("poste SIP {name} : nouvelle adresse {from} (était {})", reg.addr);
                    reg.addr = from;
                }
                reg.last_seen = now;
                reg.lien = Lien::Sip(Poste { contact: uri.clone(), expires: until });
            }
            None => {
                let Some(number) = self.annuaire.poste(&name).map(|(n, _)| n.clone()) else { return };
                if let Some(&old) = self.by_name.get(&name) {
                    self.remove_registration(old, "remplacé par le poste SIP du même nom");
                }
                if self.regs.len() >= self.cfg.central.connectes_max {
                    warn!("poste SIP {name} refusé : limite de {} abonnés connectés", self.cfg.central.connectes_max);
                    send_sip(&self.sock, &reply(req, 503, Some(&tag), from), from);
                    return;
                }
                let session = self.new_session_id();
                let lien = Lien::Sip(Poste { contact: uri.clone(), expires: until });
                self.regs.insert(session, Reg { name: name.clone(), number: number.clone(), addr: from, last_seen: now, call: None, lien });
                self.by_name.insert(name.clone(), session);
                self.by_number.insert(number.clone(), session);
                info!("poste SIP {name} enregistré au {number} depuis {from} ({})", req.header("User-Agent").unwrap_or("?"));
                if let Some(f) = self.facturation.as_mut() {
                    f.connexion(&name, unix_now());
                }
            }
        }
        let mut r = reply(req, 200, Some(&tag), from);
        r.push("Contact", format!("<{uri}>;expires={expires}"));
        r.push("Expires", expires.to_string());
        r.push("Date", http_date(unix_now()));
        send_sip(&self.sock, &r, from);
    }

    // ───────────────────────── le poste appelle ─────────────────────────

    fn sip_invite(&mut self, req: &Message, len: usize, from: SocketAddr, now: Instant) {
        let call_id = req.call_id().unwrap_or_default();
        let cseq = req.cseq().map_or(0, |c| c.0);
        // retransmission d'un INVITE déjà traité : même réponse
        let again = self.sip.as_ref().and_then(|st| st.legs.iter().find(|l| l.role == Role::Uas && l.sip_call_id == call_id && l.invite_cseq == cseq));
        if let Some(l) = again {
            if let Some(b) = l.last_response.clone() {
                self.send_to(&b, from);
            }
            return;
        }
        let Some(name) = self.sip_auth(req, "INVITE", 407, len, from, now) else { return };
        let tag = rand_hex(10);
        let refuse = |s: &Server, code: u16| {
            send_sip(&s.sock, &reply(req, code, Some(&tag), from), from);
        };
        let Some(session) = self.sip_session(&name) else {
            info!("poste SIP {name} : appel refusé, poste non enregistré");
            return refuse(self, 403);
        };
        if let Some(ext) = req.header("Require") {
            let mut r = reply(req, 420, Some(&tag), from);
            r.push("Unsupported", ext);
            send_sip(&self.sock, &r, from);
            return;
        }
        let offer = sdp::Session::parse(req.body_str());
        let Some((law, pt)) = offer.as_ref().and_then(sdp::Session::choose) else {
            info!("poste SIP {name} : appel refusé, pas de G.711 proposé (réglez le codec G711a)");
            return refuse(self, 488);
        };
        let busy = self.regs.get(&session).is_some_and(|r| r.call.is_some())
            || self.sip.as_ref().is_some_and(|st| st.legs.iter().any(|l| l.session == session && l.active()));
        if busy {
            return refuse(self, 486);
        }
        let Some(number) = req.uri().and_then(message::uri_user).filter(|n| valid_number(n)).map(str::to_string) else {
            info!("poste SIP {name} : numéro invalide ({})", req.uri().unwrap_or_default());
            return refuse(self, 404);
        };
        if let Some(p) = offer.as_ref().and_then(|o| o.ptime).filter(|p| *p != 20) {
            info!("poste SIP {name} : paquets de {p} ms demandés (recoupés en 20 ms ; mieux : RTP Packet Size 0.020)");
        }
        let mut leg = Leg::new(session, name.clone(), Role::Uas, req.clone(), from, now);
        leg.local_hdr = format!("{};tag={}", req.header("To").unwrap_or_default(), leg.local_tag);
        leg.remote_hdr = req.header("From").unwrap_or_default().to_string();
        leg.remote_target = message::addr_uri(req.header("Contact").or(req.header("From")).unwrap_or_default()).to_string();
        leg.law = law;
        leg.pt = pt;
        leg.numbering.pt = pt;
        leg.rtp_dest = guess_rtp(offer.and_then(|o| o.audio), from);
        leg.sdp_dest = leg.rtp_dest;
        leg.last_response = Some(send_sip(&self.sock, &reply(req, 100, None, from), from));
        if let Some(st) = self.sip.as_mut() {
            st.legs.push(leg);
        }
        info!("poste SIP {name} compose le {number}");
        // Le central répond tout de suite (RINGING, BUSY…) : sip_deliver.
        self.on_dial(session, number);
    }

    /// Réponse au poste sur son INVITE (180, 200 avec notre SDP, refus).
    fn sip_uas_respond(&mut self, i: usize, code: u16, now: Instant) {
        let Some(st) = self.sip.as_mut() else { return };
        let sig = st.legs[i].sig;
        let ip = st.local_ip(sig);
        let port = st.listen.port();
        let leg = &mut st.legs[i];
        let mut r = reply(&leg.invite, code, Some(&leg.local_tag), sig);
        if code < 300 {
            r.push("Contact", format!("<sip:v34net@{}:{port}>", host(ip)));
        }
        if code == 200 {
            r.push("Allow", ALLOW);
            r.push("Content-Type", "application/sdp");
            r.body = sdp::build(ip, port, &[leg.pt], leg.sdp_id, leg.sdp_version).into_bytes();
        }
        let bytes = send_sip(&self.sock, &r, sig);
        leg.last_response = Some(bytes.clone());
        if (200..300).contains(&code) {
            leg.state = State::WaitAck;
            leg.retrans = Some(Retrans::new(Kind::Ok, bytes, sig, now));
        } else if code >= 300 {
            leg.state = State::Ending;
            leg.since = now;
            leg.retrans = Some(Retrans::new(Kind::Final, bytes, sig, now));
        }
    }

    fn sip_ack(&mut self, req: &Message, now: Instant) {
        let (Some(call_id), Some((cseq, _))) = (req.call_id(), req.cseq()) else { return };
        let Some(st) = self.sip.as_mut() else { return };
        // ACK d'un re-INVITE du poste : dans un dialogue de l'un ou l'autre sens
        if let Some(leg) = st.legs.iter_mut().find(|l| l.sip_call_id == call_id && l.reinvite.as_ref().is_some_and(|(c, _)| *c == cseq)) {
            if leg.retrans.as_ref().is_some_and(|r| r.kind == Kind::Reinvite) {
                leg.retrans = None;
            }
            return;
        }
        let Some(i) = st.legs.iter().position(|l| l.role == Role::Uas && l.sip_call_id == call_id) else { return };
        let leg = &mut st.legs[i];
        if cseq != leg.invite_cseq {
            return;
        }
        match leg.state {
            State::WaitAck => {
                leg.retrans = None;
                leg.state = State::Confirmed;
                if std::mem::take(&mut leg.bye_pending) {
                    self.sip_send_bye(i, now);
                }
            }
            State::Ending if leg.retrans.as_ref().is_some_and(|r| r.kind == Kind::Final) => leg.retrans = None,
            _ => {}
        }
    }

    fn sip_cancel(&mut self, req: &Message, len: usize, from: SocketAddr, now: Instant) {
        let (Some(call_id), Some((cseq, _))) = (req.call_id(), req.cseq()) else { return };
        let branch = req.branch();
        let found = self.sip.as_ref().and_then(|st| {
            let i = st.legs.iter().position(|l| l.role == Role::Uas && l.sip_call_id == call_id && l.invite.branch() == branch)?;
            Some((i, st.legs[i].local_tag.clone()))
        });
        let Some((i, tag)) = found else {
            return self.sip_guarded(reply(req, 481, Some(&rand_hex(10)), from), len, from);
        };
        let bytes = send_sip(&self.sock, &reply(req, 200, Some(&tag), from), from);
        let Some(st) = self.sip.as_mut() else { return };
        st.recent.push(Recent { call_id: call_id.to_string(), cseq, method: "CANCEL".into(), bytes, at: now });
        let leg = &st.legs[i];
        if leg.state != State::Incoming {
            return;
        }
        let (call, session, name) = (leg.call, leg.session, leg.name.clone());
        self.sip_uas_respond(i, 487, now);
        if let Some(c) = call {
            info!("appel {c:08x} : le poste SIP {name} raccroche avant la réponse");
            self.end_call(c, Some(session), HangupReason::Normal);
        }
    }

    fn sip_reinvite(&mut self, req: &Message, len: usize, from: SocketAddr, now: Instant) {
        let (Some(call_id), Some((cseq, _))) = (req.call_id(), req.cseq()) else { return };
        let Some(st) = self.sip.as_mut() else { return };
        let Some(i) = st.legs.iter().position(|l| l.sip_call_id == call_id && l.active()) else {
            return self.sip_guarded(reply(req, 481, Some(&rand_hex(10)), from), len, from);
        };
        if let Some((c, b)) = &st.legs[i].reinvite {
            if *c == cseq {
                let b = b.clone();
                return self.send_to(&b, from);
            }
        }
        let ip = st.local_ip(from);
        let port = st.listen.port();
        let leg = &mut st.legs[i];
        if leg.state != State::Confirmed {
            send_sip(&self.sock, &reply(req, 491, None, from), from);
            return;
        }
        let offer = sdp::Session::parse(req.body_str());
        let code = match offer.as_ref().map(sdp::Session::choose) {
            // Sans G.711 (bascule en T.38) : refusé, le poste reste en G.711
            Some(None) => {
                info!("poste SIP {} : bascule {} refusée, on reste en G.711", leg.name, if offer.as_ref().is_some_and(|o| o.t38) { "T.38" } else { "sans G.711" });
                488
            }
            Some(Some((law, pt))) => {
                if (law, pt) != (leg.law, leg.pt) {
                    (leg.law, leg.pt, leg.numbering.pt) = (law, pt, pt);
                    leg.sdp_version += 1;
                }
                // nouvelle adresse média : on la reprend et on se recale sur
                // les paquets ; même adresse (simple rafraîchissement) : rien
                let d = guess_rtp(offer.and_then(|o| o.audio), from);
                if d.is_some() && d != leg.sdp_dest {
                    leg.sdp_dest = d;
                    leg.rtp_dest = d;
                    leg.latched = false;
                }
                200
            }
            None => 200, // sans offre : la nôtre, réponse dans l'ACK
        };
        let mut r = reply(req, code, None, from);
        if code == 200 {
            r.push("Contact", format!("<sip:v34net@{}:{port}>", host(ip)));
            r.push("Content-Type", "application/sdp");
            r.body = sdp::build(ip, port, &[leg.pt], leg.sdp_id, leg.sdp_version).into_bytes();
        }
        let bytes = send_sip(&self.sock, &r, from);
        leg.reinvite = Some((cseq, bytes.clone()));
        leg.retrans = Some(Retrans::new(Kind::Reinvite, bytes, from, now));
    }

    fn sip_bye(&mut self, req: &Message, len: usize, from: SocketAddr, now: Instant) {
        let (Some(call_id), Some((cseq, _))) = (req.call_id(), req.cseq()) else { return };
        let Some(i) = self.sip.as_ref().and_then(|st| st.legs.iter().position(|l| l.sip_call_id == call_id)) else {
            return self.sip_guarded(reply(req, 481, Some(&rand_hex(10)), from), len, from);
        };
        let bytes = send_sip(&self.sock, &reply(req, 200, None, from), from);
        let Some(st) = self.sip.as_mut() else { return };
        st.recent.push(Recent { call_id: call_id.to_string(), cseq, method: "BYE".into(), bytes, at: now });
        let leg = st.legs.remove(i);
        if let Some(c) = leg.call.filter(|c| leg.active() && self.calls.contains_key(c)) {
            info!("appel {c:08x} : le poste SIP {} raccroche", leg.name);
            self.end_call(c, Some(leg.session), HangupReason::Normal);
        }
    }

    // ───────────────────────── on appelle le poste ─────────────────────────

    fn sip_call_out(&mut self, session: u32, call_id: u32, caller: &str) {
        let Some(Reg { name, addr, lien: Lien::Sip(p), .. }) = self.regs.get(&session) else { return };
        let (name, sig, target) = (name.clone(), *addr, p.contact.clone());
        let Some(st) = self.sip.as_mut() else { return };
        let ip = st.local_ip(sig);
        let port = st.listen.port();
        let now = Instant::now();
        let caller = if valid_number(caller) { caller } else { "anonymous" };
        let local_tag = rand_hex(10);
        let local_hdr = format!("\"{caller}\" <sip:{caller}@{}>;tag={local_tag}", st.realm);
        let remote_hdr = format!("<sip:{name}@{}>", st.realm);
        let mut inv = Message::request("INVITE", &target);
        inv.push("Via", format!("SIP/2.0/UDP {}:{port};branch=z9hG4bK{};rport", host(ip), rand_hex(16)))
            .push("Max-Forwards", "70")
            .push("From", local_hdr.clone())
            .push("To", remote_hdr.clone())
            .push("Call-ID", format!("{}@{}", rand_hex(24), host(ip)))
            .push("CSeq", "1 INVITE")
            .push("Contact", format!("<sip:v34net@{}:{port}>", host(ip)))
            .push("Allow", ALLOW)
            .push("User-Agent", agent())
            .push("Content-Type", "application/sdp");
        let mut leg = Leg::new(session, name.clone(), Role::Uac, inv, sig, now);
        leg.invite.body = sdp::build(ip, port, &[8, 0], leg.sdp_id, leg.sdp_version).into_bytes();
        leg.local_tag = local_tag;
        leg.local_hdr = local_hdr;
        leg.remote_hdr = remote_hdr;
        leg.remote_target = target;
        leg.call = Some(call_id);
        let bytes = send_sip(&self.sock, &leg.invite, sig);
        leg.retrans = Some(Retrans::new(Kind::Invite, bytes, sig, now));
        st.legs.push(leg);
        info!("appel {call_id:08x} : le poste SIP {name} sonne");
    }

    fn sip_response(&mut self, resp: &Message, now: Instant) {
        let (Some(call_id), Some((cseq, method)), Some(code)) = (resp.call_id(), resp.cseq(), resp.code()) else { return };
        let Some(st) = self.sip.as_mut() else { return };
        let Some(i) = st.legs.iter().position(|l| l.sip_call_id == call_id) else { return };
        let leg = &mut st.legs[i];
        match method {
            "INVITE" if leg.role == Role::Uac && cseq == leg.invite_cseq => self.sip_invite_response(i, resp, code, now),
            "BYE" | "CANCEL" if code >= 200 => {
                if leg.retrans.as_ref().is_some_and(|r| matches!(r.kind, Kind::Bye | Kind::Cancel)) {
                    leg.retrans = None;
                }
                if method == "BYE" {
                    st.legs.remove(i);
                }
            }
            _ => {}
        }
    }

    fn sip_invite_response(&mut self, i: usize, resp: &Message, code: u16, now: Instant) {
        let Some(st) = self.sip.as_mut() else { return };
        let sig = st.legs[i].sig;
        let ip = st.local_ip(sig);
        let port = st.listen.port();
        let leg = &mut st.legs[i];
        if code < 200 {
            if leg.state == State::Calling {
                leg.state = State::Proceeding;
                leg.retrans = None;
            }
            if leg.cancel_pending && leg.state == State::Proceeding {
                leg.cancel_pending = false;
                self.sip_send_cancel(i, now);
            }
            return;
        }
        if code >= 300 {
            // refus : ACK dans la même transaction, l'appel s'arrête
            let ack = invite_transaction_request(&leg.invite, "ACK", resp.header("To").unwrap_or_default());
            send_sip(&self.sock, &ack, sig);
            leg.retrans = None;
            let live = leg.active() && !leg.cancel_pending;
            let (call, session, name) = (leg.call.take(), leg.session, leg.name.clone());
            leg.state = State::Ending;
            leg.since = now;
            if matches!(code, 401 | 407) {
                warn!("le poste SIP {name} exige une authentification pour les appels entrants : désactivez « Auth INVITE » sur l'adaptateur");
            }
            if let Some(c) = call.filter(|_| live) {
                info!("appel {c:08x} : le poste SIP {name} refuse ({code} {})", message::reason(code));
                let reason = match code {
                    486 | 600 | 603 => HangupReason::Rejected,
                    408 | 480 => HangupReason::NoAnswer,
                    _ => HangupReason::Failure,
                };
                self.end_call(c, Some(session), reason);
            }
            return;
        }
        // 2xx : dialogue établi (ou 200 retransmis) ; toujours acquitter
        if let Some(to) = resp.header("To") {
            leg.remote_hdr = to.to_string();
        }
        if let Some(c) = resp.header("Contact") {
            leg.remote_target = message::addr_uri(c).to_string();
        }
        let ack = dialog_request(leg, "ACK", ip, port);
        send_sip(&self.sock, &ack, sig);
        leg.retrans = None;
        if leg.state == State::Confirmed {
            return; // 200 retransmis
        }
        let wanted = matches!(leg.state, State::Calling | State::Proceeding) && !leg.cancel_pending && leg.call.is_some();
        leg.state = State::Confirmed;
        leg.cancel_pending = false;
        if !wanted {
            // raccroché entre-temps au central : on termine aussitôt
            return self.sip_send_bye(i, now);
        }
        let answer = sdp::Session::parse(resp.body_str());
        let (session, call, name) = (leg.session, leg.call.unwrap_or_default(), leg.name.clone());
        match answer.as_ref().and_then(sdp::Session::choose) {
            Some((law, pt)) => {
                (leg.law, leg.pt, leg.numbering.pt) = (law, pt, pt);
                leg.rtp_dest = guess_rtp(answer.and_then(|a| a.audio), sig);
                leg.sdp_dest = leg.rtp_dest;
                info!("appel {call:08x} : le poste SIP {name} décroche");
                self.on_answer(session, call);
            }
            None => {
                warn!("appel {call:08x} : le poste SIP {name} décroche sans G.711 ; raccrochage");
                leg.call = None;
                self.sip_send_bye(i, now);
                self.end_call(call, Some(session), HangupReason::Failure);
            }
        }
    }

    fn sip_send_bye(&mut self, i: usize, now: Instant) {
        let Some(st) = self.sip.as_mut() else { return };
        let sig = st.legs[i].sig;
        let ip = st.local_ip(sig);
        let port = st.listen.port();
        let leg = &mut st.legs[i];
        let bye = dialog_request(leg, "BYE", ip, port);
        let bytes = send_sip(&self.sock, &bye, sig);
        leg.state = State::Ending;
        leg.since = now;
        leg.retrans = Some(Retrans::new(Kind::Bye, bytes, sig, now));
    }

    fn sip_send_cancel(&mut self, i: usize, now: Instant) {
        let Some(st) = self.sip.as_mut() else { return };
        let leg = &mut st.legs[i];
        let cancel = invite_transaction_request(&leg.invite, "CANCEL", leg.invite.header("To").unwrap_or_default());
        let bytes = send_sip(&self.sock, &cancel, leg.sig);
        leg.state = State::Ending;
        leg.since = now;
        leg.retrans = Some(Retrans::new(Kind::Cancel, bytes, leg.sig, now));
    }

    // ───────────────────────── messages du central ─────────────────────────

    /// Un message du central pour ce poste (à la place de `send_reliable`).
    pub(super) fn sip_deliver(&mut self, session: u32, msg: Msg) {
        let Some(st) = self.sip.as_mut() else { return };
        let Some(i) = st.legs.iter().position(|l| l.session == session && l.active()) else {
            if let Msg::Ring { call_id, from } = msg {
                self.sip_call_out(session, call_id, &from);
            }
            return;
        };
        let now = Instant::now();
        let leg = &mut st.legs[i];
        let incoming = leg.role == Role::Uas && leg.state == State::Incoming;
        match msg {
            Msg::Ringing { call_id } if incoming => {
                leg.call = Some(call_id);
                self.sip_uas_respond(i, 180, now);
            }
            Msg::Answer { call_id } if incoming => {
                leg.call = Some(call_id);
                self.sip_uas_respond(i, 200, now);
            }
            Msg::Busy { .. } if incoming => self.sip_uas_respond(i, 486, now),
            Msg::UnknownNumber { .. } if incoming => self.sip_uas_respond(i, 404, now),
            Msg::Hangup { call_id, reason } => {
                if leg.call.is_none_or(|c| c == call_id) {
                    self.sip_end_leg(i, reason, now);
                }
            }
            // déjà en ligne côté SIP (fin de dialogue en cours) : refusé
            Msg::Ring { call_id, .. } => self.end_call(call_id, Some(session), HangupReason::Rejected),
            _ => {}
        }
    }

    /// L'appel s'est terminé au central : on termine le dialogue SIP.
    fn sip_end_leg(&mut self, i: usize, reason: HangupReason, now: Instant) {
        let Some(st) = self.sip.as_mut() else { return };
        let leg = &mut st.legs[i];
        leg.call = None;
        match (leg.role, leg.state) {
            (Role::Uas, State::Incoming) => {
                let code = match reason {
                    HangupReason::Forbidden => 403,
                    HangupReason::Rejected => 603,
                    HangupReason::Shutdown | HangupReason::Failure => 503,
                    HangupReason::Normal | HangupReason::NoAnswer | HangupReason::PeerLost => 480,
                };
                self.sip_uas_respond(i, code, now);
            }
            // pas de BYE avant l'ACK du 200 (RFC 3261 §15)
            (Role::Uas, State::WaitAck) => leg.bye_pending = true,
            (_, State::Confirmed) => self.sip_send_bye(i, now),
            (Role::Uac, State::Calling) => leg.cancel_pending = true,
            (Role::Uac, State::Proceeding) => self.sip_send_cancel(i, now),
            _ => {}
        }
    }

    /// Le poste n'est plus enregistré : ses dialogues se terminent.
    pub(super) fn sip_forget(&mut self, session: u32) {
        let now = Instant::now();
        let Some(st) = self.sip.as_ref() else { return };
        let idx: Vec<usize> = st.legs.iter().enumerate().filter(|(_, l)| l.session == session && l.active()).map(|(i, _)| i).collect();
        for i in idx {
            self.sip_end_leg(i, HangupReason::PeerLost, now);
        }
    }

    /// Média d'un poste qui appelle un numéro spécial : (destination RTP,
    /// loi G.711, type de charge utile), pour le modem du central.
    pub(super) fn sip_media(&self, session: u32) -> Option<(SocketAddr, Codec, u8)> {
        let l = self.sip.as_ref()?.legs.iter().find(|l| l.session == session && l.active())?;
        Some((l.rtp_dest?, l.law, l.pt))
    }

    // ───────────────────────── audio ─────────────────────────

    /// Paquet RTP d'un poste : recoupé en trames, relayé comme l'audio d'un abonné.
    pub(super) fn on_rtp(&mut self, buf: &[u8], from: SocketAddr) {
        let Some((h, payload)) = rtp::parse(buf) else {
            self.counters.dropped_packets += 1;
            return;
        };
        let Some(st) = self.sip.as_mut() else { return };
        // Le flux d'un poste : d'abord sa source déjà connue, sinon la
        // destination devinée, sinon l'adresse IP de sa signalisation (NAT).
        let i = st
            .legs
            .iter()
            .position(|l| l.media_live() && l.latched && l.rtp_dest == Some(from))
            .or_else(|| st.legs.iter().position(|l| l.media_live() && !l.latched && l.rtp_dest == Some(from)))
            .or_else(|| st.legs.iter().position(|l| l.media_live() && !l.latched && l.sig.ip() == from.ip()));
        let Some(i) = i else {
            self.counters.dropped_packets += 1;
            return;
        };
        let leg = &mut st.legs[i];
        if !leg.latched {
            if leg.rtp_dest != Some(from) {
                debug!("poste SIP {} : audio reçu de {from}", leg.name);
            }
            leg.latched = true;
            leg.rtp_dest = Some(from);
        }
        if h.pt != leg.pt {
            return; // bruit de confort, DTMF (RFC 2833)… : ignorés
        }
        if payload.len() != FRAME_SAMPLES && !leg.warned_ptime {
            leg.warned_ptime = true;
            info!("poste SIP {} : paquets RTP de {} ms, recoupés en 20 ms (mieux : RTP Packet Size 0.020)", leg.name, payload.len() / 8);
        }
        let (Some(call_id), session, law) = (leg.call, leg.session, leg.law) else { return };
        let mut pkts = Vec::new();
        let now = Instant::now();
        // Avec un modem du central (FAI, 3615, ligne de test), c'est son fil
        // qui envoie le RTP au poste, déjà à la cadence du poste.
        leg.paced = !matches!(self.calls.get(&call_id).map(|c| &c.callee), Some(Callee::Modem { .. }));
        for (seq, bytes) in leg.reframer.push(h.ssrc, h.ts, payload) {
            // Son horloge donne la cadence de notre envoi (voir sip_pump)
            match leg.up_last {
                None => {
                    leg.tx_mark = seq.wrapping_sub(1);
                    leg.up_last = Some(seq);
                    leg.up_at = Some(now);
                }
                Some(l) if (seq.wrapping_sub(l) as i32) > 0 => {
                    leg.up_last = Some(seq);
                    leg.up_at = Some(now);
                }
                Some(_) => {}
            }
            let samples = rtp::decode_payload(law, &bytes);
            // les trames précédentes, si on les a, voyagent avec celle-ci
            let mut backup = Vec::new();
            for k in 1..=BACKUP_FRAMES as u32 {
                match leg.history.iter().find(|(s, _)| *s == seq.wrapping_sub(k)) {
                    Some((_, f)) => backup.push(f.clone()),
                    None => break,
                }
            }
            leg.history.push_front((seq, samples.clone()));
            leg.history.truncate(BACKUP_FRAMES + 2);
            pkts.push(Packet { codec: law, call_id, seq, timestamp: seq.wrapping_mul(FRAME_SAMPLES as u32), samples, backup });
        }

        for p in pkts {
            let Some(call) = self.calls.get(&call_id) else { return };
            if call.state != CallState::Active {
                self.counters.dropped_packets += 1;
                return;
            }
            let dir = if call.caller == session {
                0
            } else if matches!(call.callee, Callee::Client(s) if s == session) {
                1
            } else {
                return;
            };
            self.relay(call_id, dir, Audio::Decoded(p), from);
        }
        self.sip_pump(now);
    }

    /// Envoi vers les postes, à leur cadence : la trame du poste que nous
    /// devrions avoir reçue à cet instant est extrapolée de la dernière
    /// reçue (20 ms par trame, au plus [`UPLINK_STALE`]) ; on envoie la
    /// nôtre pour chacune. Un calage de son flux montant n'arrête pas le
    /// nôtre ; quand sa rafale arrive, l'extrapolation retombe sur ses pieds
    /// sans rien envoyer en double. Appelé à chaque tour de la boucle.
    pub(super) fn sip_pump(&mut self, now: Instant) {
        let Some(st) = self.sip.as_mut() else { return };
        for leg in st.legs.iter_mut() {
            if !leg.paced || !leg.media_live() {
                continue;
            }
            let (Some(last), Some(at), Some(dest)) = (leg.up_last, leg.up_at, leg.rtp_dest) else { continue };
            let since = now.saturating_duration_since(at);
            if since > UPLINK_STALE {
                continue;
            }
            let target = last.wrapping_add((since.as_millis() / 20) as u32);
            let mut due = target.wrapping_sub(leg.tx_mark) as i32;
            if due <= 0 {
                continue;
            }
            if due > MAX_CATCHUP {
                // très en retard (reprise du flux après un long silence) :
                // ces créneaux sont passés
                for _ in 0..due - MAX_CATCHUP {
                    if let Some(jb) = leg.tx_jb.as_mut() {
                        jb.pop_frame();
                    }
                    leg.tx_out = leg.tx_out.wrapping_add(1);
                }
                due = MAX_CATCHUP;
            }
            for k in 0..due {
                let frame = leg.tx_jb.as_mut().map_or_else(|| vec![0.0; FRAME_SAMPLES], |jb| jb.pop_frame().0);
                let n = leg.tx_out;
                leg.tx_out = n.wrapping_add(1);
                // un retard de plus de 2 trames : les plus anciennes sont
                // déjà passées chez lui, seules les dernières partent
                if due - k <= 2 {
                    let bytes = leg.numbering.packet(leg.law, n, n.wrapping_mul(FRAME_SAMPLES as u32), &frame);
                    if let Err(e) = self.sock.send_to(&bytes, dest) {
                        if !is_transient(&e) {
                            debug!("envoi RTP vers {dest} : {e}");
                        }
                    }
                }
            }
            leg.tx_mark = target;
        }
    }

    /// Une trame du central vers un poste : elle attend son créneau dans le
    /// tampon du poste (voir [`TX_DEPTH_APPLI_MS`]) ; c'est l'arrivée de
    /// ses propres trames qui la fait partir ([`Server::on_rtp`]). Les copies
    /// de redondance de l'appli comblent les trames perdues en route.
    pub(super) fn sip_send_audio(&mut self, session: u32, p: &Packet) {
        let Some(st) = self.sip.as_mut() else { return };
        let Some(leg) = st.legs.iter_mut().find(|l| l.session == session && l.media_live()) else { return };
        let Some(dest) = leg.rtp_dest else { return };
        let t = leg.since.elapsed().as_secs_f64();
        let jb = leg.tx_jb.get_or_insert_with(|| JitterBuffer::for_modem(TX_DEPTH_APPLI_MS));
        jb.push_with_backup(p.seq, p.timestamp, p.samples.clone(), &p.backup, t);
        // Le poste se tait (pas encore de RTP, ou détection de silence) :
        // rien ne cadence l'envoi, la trame part au rythme de l'autre bout.
        if leg.up_at.is_some_and(|at| at.elapsed() < UPLINK_STALE) {
            return;
        }
        let (frame, _) = jb.pop_frame();
        let n = leg.tx_out;
        leg.tx_out = n.wrapping_add(1);
        let bytes = leg.numbering.packet(leg.law, n, n.wrapping_mul(FRAME_SAMPLES as u32), &frame);
        if let Err(e) = self.sock.send_to(&bytes, dest) {
            if !is_transient(&e) {
                debug!("envoi RTP vers {dest} : {e}");
            }
        }
    }

    // ───────────────────────── temporisateurs ─────────────────────────

    pub(super) fn sip_housekeeping(&mut self, now: Instant) {
        let Some(st) = self.sip.as_mut() else { return };
        let mut expired = Vec::new();
        for leg in st.legs.iter_mut() {
            let Some(r) = leg.retrans.as_mut() else { continue };
            if now >= r.deadline {
                expired.push(leg.sip_call_id.clone());
            } else if now >= r.next {
                let _ = self.sock.send_to(&r.bytes, r.to);
                // l'INVITE double sans plafond (temporisateur A), le reste plafonne à T2
                r.interval = if r.kind == Kind::Invite { r.interval * 2 } else { (r.interval * 2).min(T2) };
                r.next = now + r.interval;
            }
        }
        for id in expired {
            self.sip_timeout(&id, now);
        }
        if let Some(st) = self.sip.as_mut() {
            st.legs.retain(|l| !(l.state == State::Ending && l.retrans.is_none() && now.duration_since(l.since) > TIMEOUT));
            st.recent.retain(|r| now.duration_since(r.at) < TIMEOUT);
        }
    }

    /// Une transaction n'a jamais été acquittée.
    fn sip_timeout(&mut self, sip_call_id: &str, now: Instant) {
        let Some(st) = self.sip.as_mut() else { return };
        let Some(i) = st.legs.iter().position(|l| l.sip_call_id == sip_call_id) else { return };
        let leg = &mut st.legs[i];
        let kind = leg.retrans.take().map(|r| r.kind);
        let (call, session, name) = (leg.call, leg.session, leg.name.clone());
        match kind {
            Some(Kind::Invite) => {
                st.legs.remove(i);
                if let Some(c) = call {
                    info!("appel {c:08x} : le poste SIP {name} ne répond pas");
                    self.end_call(c, Some(session), HangupReason::NoAnswer);
                }
            }
            Some(Kind::Ok) => {
                info!("le poste SIP {name} n'a pas acquitté le décroché : raccrochage");
                leg.call = None;
                self.sip_send_bye(i, now);
                if let Some(c) = call {
                    self.end_call(c, Some(session), HangupReason::PeerLost);
                }
            }
            Some(Kind::Reinvite) => {}
            _ => {
                st.legs.remove(i);
            }
        }
    }
}
