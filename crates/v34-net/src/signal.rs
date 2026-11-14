//! Signalisation du central téléphonique (messages de contrôle sur UDP).
//!
//! Les messages de contrôle partagent la socket UDP des paquets audio ; ils
//! s'en distinguent par leur « magic » : `"VC"` (contrôle) au lieu de `"V3"`
//! (audio, voir [`crate::packet`]).
//!
//! ## En-tête (12 octets, gros-boutiste)
//! ```text
//! 0      2     3      4          8          12
//! | "VC" | ver | type | session  |  msg_id  | corps… | [étiquette 16 o]
//! ```
//! - `session` : 0 avant l'authentification (REGISTER, CHALLENGE, AUTH),
//!   sinon identifiant de session attribué par le serveur dans WELCOME.
//! - `msg_id` : numéro du message chez l'émetteur (croissant, 0 pour ACK et
//!   les messages pré-authentification). Sert aux acquittements et à
//!   l'élimination des doublons / rejeux.
//! - étiquette : présente dès que `session != 0` : les 16 premiers octets de
//!   HMAC-SHA256(clé de session, en-tête ‖ corps).
//!
//! ## Authentification (défi-réponse, le mot de passe ne circule jamais)
//! ```text
//! client  → REGISTER(nom)               (rembourré à 96 octets : pas d'amplification)
//! serveur → CHALLENGE(nonce_s, itérations, sel)
//!           nonce_s sans état : horodatage + HMAC(clé serveur, adresse, nom)
//!           nom inconnu : sel factice stable (on ne révèle pas qui existe)
//! client  → AUTH(nom, nonce_s, nonce_c, preuve)     (preuve façon SCRAM, voir crate::auth)
//! serveur → WELCOME(numéro attribué, …) étiqueté avec la clé de session
//!           (prouve au client que le serveur connaît sa clé stockée)
//!        ou AUTH_FAIL(raison) : identifiants refusés (sans dire si c'est le
//!           nom ou le mot de passe), trop de tentatives depuis cette IP…
//! ```
//! Le numéro est ATTRIBUÉ par le serveur (annuaire de la configuration) :
//! un client ne peut pas en réclamer un.
//!
//! ## Messages authentifiés
//! | type | nom            | sens | corps                         |
//! |------|----------------|------|-------------------------------|
//! | 0x04 | WELCOME (OK)   | S→C  | n u8 numéro attribué, keepalive_s u16, expiry_s u16, nb u8 × (numéro spécial, service) |
//! | 0x10 | KEEPALIVE      | C→S  | —                             |
//! | 0x11 | ACK            | ↔    | msg_id acquitté u32           |
//! | 0x12 | UNREGISTER     | C→S  | —                             |
//! | 0x20 | DIAL           | C→S  | n u8, numéro                  |
//! | 0x21 | RINGING        | S→C  | call_id u32 (ça sonne en face)|
//! | 0x22 | RING           | S→C  | call_id u32, n u8, appelant   |
//! | 0x23 | ANSWER         | ↔    | call_id u32 (décroché)        |
//! | 0x24 | HANGUP         | ↔    | call_id u32, raison u8        |
//! | 0x25 | BUSY           | S→C  | n u8, numéro                  |
//! | 0x26 | UNKNOWN_NUMBER | S→C  | n u8, numéro                  |
//! | 0x30 | DIRECTORY_REQ  | C→S  | —                             |
//! | 0x31 | DIRECTORY      | S→C  | nb u8 × (nom, numéro, état u8 : 0 absent, 1 en ligne, 2 en communication) |
//!
//! Tous les messages authentifiés sauf ACK, WELCOME et l'annuaire sont
//! « fiables ». L'annuaire est redemandé régulièrement par le client : une
//! réponse perdue n'a aucune importance, et un central trop ancien qui ne
//! connaît pas DIRECTORY_REQ l'ignore sans faire croire au client qu'il a
//! disparu (pas d'acquittement attendu). Pour les messages fiables,
//! l'émetteur les répète (300 ms, puis ×2 jusqu'à 2 s, 8 essais) tant qu'il
//! n'a pas reçu l'ACK correspondant ; le récepteur acquitte toujours, mais
//! ne traite qu'une fois (voir [`Outbox`] et [`Dedup`]).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::crypto::{ct_eq, hmac_sha256};


