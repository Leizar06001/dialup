//! Bout « client » du tunnel : proxy local SOCKS5 / HTTP CONNECT.
//!
//! Le moteur tourne dans son propre thread et ne connaît du modem que deux
//! canaux d'octets : `from_modem` (octets reçus) et `to_modem` (octets à
//! émettre). Il peut donc être relié à n'importe quoi : un modem réel (via
//! [`super::ModemPipe`]), ou directement au bout serveur dans les tests.
//!
//! Sur le port d'écoute (127.0.0.1:1080 par défaut) on accepte :
//! - SOCKS5 sans authentification, commande CONNECT (IPv4, IPv6, nom de
//!   domaine : la résolution DNS se fait côté serveur, comme `socks5h`) ;
//! - HTTP `CONNECT hôte:port` (proxy HTTPS des navigateurs).
//!
//! Chaque connexion acceptée devient un flux du tunnel : OPEN → OPEN_OK
//! (réponse de succès au client local) ou OPEN_FAIL (réponse d'échec).

use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use super::mux::{Mux, StreamEnd};
use super::progress::{Meter, Progress};
use super::proto::{Decoded, FailCode, Frame, LOGIN_FAILED, LOGIN_PROMPT, PASSWORD_PROMPT, TUNNEL_MARKER, VERSION};

/// Adresse par défaut du proxy local.
pub const DEFAULT_PROXY: &str = "127.0.0.1:1080";
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClientConfig {
    /// Identifiant envoyé à l'invite `login:` du serveur
    pub login: Option<String>,
    /// Mot de passe envoyé à l'invite `password:`
    pub password: Option<String>,
}