pub const MAGIC: [u8; 2] = *b"VC";
pub const VERSION: u8 = 1;
pub const HEADER_LEN: usize = 12;
pub const TAG_LEN: usize = 16;
/// Taille minimale d'un REGISTER : au moins celle du CHALLENGE (ou de
/// l'AUTH_FAIL) renvoyé, pour que le serveur ne serve pas d'amplificateur.
pub const REGISTER_MIN_LEN: usize = 96;
pub const MAX_NAME_LEN: usize = 32;
pub const MAX_SALT_LEN: usize = 32;
pub const MAX_NUMBER_LEN: usize = 20;
pub const SERVER_NONCE_LEN: usize = 24;
pub const CLIENT_NONCE_LEN: usize = 16;

pub type SessionKey = [u8; 32];

const T_REGISTER: u8 = 0x01;
const T_CHALLENGE: u8 = 0x02;
const T_AUTH: u8 = 0x03;
const T_WELCOME: u8 = 0x04;
const T_AUTH_FAIL: u8 = 0x05;
const T_KEEPALIVE: u8 = 0x10;
const T_ACK: u8 = 0x11;
const T_UNREGISTER: u8 = 0x12;
const T_DIAL: u8 = 0x20;
const T_RINGING: u8 = 0x21;
const T_RING: u8 = 0x22;
const T_ANSWER: u8 = 0x23;
const T_HANGUP: u8 = 0x24;
const T_BUSY: u8 = 0x25;
const T_UNKNOWN: u8 = 0x26;
const T_DIRECTORY_REQ: u8 = 0x30;
const T_DIRECTORY: u8 = 0x31;
/// Taille maximale d'un DIRECTORY encodé (pas de fragmentation IP)
pub const MAX_DIRECTORY_BYTES: usize = 1200;

/// Une ligne de l'annuaire du central.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct DirEntry {
    pub name: String,
    pub number: String,
    pub status: Presence,
}

/// Présence d'un abonné.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Presence {
    /// Pas connecté au central
    Absent,
    /// Connecté, disponible
    Online,
    /// Connecté, en communication
    Busy,
}

impl Presence {
    fn code(self) -> u8 {
        match self {
            Presence::Absent => 0,
            Presence::Online => 1,
            Presence::Busy => 2,
        }
    }
    fn from_code(c: u8) -> Presence {
        match c {
            1 => Presence::Online,
            2 => Presence::Busy,
            _ => Presence::Absent,
        }
    }
}

/// Pourquoi un appel s'est terminé.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HangupReason {
    /// Raccrochage normal par l'autre partie
    Normal,
    /// Personne n'a décroché à temps
    NoAnswer,
    /// L'autre partie a disparu (keepalive expiré, injoignable)
    PeerLost,
    /// Refusé par le central (appel invalide, limite atteinte…)
    Rejected,
    /// Arrêt du central
    Shutdown,
    /// Échec côté modem (FAI) : pas de porteuse, login refusé…
    Failure,
    /// Service non autorisé pour cet abonné (accès Internet non activé)
    Forbidden,
}

impl HangupReason {
    fn code(self) -> u8 {
        match self {
            HangupReason::Normal => 0,
            HangupReason::NoAnswer => 1,
            HangupReason::PeerLost => 2,
            HangupReason::Rejected => 3,
            HangupReason::Shutdown => 4,
            HangupReason::Failure => 5,
            HangupReason::Forbidden => 6,
        }
    }

    fn from_code(c: u8) -> HangupReason {
        match c {
            0 => HangupReason::Normal,
            1 => HangupReason::NoAnswer,
            2 => HangupReason::PeerLost,
            3 => HangupReason::Rejected,
            4 => HangupReason::Shutdown,
            6 => HangupReason::Forbidden,
            _ => HangupReason::Failure,
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            HangupReason::Normal => "raccroché",
            HangupReason::NoAnswer => "pas de réponse",
            HangupReason::PeerLost => "correspondant perdu",
            HangupReason::Rejected => "appel refusé par le central",
            HangupReason::Shutdown => "arrêt du central",
            HangupReason::Failure => "échec de la liaison",
            HangupReason::Forbidden => "accès non autorisé pour ce compte",
        }
    }
}

/// Pourquoi l'enregistrement est refusé.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthFailReason {
    /// Nom inconnu ou mauvais mot de passe (volontairement indistincts)
    BadCredentials,
    /// Trop d'échecs récents depuis cette adresse IP : réessayer plus tard
    TooManyAttempts,
    /// Le central est plein
    ServerFull,
}

impl AuthFailReason {
    fn code(self) -> u8 {
        match self {
            AuthFailReason::BadCredentials => 1,
            AuthFailReason::TooManyAttempts => 2,
            AuthFailReason::ServerFull => 3,
        }
    }

    fn from_code(c: u8) -> AuthFailReason {
        match c {
            2 => AuthFailReason::TooManyAttempts,
            3 => AuthFailReason::ServerFull,
            _ => AuthFailReason::BadCredentials,
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            AuthFailReason::BadCredentials => "nom ou mot de passe incorrect",
            AuthFailReason::TooManyAttempts => "trop de tentatives, réessayez dans une minute",
            AuthFailReason::ServerFull => "central complet",
        }
    }
}

/// Un message de signalisation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    Register { name: String },
    Challenge { nonce: [u8; SERVER_NONCE_LEN], iterations: u32, salt: Vec<u8> },
    Auth { name: String, server_nonce: [u8; SERVER_NONCE_LEN], client_nonce: [u8; CLIENT_NONCE_LEN], proof: [u8; 32] },
    AuthFail { reason: AuthFailReason },
    /// `services` : numéros spéciaux (numéro, service : "fai", "echo"…)
    Welcome { number: String, keepalive_s: u16, expiry_s: u16, services: Vec<(String, String)> },
    Keepalive,
    Ack { msg_id: u32 },
    Unregister,
    Dial { number: String },
    Ringing { call_id: u32 },
    Ring { call_id: u32, from: String },
    Answer { call_id: u32 },
    Hangup { call_id: u32, reason: HangupReason },
    Busy { number: String },
    UnknownNumber { number: String },
    DirectoryReq,
    Directory { entries: Vec<DirEntry> },
}

impl Msg {
    pub fn type_code(&self) -> u8 {
        match self {
            Msg::Register { .. } => T_REGISTER,
            Msg::Challenge { .. } => T_CHALLENGE,
            Msg::Auth { .. } => T_AUTH,
            Msg::AuthFail { .. } => T_AUTH_FAIL,
            Msg::Welcome { .. } => T_WELCOME,
            Msg::Keepalive => T_KEEPALIVE,
            Msg::Ack { .. } => T_ACK,
            Msg::Unregister => T_UNREGISTER,
            Msg::Dial { .. } => T_DIAL,
            Msg::Ringing { .. } => T_RINGING,
            Msg::Ring { .. } => T_RING,
            Msg::Answer { .. } => T_ANSWER,
            Msg::Hangup { .. } => T_HANGUP,
            Msg::Busy { .. } => T_BUSY,
            Msg::UnknownNumber { .. } => T_UNKNOWN,
            Msg::DirectoryReq => T_DIRECTORY_REQ,
            Msg::Directory { .. } => T_DIRECTORY,
        }
    }

    /// Message échangé avant l'authentification (session 0, sans étiquette).
    pub fn is_pre_auth(&self) -> bool {
        is_pre_auth_type(self.type_code())
    }

    /// Message à répéter jusqu'à acquittement.
    pub fn is_reliable(&self) -> bool {
        !self.is_pre_auth() && !matches!(self, Msg::Ack { .. } | Msg::Welcome { .. } | Msg::DirectoryReq | Msg::Directory { .. })
    }

    pub fn name(&self) -> &'static str {
        match self {
            Msg::Register { .. } => "REGISTER",
            Msg::Challenge { .. } => "CHALLENGE",
            Msg::Auth { .. } => "AUTH",
            Msg::AuthFail { .. } => "AUTH_FAIL",
            Msg::Welcome { .. } => "WELCOME",
            Msg::Keepalive => "KEEPALIVE",
            Msg::Ack { .. } => "ACK",
            Msg::Unregister => "UNREGISTER",
            Msg::Dial { .. } => "DIAL",
            Msg::Ringing { .. } => "RINGING",
            Msg::Ring { .. } => "RING",
            Msg::Answer { .. } => "ANSWER",
            Msg::Hangup { .. } => "HANGUP",
            Msg::Busy { .. } => "BUSY",
            Msg::UnknownNumber { .. } => "UNKNOWN_NUMBER",
            Msg::DirectoryReq => "DIRECTORY_REQ",
            Msg::Directory { .. } => "DIRECTORY",
        }
    }
}