/// Ce qui se passe dans le tunnel (pour l'affichage).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TunnelEvent {
    /// Texte reçu du serveur avant le tunnel (bannière, invites…)
    Text(String),
    /// Tunnel établi (HELLO du serveur reçu)
    Ready { max_streams: u16 },
    Opened { id: u16, target: String },
    OpenFailed { id: u16, target: String, reason: String },
    Closed { id: u16 },
    /// Erreur de transmission détectée : tous les flux ont été coupés
    Corruption,
    /// Fin du moteur
    Ended { reason: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProxyKind {
    Socks5,
    HttpConnect,
}

struct Request {
    sock: TcpStream,
    kind: ProxyKind,
    host: String,
    port: u16,
}

impl Request {
    fn target(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// Proxy local + moteur de tunnel (thread).
pub struct TunnelClient {
    local: SocketAddr,
    stop: Arc<AtomicBool>,
    events: Receiver<TunnelEvent>,
    progress: Arc<Mutex<Progress>>,
    join: Option<JoinHandle<()>>,
}

impl TunnelClient {
    /// Démarre le moteur sur un port d'écoute déjà ouvert (on l'ouvre avant
    /// d'appeler, pour signaler tout de suite un port déjà pris).
    pub fn spawn(
        listener: TcpListener,
        cfg: ClientConfig,
        from_modem: Receiver<Vec<u8>>,
        to_modem: SyncSender<Vec<u8>>,
    ) -> io::Result<TunnelClient> {
        listener.set_nonblocking(true)?;
        let local = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let (ev_tx, events) = mpsc::channel();
        let st = stop.clone();
        let progress = Arc::new(Mutex::new(Progress::default()));
        let pr = progress.clone();
        let join = std::thread::Builder::new().name("tunnel-client".into()).spawn(move || {
            let mut e = Engine::new(listener, cfg, from_modem, to_modem, ev_tx, st, pr);
            let reason = e.run();
            e.mux.reset_all(false);
            let _ = e.events.send(TunnelEvent::Ended { reason });
        })?;
        Ok(TunnelClient { local, stop, events, progress, join: Some(join) })
    }

    /// Adresse réelle du proxy local.
    pub fn local_addr(&self) -> SocketAddr {
        self.local
    }

    /// Avancement des chargements (barre d'état du navigateur).
    pub fn progress(&self) -> Progress {
        self.progress.lock().map(|p| p.clone()).unwrap_or_default()
    }

    pub fn try_event(&self) -> Option<TunnelEvent> {
        self.events.try_recv().ok()
    }

    /// Vrai quand le moteur s'est arrêté.
    pub fn is_finished(&self) -> bool {
        self.join.as_ref().is_none_or(|j| j.is_finished())
    }

    /// Arrête le moteur et ferme toutes les connexions locales.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for TunnelClient {
    fn drop(&mut self) {
        self.stop();
    }
}

// ───────────────────────── phase de login ─────────────────────────

/// Automate de réponse aux invites `login:` / `password:`.
#[derive(Default)]
pub struct LoginClient {
    login: Option<String>,
    password: Option<String>,
    acc: Vec<u8>,
}

/// Action demandée par [`LoginClient::feed`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoginStep {
    /// Octets à envoyer au serveur
    Send(Vec<u8>),
    /// Texte reçu, à afficher
    Text(String),
    /// Tunnel ouvert ; octets déjà reçus qui appartiennent au tunnel
    Done(Vec<u8>),
    Failed(String),
}

impl LoginClient {
    pub fn new(login: Option<String>, password: Option<String>) -> Self {
        Self { login, password, acc: Vec::new() }
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<LoginStep> {
        let mut steps = Vec::new();
        self.acc.extend_from_slice(bytes);
        let marker = TUNNEL_MARKER.as_bytes();
        if let Some(i) = find(&self.acc, marker) {
            let text = String::from_utf8_lossy(&self.acc[..i]).into_owned();
            if !text.is_empty() {
                steps.push(LoginStep::Text(text));
            }
            let rest = self.acc[i + marker.len()..].to_vec();
            self.acc.clear();
            steps.push(LoginStep::Done(rest));
            return steps;
        }
        if find(&self.acc, LOGIN_FAILED.as_bytes()).is_some() {
            steps.push(LoginStep::Text(String::from_utf8_lossy(&self.acc).into_owned()));
            steps.push(LoginStep::Failed("identifiants refusés par le serveur (Login incorrect)".into()));
            self.acc.clear();
            return steps;
        }
        for (prompt, value, what) in [
            (LOGIN_PROMPT, self.login.clone(), "identifiant"),
            (PASSWORD_PROMPT, self.password.clone(), "mot de passe"),
        ] {
            if self.acc.ends_with(prompt.as_bytes()) {
                steps.push(LoginStep::Text(String::from_utf8_lossy(&self.acc).into_owned()));
                self.acc.clear();
                match value {
                    Some(v) => {
                        let mut b = v.into_bytes();
                        b.push(b'\r');
                        steps.push(LoginStep::Send(b));
                    }
                    None => steps.push(LoginStep::Failed(format!("le serveur demande un {what} (--login / --password)"))),
                }
                return steps;
            }
        }
        // Texte sans invite : on l'affiche par morceaux, en gardant de quoi
        // reconnaître un marqueur coupé en deux.
        if self.acc.len() > 4096 {
            let keep = marker.len();
            let cut = self.acc.len() - keep;
            steps.push(LoginStep::Text(String::from_utf8_lossy(&self.acc[..cut]).into_owned()));
            self.acc.drain(..cut);
        }
        steps
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

// ───────────────────────── moteur ─────────────────────────

enum Phase {
    Login(LoginClient),
    Tunnel,
}

struct Engine {
    listener: TcpListener,
    from_modem: Receiver<Vec<u8>>,
    to_modem: SyncSender<Vec<u8>>,
    events: Sender<TunnelEvent>,
    stop: Arc<AtomicBool>,
    phase: Phase,
    mux: Mux,
    hs_tx: Sender<io::Result<Request>>,
    hs_rx: Receiver<io::Result<Request>>,
    /// Demandes en attente de la fin du login
    waiting: VecDeque<Request>,
    /// OPEN envoyés, en attente de OPEN_OK / OPEN_FAIL
    opening: HashMap<u16, Request>,
    next_id: u16,
    max_streams: u16,
    meter: Meter,
    /// Instantané du compteur partagé avec [`TunnelClient::progress`]
    progress: Arc<Mutex<Progress>>,
    /// Le compteur a changé depuis le dernier instantané
    dirty: bool,
}

impl Engine {
    fn new(
        listener: TcpListener,
        cfg: ClientConfig,
        from_modem: Receiver<Vec<u8>>,
        to_modem: SyncSender<Vec<u8>>,
        events: Sender<TunnelEvent>,
        stop: Arc<AtomicBool>,
        progress: Arc<Mutex<Progress>>,
    ) -> Self {
        let (hs_tx, hs_rx) = mpsc::channel();
        Self {
            listener,
            from_modem,
            to_modem,
            events,
            stop,
            phase: Phase::Login(LoginClient::new(cfg.login, cfg.password)),
            mux: Mux::new(),
            hs_tx,
            hs_rx,
            waiting: VecDeque::new(),
            opening: HashMap::new(),
            next_id: 0,
            max_streams: 64,
            meter: Meter::new(),
            progress,
            dirty: true,
        }
    }

    fn emit(&self, ev: TunnelEvent) {
        let _ = self.events.send(ev);
    }

    fn run(&mut self) -> String {
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return "arrêt demandé".into();
            }
            let mut busy = false;
            // nouvelles connexions locales → poignée de main dans un thread
            loop {
                match self.listener.accept() {
                    Ok((sock, _)) => {
                        let tx = self.hs_tx.clone();
                        let _ = std::thread::Builder::new()
                            .name("proxy-handshake".into())
                            .spawn(move || {
                                let _ = tx.send(handshake(sock));
                            });
                        busy = true;
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(_) => break,
                }
            }
            while let Ok(res) = self.hs_rx.try_recv() {
                if let Ok(req) = res {
                    self.waiting.push_back(req);
                }
                busy = true;
            }
            // octets du modem
            loop {
                match self.from_modem.try_recv() {
                    Ok(bytes) => {
                        busy = true;
                        if let Err(reason) = self.on_modem_bytes(&bytes) {
                            let _ = self.mux.flush(&self.to_modem);
                            return reason;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return "liaison modem terminée".into(),
                }
            }
            if matches!(self.phase, Phase::Tunnel) {
                while let Some(req) = self.waiting.pop_front() {
                    self.open(req);
                    busy = true;
                }
                let (p, ended) = self.mux.pump();
                busy |= p;
                for e in ended {
                    let (StreamEnd::Local(id) | StreamEnd::Remote(id)) = e;
                    self.meter.closed(id);
                    self.dirty = true;
                    self.emit(TunnelEvent::Closed { id });
                }
            }
            if self.dirty {
                self.dirty = false;
                if let Ok(mut p) = self.progress.lock() {
                    *p = self.meter.snapshot(self.mux.len() as u32);
                }
            }
            match self.mux.flush(&self.to_modem) {
                Ok(p) => busy |= p,
                Err(_) => return "liaison modem terminée".into(),
            }
            if !busy {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }

    fn on_modem_bytes(&mut self, bytes: &[u8]) -> Result<(), String> {
        match &mut self.phase {
            Phase::Login(login) => {
                for step in login.feed(bytes) {
                    match step {
                        LoginStep::Send(b) => self.mux.send_raw(b),
                        LoginStep::Text(t) => self.emit(TunnelEvent::Text(t)),
                        LoginStep::Failed(why) => return Err(why),
                        LoginStep::Done(rest) => {
                            self.phase = Phase::Tunnel;
                            self.mux.send(&Frame::Hello { version: VERSION, max_streams: self.max_streams });
                            self.mux.dec.push(&rest);
                            self.decode_frames();
                            return Ok(());
                        }
                    }
                }
            }
            Phase::Tunnel => {
                self.mux.dec.push(bytes);
                self.decode_frames();
            }
        }
        Ok(())
    }

    fn decode_frames(&mut self) {
        while let Some(d) = self.mux.next_decoded() {
            match d {
                Decoded::Loss => {
                    self.mux.reset_all(true);
                    self.meter.reset();
                    self.dirty = true;
                    for (id, req) in self.opening.drain() {
                        reply_failure(req, FailCode::General);
                        self.mux.send(&Frame::Close { id });
                    }
                    self.emit(TunnelEvent::Corruption);
                }
                Decoded::Frame(f) => self.on_frame(f),
            }
        }
    }

    fn on_frame(&mut self, f: Frame) {
        match f {
            Frame::Hello { max_streams, .. } => {
                self.max_streams = max_streams.max(1);
                self.emit(TunnelEvent::Ready { max_streams });
            }
            Frame::OpenOk { id } => {
                if let Some(req) = self.opening.remove(&id) {
                    let target = req.target();
                    let (req_host, req_port) = (req.host.clone(), req.port);
                    let reply = success_reply(req.kind);
                    if self.mux.attach(id, req.sock, &reply).is_ok() {
                        self.meter.opened(id, &req_host, req_port);
                        self.dirty = true;
                        self.emit(TunnelEvent::Opened { id, target });
                    } else {
                        self.mux.send(&Frame::Close { id });
                    }
                }
            }
            Frame::OpenFail { id, code, msg } => {
                if let Some(req) = self.opening.remove(&id) {
                    let target = req.target();
                    reply_failure(req, code);
                    self.emit(TunnelEvent::OpenFailed { id, target, reason: msg });
                }
            }
            Frame::Data { id, data } => {
                let written = self.mux.write(id, &data);
                if written {
                    self.meter.data(id, &data);
                } else {
                    self.meter.closed(id);
                }
                self.dirty = true;
                if !written && !self.mux.contains(id) && !self.opening.contains_key(&id) {
                    // flux inconnu : on le referme de l'autre côté
                    self.mux.send(&Frame::Close { id });
                }
            }
            Frame::Close { id } => {
                if let Some(req) = self.opening.remove(&id) {
                    reply_failure(req, FailCode::General);
                }
                self.mux.close_remote(id);
            }
            // Messages réservés au serveur
            Frame::Open { id, .. } => self.mux.send(&Frame::OpenFail {
                id,
                code: FailCode::NotAllowed,
                msg: "le client n'accepte pas d'ouverture".into(),
            }),
        }
    }

    fn open(&mut self, req: Request) {
        if self.mux.len() + self.opening.len() >= self.max_streams as usize {
            let target = req.target();
            reply_failure(req, FailCode::General);
            self.emit(TunnelEvent::OpenFailed { id: 0, target, reason: "trop de connexions simultanées".into() });
            return;
        }
        let id = loop {
            self.next_id = self.next_id.wrapping_add(1).max(1);
            if !self.mux.contains(self.next_id) && !self.opening.contains_key(&self.next_id) {
                break self.next_id;
            }
        };
        self.mux.send(&Frame::Open { id, host: req.host.clone(), port: req.port });
        self.opening.insert(id, req);
    }
}

// ───────────────────────── SOCKS5 / HTTP CONNECT ─────────────────────────

fn success_reply(kind: ProxyKind) -> Vec<u8> {
    match kind {
        ProxyKind::Socks5 => vec![5, 0, 0, 1, 0, 0, 0, 0, 0, 0],
        ProxyKind::HttpConnect => b"HTTP/1.1 200 Connection established\r\n\r\n".to_vec(),
    }
}

fn reply_failure(mut req: Request, code: FailCode) {
    let _ = req.sock.set_nonblocking(false);
    let msg: Vec<u8> = match req.kind {
        ProxyKind::Socks5 => vec![5, code as u8, 0, 1, 0, 0, 0, 0, 0, 0],
        ProxyKind::HttpConnect => {
            let status = match code {
                FailCode::NotAllowed => "403 Forbidden",
                FailCode::Timeout => "504 Gateway Timeout",
                _ => "502 Bad Gateway",
            };
            format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes()
        }
    };
    let _ = req.sock.write_all(&msg);
}

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

/// Poignée de main du proxy (bloquante, dans un thread dédié).
fn handshake(mut sock: TcpStream) -> io::Result<Request> {
    sock.set_nonblocking(false)?;
    sock.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    sock.set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;
    let mut first = [0u8; 1];
    sock.read_exact(&mut first)?;
    match first[0] {
        5 => socks5(sock),
        b'C' | b'c' => http_connect(sock, first[0]),
        _ => {
            let _ = sock.write_all(
                b"HTTP/1.1 501 Not Implemented\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\n\r\n\
                  Proxy V34 : utilisez SOCKS5 ou HTTP CONNECT (proxy HTTPS).\r\n",
            );
            Err(bad("protocole de proxy inconnu"))
        }
    }
}

fn socks5(mut sock: TcpStream) -> io::Result<Request> {
    let mut n = [0u8; 1];
    sock.read_exact(&mut n)?;
    let mut methods = vec![0u8; n[0] as usize];
    sock.read_exact(&mut methods)?;
    if !methods.contains(&0) {
        sock.write_all(&[5, 0xFF])?;
        return Err(bad("SOCKS5 : aucune méthode sans authentification"));
    }
    sock.write_all(&[5, 0])?;
    let mut h = [0u8; 4];
    sock.read_exact(&mut h)?;
    if h[0] != 5 {
        return Err(bad("SOCKS5 : version invalide"));
    }
    let host = match h[3] {
        1 => {
            let mut a = [0u8; 4];
            sock.read_exact(&mut a)?;
            Ipv4Addr::from(a).to_string()
        }
        3 => {
            let mut l = [0u8; 1];
            sock.read_exact(&mut l)?;
            let mut name = vec![0u8; l[0] as usize];
            sock.read_exact(&mut name)?;
            String::from_utf8(name).map_err(|_| bad("SOCKS5 : nom invalide"))?
        }
        4 => {
            let mut a = [0u8; 16];
            sock.read_exact(&mut a)?;
            Ipv6Addr::from(a).to_string()
        }
        _ => {
            sock.write_all(&[5, 8, 0, 1, 0, 0, 0, 0, 0, 0])?;
            return Err(bad("SOCKS5 : type d'adresse non géré"));
        }
    };
    let mut p = [0u8; 2];
    sock.read_exact(&mut p)?;
    if h[1] != 1 {
        sock.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0])?;
        return Err(bad("SOCKS5 : seule la commande CONNECT est gérée"));
    }
    Ok(Request { sock, kind: ProxyKind::Socks5, host, port: u16::from_be_bytes(p) })
}

fn http_connect(mut sock: TcpStream, first: u8) -> io::Result<Request> {
    // Lecture octet par octet jusqu'à la ligne vide : rien n'est lu au-delà.
    let mut head = vec![first];
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") && !head.ends_with(b"\n\n") {
        if head.len() > 8192 {
            return Err(bad("HTTP : en-têtes trop longs"));
        }
        sock.read_exact(&mut b)?;
        head.push(b[0]);
    }
    let text = String::from_utf8_lossy(&head);
    let line = text.lines().next().unwrap_or_default();
    let mut parts = line.split_whitespace();
    let (method, authority) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
    let parsed = if method.eq_ignore_ascii_case("CONNECT") { parse_authority(authority) } else { None };
    match parsed {
        Some((host, port)) => Ok(Request { sock, kind: ProxyKind::HttpConnect, host, port }),
        None => {
            let _ = sock.write_all(
                b"HTTP/1.1 405 Method Not Allowed\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\n\r\n\
                  Proxy V34 : seule la methode CONNECT est geree (proxy HTTPS), ou SOCKS5.\r\n",
            );
            Err(bad("HTTP : requête non CONNECT"))
        }
    }
}

/// « hôte:port » ou « [ipv6]:port ».
fn parse_authority(s: &str) -> Option<(String, u16)> {
    let (host, port) = if let Some(rest) = s.strip_prefix('[') {
        let (h, p) = rest.split_once("]:")?;
        (h.to_string(), p)
    } else {
        let (h, p) = s.rsplit_once(':')?;
        (h.to_string(), p)
    };
    if host.is_empty() {
        return None;
    }
    if host.contains(':') && host.parse::<IpAddr>().is_err() {
        return None;
    }
    Some((host, port.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autorites_http() {
        assert_eq!(parse_authority("example.org:443"), Some(("example.org".into(), 443)));
        assert_eq!(parse_authority("[2001:db8::1]:8443"), Some(("2001:db8::1".into(), 8443)));
        assert_eq!(parse_authority("example.org"), None);
        assert_eq!(parse_authority(":80"), None);
        assert_eq!(parse_authority("a:b:80"), None);
    }

    #[test]
    fn login_automatique() {
        let mut l = LoginClient::new(Some("minitel".into()), Some("3615".into()));
        let steps = l.feed(b"\r\nV34NET\r\nlog");
        assert!(steps.is_empty());
        let steps = l.feed(b"in: ");
        assert_eq!(steps.last(), Some(&LoginStep::Send(b"minitel\r".to_vec())));
        let steps = l.feed(b"minitel\r\npassword: ");
        assert_eq!(steps.last(), Some(&LoginStep::Send(b"3615\r".to_vec())));
        let steps = l.feed(b"\r\nBienvenue\r\nTUNNEL\r\n\xA5\x01");
        assert_eq!(steps.last(), Some(&LoginStep::Done(vec![0xA5, 0x01])));
    }

    #[test]
    fn login_refuse_ou_absent() {
        let mut l = LoginClient::new(Some("x".into()), Some("y".into()));
        let steps = l.feed(b"\r\nLogin incorrect\r\nlogin: ");
        assert!(matches!(steps.last(), Some(LoginStep::Failed(_))));
        let mut l = LoginClient::new(None, None);
        let steps = l.feed(b"login: ");
        assert!(matches!(steps.last(), Some(LoginStep::Failed(_))));
        // serveur sans login : marqueur direct
        let mut l = LoginClient::new(None, None);
        let steps = l.feed(b"Bonjour\r\nTUNNEL\r\n");
        assert_eq!(steps, vec![LoginStep::Text("Bonjour".into()), LoginStep::Done(vec![])]);
    }
}