fn is_pre_auth_type(t: u8) -> bool {
    matches!(t, T_REGISTER | T_CHALLENGE | T_AUTH | T_AUTH_FAIL)
}

/// Message + champs d'en-tête.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub session: u32,
    pub msg_id: u32,
    pub msg: Msg,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignalError {
    /// Pas un message de contrôle (mauvais magic / version)
    NotControl,
    /// Message tronqué ou champ invalide
    Malformed,
    /// Étiquette d'authentification absente ou fausse
    BadTag,
    /// Session non nulle sur un message pré-auth, ou l'inverse
    BadSession,
}

/// Vrai si le datagramme commence comme un message de contrôle.
pub fn is_control(buf: &[u8]) -> bool {
    buf.len() >= HEADER_LEN && buf[0..2] == MAGIC
}

/// Lit le champ `session` sans rien vérifier (pour choisir la clé).
pub fn peek_session(buf: &[u8]) -> Option<u32> {
    if !is_control(buf) || buf[2] != VERSION {
        return None;
    }
    Some(u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]))
}

/// Numéro de téléphone valide : 1 à 20 chiffres.
pub fn valid_number(s: &str) -> bool {
    !s.is_empty() && s.len() <= MAX_NUMBER_LEN && s.bytes().all(|b| b.is_ascii_digit())
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    let n = b.len().min(255);
    out.push(n as u8);
    out.extend_from_slice(&b[..n]);
}

/// Encode un message. `key` est obligatoire pour les messages authentifiés
/// (session ≠ 0) : l'étiquette est alors ajoutée.
pub fn encode(frame: &Frame, key: Option<&SessionKey>) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(&MAGIC);
    out.push(VERSION);
    out.push(frame.msg.type_code());
    out.extend_from_slice(&frame.session.to_be_bytes());
    out.extend_from_slice(&frame.msg_id.to_be_bytes());
    match &frame.msg {
        Msg::Register { name } => {
            put_str(&mut out, name);
            if out.len() < REGISTER_MIN_LEN {
                out.resize(REGISTER_MIN_LEN, 0);
            }
        }
        Msg::Challenge { nonce, iterations, salt } => {
            out.extend_from_slice(nonce);
            out.extend_from_slice(&iterations.to_be_bytes());
            let n = salt.len().min(MAX_SALT_LEN);
            out.push(n as u8);
            out.extend_from_slice(&salt[..n]);
        }
        Msg::Auth { name, server_nonce, client_nonce, proof } => {
            put_str(&mut out, name);
            out.extend_from_slice(server_nonce);
            out.extend_from_slice(client_nonce);
            out.extend_from_slice(proof);
        }
        Msg::AuthFail { reason } => out.push(reason.code()),
        Msg::Welcome { number, keepalive_s, expiry_s, services } => {
            put_str(&mut out, number);
            out.extend_from_slice(&keepalive_s.to_be_bytes());
            out.extend_from_slice(&expiry_s.to_be_bytes());
            let n = services.len().min(32);
            out.push(n as u8);
            for (num, kind) in &services[..n] {
                put_str(&mut out, num);
                put_str(&mut out, kind);
            }
        }
        Msg::Keepalive | Msg::Unregister | Msg::DirectoryReq => {}
        Msg::Directory { entries } => {
            // Autant d'entrées que la taille maximale le permet (la place de
            // l'étiquette est réservée).
            let count_at = out.len();
            out.push(0);
            let mut n = 0u8;
            for e in entries.iter().take(255) {
                let len = 1 + e.name.len().min(255) + 1 + e.number.len().min(255) + 1;
                if out.len() + len + TAG_LEN > MAX_DIRECTORY_BYTES {
                    break;
                }
                put_str(&mut out, &e.name);
                put_str(&mut out, &e.number);
                out.push(e.status.code());
                n += 1;
            }
            out[count_at] = n;
        }
        Msg::Ack { msg_id } => out.extend_from_slice(&msg_id.to_be_bytes()),
        Msg::Dial { number } | Msg::Busy { number } | Msg::UnknownNumber { number } => put_str(&mut out, number),
        Msg::Ringing { call_id } | Msg::Answer { call_id } => out.extend_from_slice(&call_id.to_be_bytes()),
        Msg::Ring { call_id, from } => {
            out.extend_from_slice(&call_id.to_be_bytes());
            put_str(&mut out, from);
        }
        Msg::Hangup { call_id, reason } => {
            out.extend_from_slice(&call_id.to_be_bytes());
            out.push(reason.code());
        }
    }
    if frame.session != 0 {
        if let Some(k) = key {
            let tag = hmac_sha256(k, &[&out]);
            out.extend_from_slice(&tag[..TAG_LEN]);
        }
    }
    out
}

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], SignalError> {
        if self.i + n > self.b.len() {
            return Err(SignalError::Malformed);
        }
        let s = &self.b[self.i..self.i + n];
        self.i += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, SignalError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, SignalError> {
        let s = self.take(2)?;
        Ok(u16::from_be_bytes([s[0], s[1]]))
    }
    fn u32(&mut self) -> Result<u32, SignalError> {
        let s = self.take(4)?;
        Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn arr<const N: usize>(&mut self) -> Result<[u8; N], SignalError> {
        let mut a = [0u8; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }
    /// Chaîne préfixée par sa longueur (texte libre ≤ 255 octets).
    fn string(&mut self) -> Result<String, SignalError> {
        let n = self.u8()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| SignalError::Malformed)
    }
    /// Numéro de téléphone (1 à 20 chiffres).
    fn number(&mut self) -> Result<String, SignalError> {
        let s = self.string()?;
        if valid_number(&s) {
            Ok(s)
        } else {
            Err(SignalError::Malformed)
        }
    }
    /// Nom d'abonné (voir `auth::valid_name`).
    fn name(&mut self) -> Result<String, SignalError> {
        let s = self.string()?;
        if crate::auth::valid_name(&s) {
            Ok(s)
        } else {
            Err(SignalError::Malformed)
        }
    }
}

/// Décode et vérifie un message. Pour un message authentifié, `key` doit
/// être la clé de la session annoncée dans l'en-tête.
pub fn decode(buf: &[u8], key: Option<&SessionKey>) -> Result<Frame, SignalError> {
    if !is_control(buf) || buf[2] != VERSION {
        return Err(SignalError::NotControl);
    }
    let t = buf[3];
    let session = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
    let msg_id = u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]);
    let pre = is_pre_auth_type(t);
    if pre != (session == 0) {
        return Err(SignalError::BadSession);
    }
    let body_end = if pre {
        buf.len()
    } else {
        if buf.len() < HEADER_LEN + TAG_LEN {
            return Err(SignalError::Malformed);
        }
        let end = buf.len() - TAG_LEN;
        let k = key.ok_or(SignalError::BadTag)?;
        let tag = hmac_sha256(k, &[&buf[..end]]);
        if !ct_eq(&tag[..TAG_LEN], &buf[end..]) {
            return Err(SignalError::BadTag);
        }
        end
    };
    let mut r = Reader { b: &buf[..body_end], i: HEADER_LEN };
    let msg = match t {
        T_REGISTER => {
            let name = r.name()?;
            if buf.len() < REGISTER_MIN_LEN {
                return Err(SignalError::Malformed);
            }
            // le reste est du rembourrage
            r.i = r.b.len();
            Msg::Register { name }
        }
        T_CHALLENGE => {
            let nonce = r.arr()?;
            let iterations = r.u32()?;
            let n = r.u8()? as usize;
            if n == 0 || n > MAX_SALT_LEN {
                return Err(SignalError::Malformed);
            }
            Msg::Challenge { nonce, iterations, salt: r.take(n)?.to_vec() }
        }
        T_AUTH => Msg::Auth { name: r.name()?, server_nonce: r.arr()?, client_nonce: r.arr()?, proof: r.arr()? },
        T_AUTH_FAIL => Msg::AuthFail { reason: AuthFailReason::from_code(r.u8()?) },
        T_WELCOME => {
            let number = r.number()?;
            let keepalive_s = r.u16()?;
            let expiry_s = r.u16()?;
            let n = r.u8()? as usize;
            let mut services = Vec::with_capacity(n);
            for _ in 0..n {
                services.push((r.number()?, r.string()?));
            }
            Msg::Welcome { number, keepalive_s, expiry_s, services }
        }
        T_KEEPALIVE => Msg::Keepalive,
        T_ACK => Msg::Ack { msg_id: r.u32()? },
        T_UNREGISTER => Msg::Unregister,
        T_DIAL => Msg::Dial { number: r.number()? },
        T_RINGING => Msg::Ringing { call_id: r.u32()? },
        T_RING => Msg::Ring { call_id: r.u32()?, from: r.number()? },
        T_ANSWER => Msg::Answer { call_id: r.u32()? },
        T_HANGUP => Msg::Hangup { call_id: r.u32()?, reason: HangupReason::from_code(r.u8()?) },
        T_BUSY => Msg::Busy { number: r.number()? },
        T_UNKNOWN => Msg::UnknownNumber { number: r.number()? },
        T_DIRECTORY_REQ => Msg::DirectoryReq,
        T_DIRECTORY => {
            let n = r.u8()? as usize;
            let mut entries = Vec::with_capacity(n);
            for _ in 0..n {
                entries.push(DirEntry { name: r.name()?, number: r.number()?, status: Presence::from_code(r.u8()?) });
            }
            Msg::Directory { entries }
        }
        _ => return Err(SignalError::Malformed),
    };
    if r.i != r.b.len() {
        return Err(SignalError::Malformed);
    }
    Ok(Frame { session, msg_id, msg })
}

// ───────────────────────── fiabilité ─────────────────────────

/// Premier délai de retransmission.
pub const RETRY_FIRST: Duration = Duration::from_millis(300);
/// Délai maximal entre deux retransmissions.
pub const RETRY_MAX: Duration = Duration::from_secs(2);
/// Nombre total d'émissions avant abandon (≈ 10 s).
pub const MAX_TRIES: u32 = 8;

struct Pending {
    msg_id: u32,
    bytes: Vec<u8>,
    next_at: Instant,
    delay: Duration,
    tries: u32,
}

/// Messages fiables en attente d'acquittement.
#[derive(Default)]
pub struct Outbox {
    next_id: u32,
    pending: Vec<Pending>,
}

/// Résultat d'un passage des temporisateurs.
#[derive(Default, Debug)]
pub struct OutboxPoll {
    /// Datagrammes à réémettre
    pub resend: Vec<Vec<u8>>,
    /// Messages abandonnés (jamais acquittés)
    pub failed: Vec<u32>,
}

impl Outbox {
    pub fn new() -> Self {
        Self { next_id: 1, pending: Vec::new() }
    }

    /// Numéro du prochain message (jamais 0).
    pub fn alloc_id(&mut self) -> u32 {
        if self.next_id == 0 {
            self.next_id = 1;
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    /// Suit un message qui vient d'être émis une première fois.
    pub fn track(&mut self, msg_id: u32, bytes: Vec<u8>, now: Instant) {
        self.pending.push(Pending { msg_id, bytes, next_at: now + RETRY_FIRST, delay: RETRY_FIRST, tries: 1 });
    }

    /// Acquittement reçu. Renvoie vrai si le message était en attente.
    pub fn ack(&mut self, msg_id: u32) -> bool {
        let before = self.pending.len();
        self.pending.retain(|p| p.msg_id != msg_id);
        self.pending.len() != before
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub fn clear(&mut self) {
        self.pending.clear();
    }

    pub fn poll(&mut self, now: Instant) -> OutboxPoll {
        let mut res = OutboxPoll::default();
        self.pending.retain_mut(|p| {
            if now < p.next_at {
                return true;
            }
            if p.tries >= MAX_TRIES {
                res.failed.push(p.msg_id);
                return false;
            }
            p.tries += 1;
            p.delay = (p.delay * 2).min(RETRY_MAX);
            p.next_at = now + p.delay;
            res.resend.push(p.bytes.clone());
            true
        });
        res
    }
}

/// Verdict sur un `msg_id` reçu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seen {
    New,
    /// Déjà traité (retransmission) : acquitter sans retraiter
    Duplicate,
    /// Trop ancien (rejeu probable) : ignorer
    Stale,
}

const DEDUP_WINDOW: usize = 256;

/// Élimination des doublons et des rejeux, par session.
#[derive(Default)]
pub struct Dedup {
    recent: VecDeque<u32>,
    highest: Option<u32>,
}

impl Dedup {
    pub fn check(&mut self, id: u32) -> Seen {
        if self.recent.contains(&id) {
            return Seen::Duplicate;
        }
        if let Some(h) = self.highest {
            let behind = h.wrapping_sub(id) as i32;
            if behind > DEDUP_WINDOW as i32 {
                return Seen::Stale;
            }
            if behind < 0 {
                self.highest = Some(id);
            }
        } else {
            self.highest = Some(id);
        }
        self.recent.push_back(id);
        if self.recent.len() > DEDUP_WINDOW {
            self.recent.pop_front();
        }
        Seen::New
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_messages() -> Vec<Msg> {
        vec![
            Msg::Keepalive,
            Msg::Welcome {
                number: "0142424242".into(),
                keepalive_s: 15,
                expiry_s: 60,
                services: vec![("0836656565".into(), "fai".into()), ("0800000000".into(), "echo".into())],
            },
            Msg::Welcome { number: "1".into(), keepalive_s: 15, expiry_s: 60, services: vec![] },
            Msg::Ack { msg_id: 77 },
            Msg::Unregister,
            Msg::Dial { number: "0142424242".into() },
            Msg::Ringing { call_id: 0xCAFE_0001 },
            Msg::Ring { call_id: 9, from: "0612345678".into() },
            Msg::Answer { call_id: 9 },
            Msg::Hangup { call_id: 9, reason: HangupReason::NoAnswer },
            Msg::Busy { number: "1".into() },
            Msg::UnknownNumber { number: "123".into() },
            Msg::DirectoryReq,
            Msg::Directory {
                entries: vec![
                    DirEntry { name: "raziel".into(), number: "0142424242".into(), status: Presence::Online },
                    DirEntry { name: "gaston".into(), number: "0612345678".into(), status: Presence::Busy },
                    DirEntry { name: "lagaffe".into(), number: "0700000000".into(), status: Presence::Absent },
                ],
            },
        ]
    }

    #[test]
    fn annuaire_tronque_sans_fragmentation() {
        let key = [7u8; 32];
        let entries: Vec<DirEntry> = (0..200)
            .map(|i| DirEntry { name: format!("abonne-numero-{i:03}"), number: format!("0100000{i:03}"), status: Presence::Online })
            .collect();
        let f = Frame { session: 1, msg_id: 0, msg: Msg::Directory { entries: entries.clone() } };
        let bytes = encode(&f, Some(&key));
        assert!(bytes.len() <= MAX_DIRECTORY_BYTES, "{} octets", bytes.len());
        let Ok(Frame { msg: Msg::Directory { entries: got }, .. }) = decode(&bytes, Some(&key)) else { panic!("annuaire illisible") };
        assert!(got.len() > 20 && got.len() < 200);
        assert_eq!(got[..], entries[..got.len()]);
        assert!(!Msg::DirectoryReq.is_reliable() && !f.msg.is_reliable());
    }

    #[test]
    fn aller_retour_messages_authentifies() {
        let key = [7u8; 32];
        for (i, msg) in all_messages().into_iter().enumerate() {
            let f = Frame { session: 0x1234_5678, msg_id: i as u32 + 1, msg };
            let bytes = encode(&f, Some(&key));
            assert!(is_control(&bytes));
            assert_eq!(peek_session(&bytes), Some(0x1234_5678));
            assert_eq!(decode(&bytes, Some(&key)), Ok(f.clone()), "{}", f.msg.name());
            // mauvaise clé → refusé
            assert_eq!(decode(&bytes, Some(&[8u8; 32])), Err(SignalError::BadTag));
            assert_eq!(decode(&bytes, None), Err(SignalError::BadTag));
            // un bit modifié → refusé
            let mut bad = bytes.clone();
            let pos = HEADER_LEN.min(bad.len() - TAG_LEN - 1);
            bad[pos] ^= 1;
            assert!(decode(&bad, Some(&key)).is_err());
        }
    }

    #[test]
    fn aller_retour_pre_auth() {
        let reg = Frame { session: 0, msg_id: 0, msg: Msg::Register { name: "gaston".into() } };
        let b = encode(&reg, None);
        assert_eq!(b.len(), REGISTER_MIN_LEN);
        assert_eq!(decode(&b, None), Ok(reg));

        // le plus gros CHALLENGE possible tient dans un REGISTER minimal
        let chal = Frame {
            session: 0,
            msg_id: 0,
            msg: Msg::Challenge { nonce: [3; SERVER_NONCE_LEN], iterations: 100_000, salt: vec![7; MAX_SALT_LEN] },
        };
        let cb = encode(&chal, None);
        assert!(cb.len() <= REGISTER_MIN_LEN, "pas d'amplification");
        assert_eq!(decode(&cb, None), Ok(chal));

        let auth = Frame {
            session: 0,
            msg_id: 0,
            msg: Msg::Auth {
                name: "jean-paul".into(),
                server_nonce: [1; SERVER_NONCE_LEN],
                client_nonce: [2; CLIENT_NONCE_LEN],
                proof: [9; 32],
            },
        };
        assert_eq!(decode(&encode(&auth, None), None), Ok(auth));

        for reason in [AuthFailReason::BadCredentials, AuthFailReason::TooManyAttempts, AuthFailReason::ServerFull] {
            let f = Frame { session: 0, msg_id: 0, msg: Msg::AuthFail { reason } };
            let b = encode(&f, None);
            assert!(b.len() < REGISTER_MIN_LEN);
            assert_eq!(decode(&b, None), Ok(f));
        }
    }

    #[test]
    fn rejets() {
        assert_eq!(decode(b"V3\x01\x08xxxxxxxxxxxx", None), Err(SignalError::NotControl));
        // REGISTER trop court (pas de rembourrage) → refusé
        let mut b = encode(&Frame { session: 0, msg_id: 0, msg: Msg::Register { name: "a".into() } }, None);
        b.truncate(20);
        assert_eq!(decode(&b, None), Err(SignalError::Malformed));
        // nom invalide
        let b = encode(&Frame { session: 0, msg_id: 0, msg: Msg::Register { name: "a b".into() } }, None);
        assert_eq!(decode(&b, None), Err(SignalError::Malformed));
        // numéro non numérique
        let b = encode(&Frame { session: 5, msg_id: 1, msg: Msg::Dial { number: "12a".into() } }, Some(&[0; 32]));
        assert_eq!(decode(&b, Some(&[0; 32])), Err(SignalError::Malformed));
        // message authentifié avec session 0 → refusé
        let b = encode(&Frame { session: 0, msg_id: 1, msg: Msg::Keepalive }, None);
        assert_eq!(decode(&b, None), Err(SignalError::BadSession));
        // message pré-auth avec session ≠ 0 → refusé
        let b = encode(
            &Frame { session: 3, msg_id: 0, msg: Msg::Challenge { nonce: [0; 24], iterations: 1000, salt: vec![1] } },
            Some(&[0; 32]),
        );
        assert_eq!(decode(&b, Some(&[0; 32])), Err(SignalError::BadSession));
        // type inconnu
        let mut b = encode(&Frame { session: 5, msg_id: 1, msg: Msg::Keepalive }, Some(&[0; 32]));
        b[3] = 0x7F;
        assert!(decode(&b, Some(&[0; 32])).is_err());
    }

    #[test]
    fn retransmissions() {
        let t0 = Instant::now();
        let mut ob = Outbox::new();
        let a = ob.alloc_id();
        let b = ob.alloc_id();
        assert_ne!(a, b);
        ob.track(a, vec![1], t0);
        ob.track(b, vec![2], t0);
        assert!(ob.poll(t0).resend.is_empty());
        let p = ob.poll(t0 + RETRY_FIRST);
        assert_eq!(p.resend, vec![vec![1], vec![2]]);
        assert!(ob.ack(a));
        assert!(!ob.ack(a));
        // sans ACK, b finit par être abandonné
        let mut t = t0;
        let mut failed = Vec::new();
        for _ in 0..40 {
            t += Duration::from_secs(1);
            failed.extend(ob.poll(t).failed);
        }
        assert_eq!(failed, vec![b]);
        assert!(ob.is_empty());
    }

    #[test]
    fn doublons_et_rejeux() {
        let mut d = Dedup::default();
        assert_eq!(d.check(1), Seen::New);
        assert_eq!(d.check(2), Seen::New);
        assert_eq!(d.check(1), Seen::Duplicate);
        for i in 3..1000 {
            assert_eq!(d.check(i), Seen::New);
        }
        assert_eq!(d.check(5), Seen::Stale);
        assert_eq!(d.check(999), Seen::Duplicate);
    }
}
