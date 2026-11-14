//! Thread moteur temps réel : fait tourner la liaison (2 modems + lignes)
//! à 8 kHz par blocs de 20 ms, interprète les commandes AT, alimente le HP
//! et pousse l'état à l'IHM (~25 Hz).

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::Manager;
use tauri::{AppHandle, Emitter};
use v34_core::{
    LineParams, ModemCommand, ModemConfig, ModemEvent, ModemStatus, Phase, Role, SAMPLE_RATE,
};

use crate::audio::Speaker;
use crate::fft;
use crate::link::{Link, LocalLoop, Side};
use crate::tones::{SetupAction, ToneScript};

/// 20 ms à 8 kHz.
pub const BLOCK: usize = 160;
/// Sous-blocs de 5 ms : limite le délai ajouté par la boucle à 5 ms par sens.
const SUB: usize = 40;
const SCOPE_LEN: usize = 256;

/// Numéro du FAI (comme sur le serveur du VPS) : en l'appelant, le flux du
/// modem transporte un tunnel vers Internet au lieu du chat BBS.
pub const ISP_NUMBER: &str = "0836656565";
/// Port du proxy local (SOCKS5 + HTTP CONNECT) que le navigateur utilise.
pub const PROXY_PORT: u16 = 1080;

/// Compteurs de l'appel en cours, pour le moniteur de liaison.
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct Counters {
    /// Réapprentissages et renégociations
    pub recoveries: u32,
    /// Connexions du tunnel ouvertes en ce moment / depuis le début
    pub tunnel_open: u32,
    pub tunnel_opened: u64,
    pub tunnel_failed: u64,
    /// Réinitialisations du tunnel (erreur de transmission)
    pub tunnel_resets: u64,
}

/// Ce que transporte le flux d'octets du modem pendant l'appel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Session {
    /// Chat et fichiers (protocole BBS)
    Bbs,
    /// Tunnel vers Internet (appel du FAI)
    Internet,
}

/// Tunnel actif : proxy local ↔ flux du modem.
struct ActiveTunnel {
    pipe: v34_net::tunnel::ModemPipe,
    client: v34_net::tunnel::TunnelClient,
    /// Salon de discussion du FAI (connexion par le tunnel)
    salon: Option<std::net::TcpStream>,
    salon_rx: Option<std::sync::mpsc::Receiver<SalonMsg>>,
}

/// Messages du fil de lecture du salon.
enum SalonMsg {
    Connected(std::net::TcpStream),
    Line(String),
    Closed(String),
}

/// Se connecte au salon du FAI (`salon:6667`) par le proxy SOCKS5 local du
/// tunnel, puis remonte chaque ligne reçue.
fn salon_thread(proxy: std::net::SocketAddr, tx: std::sync::mpsc::Sender<SalonMsg>) {
    use std::io::{BufRead, BufReader, Read, Write};
    let connect = || -> std::io::Result<std::net::TcpStream> {
        let mut s = std::net::TcpStream::connect(proxy)?;
        s.set_read_timeout(Some(Duration::from_secs(30)))?;
        s.write_all(&[5, 1, 0])?;
        let mut r = [0u8; 2];
        s.read_exact(&mut r)?;
        let host = b"salon";
        let mut req = vec![5, 1, 0, 3, host.len() as u8];
        req.extend_from_slice(host);
        req.extend_from_slice(&6667u16.to_be_bytes());
        s.write_all(&req)?;
        let mut rep = [0u8; 10];
        s.read_exact(&mut rep)?;
        if rep[1] != 0 {
            return Err(std::io::Error::other(format!("refusé par le FAI (code {})", rep[1])));
        }
        s.set_read_timeout(None)?;
        Ok(s)
    };
    let s = match connect() {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(SalonMsg::Closed(format!("salon du FAI injoignable : {e}")));
            return;
        }
    };
    let Ok(w) = s.try_clone() else { return };
    if tx.send(SalonMsg::Connected(w)).is_err() {
        return;
    }
    for line in BufReader::new(s).lines() {
        match line {
            Ok(l) => {
                if tx.send(SalonMsg::Line(l)).is_err() {
                    return;
                }
            }
            Err(_) => break,
        }
    }
    let _ = tx.send(SalonMsg::Closed("salon du FAI fermé".into()));
}

pub enum EngineCmd {
    At(String),
    Modem(ModemCommand),
    SetLine(LineParams),
    Speaker { mode: Option<u8>, level: Option<u8>, volume: Option<f32> },
    Chat(String),
    /// Échelle de temps de la simulation (1 = temps réel)
    Speed(f64),
    /// Passer en mode réseau : connexion au central
    NetConnect { server: String, user: String, password: String },
    /// Revenir en boucle locale
    NetLocal,
    /// Redemander l'annuaire au central
    NetDirectory,
    /// Envoi d'un fichier (nom, contenu)
    SendFile(String, Vec<u8>),
    /// Panneau CONFIG : configuration des prochains appels (voir `modem_config`)
    SetConfig(ModemConfig),
}

/// Débit le plus bas du V.90 (sens descendant), en bit/s : en dessous, le
/// 56K ne peut pas tourner.
const V90_MIN_RATE: u32 = 28000;

/// Panneau CONFIG (interrupteur DIP 1 « 56K V.90 » et molette DÉBIT MAX) →
/// configuration du modem pour les prochains appels.
///
/// `max_rate` est le débit choisi sur la molette, de 2 400 à 56 000 bit/s :
/// - le V.34 s'arrête à 33 600 bit/s, on plafonne donc `max_rate` ;
/// - le V.90 descendant va de 28 000 à 56 000 bit/s : `max_rate_pcm` est
///   ramené dans cet intervalle ;
/// - sous 28 000 bit/s, le V.90 ne peut pas tourner : il est coupé même si
///   l'interrupteur est sur ON.
///
/// Les autres réglages gardent leurs valeurs par défaut.
pub fn modem_config(v90: bool, max_rate: u32) -> ModemConfig {
    ModemConfig {
        max_rate: max_rate.min(33600),
        v90: v90 && max_rate >= V90_MIN_RATE,
        max_rate_pcm: max_rate.clamp(V90_MIN_RATE, 56000),
        ..ModemConfig::default()
    }
}

#[derive(Clone, Serialize)]
pub struct SpeakerState {
    /// ATM : 0 = coupé, 1 = jusqu'à la porteuse, 2 = toujours
    pub mode: u8,
    /// ATL : 0..3
    pub level: u8,
    /// Volume général 0..1
    pub volume: f32,
    /// Le HP joue-t-il en ce moment ?
    pub active: bool,
}

#[derive(Clone, Serialize)]
pub struct UiState {
    pub status: ModemStatus,
    pub remote_phase: Option<Phase>,
    /// "idle" | "dialing" | "ringing" | "connecting" | "online"
    pub call_state: &'static str,
    pub link: &'static str,
    pub speaker: SpeakerState,
    /// Spectre du son de la ligne, dBFS, 128 cases de 31,25 Hz
    pub spectrum: Vec<f32>,
    /// 256 derniers échantillons (32 ms) pour l'oscilloscope
    pub scope: Vec<f32>,
    pub line: LineParams,
    pub line_response: Vec<(f32, f32)>,
    pub rx_level_db: f32,
    pub tx_level_db: f32,
    pub elapsed_s: f32,
    pub audio_device: Option<String>,
    /// Échelle de temps courante
    pub speed: f64,
    /// "bbs" ou "internet"
    pub session: &'static str,
    /// Proxy local du navigateur, quand le tunnel est établi
    pub proxy: Option<String>,
    /// Mode réseau : état de la connexion au central
    pub net: Option<crate::link::NetInfo>,
    /// Compteurs de l'appel (moniteur de liaison)
    pub counters: Counters,
    /// Avancement des chargements du navigateur (session Internet)
    pub nav: Option<v34_net::tunnel::Progress>,
}

#[derive(Clone, Serialize)]
struct ConsoleLine {
    text: String,
    /// "result" (code résultat Hayes), "resp" (réponse), "info"
    kind: &'static str,
}

#[derive(Clone, Serialize)]
struct EventMsg {
    side: Side,
    event: ModemEvent,
}

/// Progression d'un transfert de fichier pour l'IHM.
#[derive(Clone, Serialize)]
struct FileMsg {
    /// "tx" | "rx"
    dir: &'static str,
    name: String,
    done: u64,
    total: u64,
    /// Ko/s
    rate: f64,
    /// "progress" | "done" | "error"
    state: &'static str,
}

#[derive(Clone, Serialize)]
struct ChatMsg {
    /// "local" | "remote" | "system"
    from: &'static str,
    text: String,
}

#[derive(Clone, Copy, PartialEq)]
enum ScriptKind {
    Dialing,
    Ringing,
}

/// Niveaux ATL0..3
const LEVELS: [f32; 4] = [0.25, 0.4, 0.65, 1.0];

pub struct Engine {
    app: AppHandle,
    link: Box<dyn Link>,
    cfg: ModemConfig,
    line: LineParams,
    line_response: Vec<(f32, f32)>,
    speaker: Speaker,
    spk_mode: u8,
    spk_level: u8,
    volume: f32,
    spk_gain: f32,
    script: ToneScript,
    script_kind: ScriptKind,
    offhook_since: Option<Instant>,
    scope: VecDeque<f32>,
    rx_rms: f32,
    tx_rms: f32,
    status: ModemStatus,
    remote_phase: Option<Phase>,
    local_was_active: bool,
    remote_was_active: bool,
    idle_mismatch_since: Option<Instant>,
    user_hangup: bool,
    utf8_pending: Vec<u8>,
    /// Ligne reçue en cours de reconstitution (chat)
    chat_line: String,
    chat_line_since: Option<Instant>,
    // Protocole BBS (chat + fichiers) sur le flux fiable du modem
    bbs_rx: crate::bbs::Decoder,
    /// Octets confiés au modem depuis le début de l'appel
    stream_pos: u64,
    next_file_id: u32,
    outgoing: VecDeque<crate::bbs::Outgoing>,
    incoming: Option<crate::bbs::Incoming>,
    last_file_emit: Instant,
    /// Échelle de temps (0,1 = ralenti ×10 ; 5 = accéléré ×5)
    speed: f64,
    blocks_due: f64,
    session: Session,
    tunnel: Option<ActiveTunnel>,
    tunnel_ready: bool,
    /// Compteurs de l'appel en cours (moniteur de liaison)
    counters: Counters,
    /// Réseau : on joue le retour d'appel tant que l'autre n'a pas décroché
    ringback: bool,
}

fn fmt_speed(x: f64) -> String {
    let s = format!("{x}");
    s.replace('.', ",")
}

fn in_call(p: Phase) -> bool {
    !matches!(p, Phase::Idle | Phase::Failed)
}

impl Engine {
    pub fn new(app: AppHandle, speaker: Speaker) -> Self {
        let line = LineParams::default();
        let link: Box<dyn Link> = Box::new(LocalLoop::new(&line));
        let status = link.local_status();
        let line_response = link.line_response(96);
        Self {
            app,
            link,
            cfg: ModemConfig::default(),
            line,
            line_response,
            speaker,
            // V34_MUTE=1 : démarrage sans le son (tests automatiques)
            spk_mode: if std::env::var("V34_MUTE").is_ok() { 0 } else { 1 },
            spk_level: 2,
            volume: 0.8,
            spk_gain: 0.0,
            script: ToneScript::default(),
            script_kind: ScriptKind::Dialing,
            offhook_since: None,
            scope: VecDeque::from(vec![0.0; SCOPE_LEN]),
            rx_rms: 0.0,
            tx_rms: 0.0,
            status,
            remote_phase: None,
            local_was_active: false,
            remote_was_active: false,
            idle_mismatch_since: None,
            user_hangup: false,
            utf8_pending: Vec::new(),
            chat_line: String::new(),
            chat_line_since: None,
            bbs_rx: crate::bbs::Decoder::default(),
            stream_pos: 0,
            next_file_id: 1,
            outgoing: VecDeque::new(),
            incoming: None,
            last_file_emit: Instant::now(),
            speed: 1.0,
            blocks_due: 0.0,
            session: Session::Bbs,
            tunnel: None,
            tunnel_ready: false,
            counters: Counters::default(),
            ringback: false,
        }
    }

    /// Boucle temps réel (ne rend la main que si l'IHM disparaît).
    pub fn run(mut self, rx: Receiver<EngineCmd>) {
        let period = Duration::from_millis(20);
        let mut next = Instant::now();
        let mut n: u64 = 0;
        loop {
            loop {
                match rx.try_recv() {
                    Ok(cmd) => self.handle(cmd),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return,
                }
            }
            self.tick();
            n += 1;
            // Démo / tests : V34_AUTODIAL=1 compose tout seul 1 s après le
            // démarrage (V34_AUTODIAL=Campagne choisit aussi la ligne).
            if n == 50 {
                if let Ok(v) = std::env::var("V34_AUTODIAL") {
                    if let Some(p) = v34_core::LineParams::preset(&v) {
                        self.link.set_line(&p);
                        self.line = p;
                        self.line_response = self.link.line_response(96);
                    }
                    let num = std::env::var("V34_NUMBER").unwrap_or_else(|_| "0142424242".into());
                    self.handle_at(&format!("ATDT{num}"));
                }
            }
            // V34_NET="serveur|abonné|mot de passe" : mode réseau au démarrage
            if n == 1 {
                if let Ok(v) = std::env::var("V34_NET") {
                    let p: Vec<&str> = v.splitn(3, '|').collect();
                    if p.len() == 3 {
                        self.handle(EngineCmd::NetConnect { server: p[0].into(), user: p[1].into(), password: p[2].into() });
                    }
                }
            }
            // V34_SPEED=0.25 : échelle de temps au démarrage
            if n == 1 {
                if let Some(x) = std::env::var("V34_SPEED").ok().and_then(|v| v.parse::<f64>().ok()) {
                    self.handle(EngineCmd::Speed(x));
                }
            }
            // V34_AUTOFILE=chemin : envoie ce fichier 15 s après le démarrage
            if n == 760 {
                if let Ok(path) = std::env::var("V34_AUTOFILE") {
                    if let Ok(data) = std::fs::read(&path) {
                        let name = std::path::Path::new(&path).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                        self.handle(EngineCmd::SendFile(name, data));
                    }
                }
            }
            // V34_AUTOCHAT="texte" : envoie ce message 14 s après le démarrage
            if n == 750 {
                if let Ok(t) = std::env::var("V34_AUTOCHAT") {
                    self.handle(EngineCmd::Chat(t));
                }
            }
            if n % 2 == 0 {
                self.emit_state();
            }
            next += period;
            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
            } else if now - next > Duration::from_millis(200) {
                next = now; // on a pris trop de retard (machine en veille…) : on recale
            }
        }
    }

    // ------------------------------------------------------------ commandes

    fn handle(&mut self, cmd: EngineCmd) {
        match cmd {
            EngineCmd::At(line) => self.handle_at(&line),
            EngineCmd::Modem(c) => match c {
                ModemCommand::Dial => {
                    if !self.start_outgoing("") {
                        self.console("ERROR", "result");
                    }
                }
                ModemCommand::Answer => {
                    if !self.start_incoming() {
                        self.console("ERROR", "result");
                    }
                }
                ModemCommand::Hangup => self.hangup(),
                other => self.link.command_local(other),
            },
            EngineCmd::SetLine(p) => {
                self.link.set_line(&p);
                self.line = p;
                self.line_response = self.link.line_response(96);
            }
            EngineCmd::Speaker { mode, level, volume } => {
                if let Some(m) = mode {
                    self.spk_mode = m.min(2);
                }
                if let Some(l) = level {
                    self.spk_level = l.min(3);
                }
                if let Some(v) = volume {
                    self.volume = v.clamp(0.0, 1.0);
                }
            }
            // Session Internet : la ligne transporte le tunnel. Le chat va au
            // salon du FAI (par le tunnel) ; jamais d'octets bruts au milieu,
            // ils casseraient toutes les connexions du navigateur.
            EngineCmd::Chat(text) if self.session == Session::Internet => self.salon_say(text),
            EngineCmd::SendFile(..) if self.session == Session::Internet => {
                self.chat(
                    "system",
                    "Les fichiers vont d'abonné à abonné : appelle quelqu'un de l'ANNUAIRE pour lui en envoyer un.".into(),
                );
            }
            EngineCmd::Chat(text) => {
                if self.status.phase == Phase::Data {
                    let bytes = crate::bbs::encode(&crate::bbs::Frame::Chat(text.clone()));
                    self.send_bytes(bytes);
                    self.chat("local", text);
                } else {
                    self.chat("system", "Pas de porteuse : connecte-toi d'abord (ATDT…)".into());
                }
            }
            EngineCmd::Speed(x) if self.link.is_network() && (x - 1.0).abs() > 1e-6 => {
                let _ = x;
                self.console("Échelle de temps indisponible en mode réseau (l'autre PC est en temps réel)", "info");
            }
            EngineCmd::NetConnect { server, user, password } => {
                self.hangup();
                match crate::link::NetLink::connect(&server, &user, &password, &self.line) {
                    Ok(net) => {
                        self.link = Box::new(net);
                        self.speed = 1.0;
                        self.speaker.set_rate(SAMPLE_RATE as f64);
                        self.status = self.link.local_status();
                        self.console(&format!("Connexion au central {server} en tant que « {user} »…"), "info");
                    }
                    Err(e) => self.console(&format!("Central injoignable ({server}) : {e}"), "info"),
                }
            }
            EngineCmd::NetDirectory => self.link.net_refresh_directory(),
            EngineCmd::NetLocal => {
                self.hangup();
                self.link = Box::new(LocalLoop::new(&self.line));
                self.status = self.link.local_status();
                self.console("Retour en boucle locale", "info");
            }
            EngineCmd::Speed(x) => {
                self.speed = x.clamp(0.05, 10.0);
                self.speaker.set_rate(SAMPLE_RATE as f64 * self.speed);
                let txt = if (self.speed - 1.0).abs() < 1e-6 { "Vitesse normale (temps réel)".to_string() } else if self.speed < 1.0 {
                    format!("Ralenti ×{} : le son est {:.0} fois plus grave", fmt_speed(self.speed), 1.0 / self.speed)
                } else {
                    format!("Accéléré ×{} : le son est {:.0} fois plus aigu", fmt_speed(self.speed), self.speed)
                };
                self.console(&txt, "info");
            }
            // Comme les interrupteurs d'un vrai modem : lus au décroché, donc
            // pris en compte au prochain `new_call` (l'appel en cours continue).
            EngineCmd::SetConfig(cfg) => self.cfg = cfg,
            EngineCmd::SendFile(name, data) => {
                if !matches!(self.status.phase, Phase::Data | Phase::Retrain) {
                    self.chat("system", "Pas de porteuse : connecte-toi d'abord (ATDT…)".into());
                    return;
                }
                let id = self.next_file_id;
                self.next_file_id += 1;
                let name = crate::bbs::safe_name(&name);
                let frames = crate::bbs::file_frames(id, &name, &data);
                let start = self.stream_pos;
                self.send_bytes(frames);
                let rate = self.status.params.data_rate_tx.unwrap_or(0) as f64;
                let eta = if rate > 0.0 { (self.stream_pos - start) as f64 * 8.0 / rate } else { 0.0 };
                self.chat("system", format!("Envoi de « {name} » ({:.1} Ko) — environ {:.0} s à {} bit/s", data.len() as f64 / 1024.0, eta, rate as u32));
                self.outgoing.push_back(crate::bbs::Outgoing { name, size: data.len() as u64, stream_start: start, stream_end: self.stream_pos, started: Instant::now() });
            }
        }
    }

    fn handle_at(&mut self, raw: &str) {
        let up = raw.trim().to_ascii_uppercase();
        if up.is_empty() {
            return;
        }
        if up == "+++" {
            self.console("OK", "result");
            return;
        }
        if !up.starts_with("AT") {
            self.console("ERROR", "result");
            return;
        }
        let body: Vec<char> = up[2..].chars().collect();
        let mut i = 0;
        let mut responded = false;
        let num = |i: &mut usize| -> Option<u32> {
            let start = *i;
            while *i < body.len() && body[*i].is_ascii_digit() {
                *i += 1;
            }
            if *i == start {
                None
            } else {
                body[start..*i].iter().collect::<String>().parse().ok()
            }
        };
        while i < body.len() {
            let c = body[i];
            i += 1;
            match c {
                ' ' => {}
                'D' => {
                    let rest: String = body[i..].iter().collect();
                    let rest = rest.trim_start_matches(['T', 'P', ' ']);
                    let number: String = rest
                        .chars()
                        .filter(|c| c.is_ascii_digit() || "*#ABCD.,".contains(*c))
                        .collect();
                    if self.start_outgoing(&number) {
                        responded = true; // le code résultat viendra plus tard (CONNECT…)
                    } else {
                        self.console("ERROR", "result");
                        return;
                    }
                    i = body.len();
                }
                'A' => {
                    if !self.start_incoming() {
                        self.console("ERROR", "result");
                        return;
                    }
                    responded = true;
                }
                'H' => {
                    if num(&mut i).unwrap_or(0) == 0 {
                        self.hangup();
                    }
                }
                'M' => match num(&mut i).unwrap_or(0) {
                    m @ 0..=2 => self.spk_mode = m as u8,
                    _ => return self.console("ERROR", "result"),
                },
                'L' => match num(&mut i).unwrap_or(0) {
                    l @ 0..=3 => self.spk_level = l as u8,
                    _ => return self.console("ERROR", "result"),
                },
                'Z' => {
                    self.hangup();
                    self.spk_mode = 1;
                    self.spk_level = 2;
                    num(&mut i);
                }
                'I' => {
                    let lines: Vec<String> = match num(&mut i).unwrap_or(0) {
                        0 => vec!["V34-TINYVERSE".into()],
                        1 => vec!["255".into()],
                        3 => vec!["Modem logiciel V.34 Tinyverse 0.1 (Rust)".into()],
                        // Capacités, d'après le panneau CONFIG
                        4 => vec![
                            format!("V.8 V.34 2400-{} bit/s", self.cfg.max_rate),
                            "Symboles 2400 2743 2800 3000 3200 3429".into(),
                            if self.cfg.v90 {
                                format!("V.90 (56K) {V90_MIN_RATE}-{} bit/s", self.cfg.max_rate_pcm)
                            } else {
                                "V.90 (56K) désactivé (panneau CONFIG)".into()
                            },
                        ],
                        _ => vec!["OK".into()],
                    };
                    for l in &lines {
                        self.console(l, "resp");
                    }
                }
                'O' => {
                    if self.status.phase != Phase::Data {
                        self.console("NO CARRIER", "result");
                        return;
                    }
                    self.console(&format!("CONNECT {}", self.status.params.data_rate_rx.unwrap_or(0)), "result");
                    responded = true;
                }
                '&' => {
                    let sub = body.get(i).copied();
                    i += 1;
                    num(&mut i);
                    if sub == Some('F') {
                        self.spk_mode = 1;
                        self.spk_level = 2;
                    }
                }
                'S' => {
                    // ATSn=v / ATSn? : acceptés sans effet
                    while i < body.len() && (body[i].is_ascii_digit() || body[i] == '=' || body[i] == '?') {
                        i += 1;
                    }
                }
                'E' | 'V' | 'Q' | 'X' | 'B' | 'N' | 'W' => {
                    num(&mut i);
                }
                _ => return self.console("ERROR", "result"),
            }
        }
        if !responded {
            self.console("OK", "result");
        }
    }

    fn is_busy(&self) -> bool {
        self.script.is_active() || in_call(self.status.phase)
    }

    fn reset_call_flags(&mut self) {
        self.offhook_since = Some(Instant::now());
        self.counters = Counters::default();
        self.local_was_active = false;
        self.remote_was_active = false;
        self.idle_mismatch_since = None;
        self.user_hangup = false;
        self.utf8_pending.clear();
        // Nouvel appel : nouveau flux, nouvelle couche de correction d'erreurs
        self.bbs_rx = crate::bbs::Decoder::default();
        self.stream_pos = 0;
        self.outgoing.clear();
        self.incoming = None;
    }

    /// Signalisation du central (mode réseau).
    fn on_net_event(&mut self, ev: crate::link::NetEvent) {
        use crate::link::NetEvent as N;
        match ev {
            N::Registered { number } => {
                self.console(&format!("Connecté au central — votre numéro : {number}"), "result");
                self.chat("system", format!("Central : votre numéro est le {number}. Le FAI est au {ISP_NUMBER}."));
            }
            N::AuthFailed(r) => self.console(&format!("Central : connexion refusée ({r})"), "result"),
            N::ServerLost => self.console("Central injoignable — nouvelle tentative…", "info"),
            N::Incoming { from } => {
                if self.is_busy() {
                    return;
                }
                self.link.new_call(Role::Answer, &self.cfg);
                self.session = Session::Bbs;
                self.stop_tunnel();
                self.reset_call_flags();
                self.script = ToneScript::ringing_net(&from);
                self.script_kind = ScriptKind::Ringing;
            }
            N::Ringing => self.console("Ça sonne chez votre correspondant…", "info"),
            N::Answered => {
                self.ringback = false;
                self.script.clear();
                self.console("Décroché", "info");
            }
            N::Busy(n) => {
                self.ringback = false;
                self.script.clear();
                self.console(&format!("BUSY ({n} est occupé)"), "result");
                self.link.command_local(ModemCommand::Hangup);
            }
            N::Unknown(n) => {
                self.ringback = false;
                self.script.clear();
                self.console(&format!("NO ANSWER — le {n} n'est pas attribué"), "result");
                self.link.command_local(ModemCommand::Hangup);
            }
            N::HungUp(r) => {
                self.ringback = false;
                self.script.clear();
                self.stop_tunnel();
                if !self.user_hangup {
                    self.console(&format!("NO CARRIER ({r})"), "result");
                    self.user_hangup = true;
                }
            }
        }
    }

    /// Démarre le proxy local (SOCKS5 + HTTP CONNECT) branché sur le modem.
    fn start_tunnel(&mut self, tx_rate: u32) {
        let listener = std::net::TcpListener::bind(("127.0.0.1", PROXY_PORT)).or_else(|_| std::net::TcpListener::bind(("127.0.0.1", 0)));
        let listener = match listener {
            Ok(l) => l,
            Err(e) => {
                self.console(&format!("Proxy local impossible : {e}"), "info");
                return;
            }
        };
        let (pipe, from_modem, to_modem) = v34_net::tunnel::ModemPipe::new(tx_rate);
        match v34_net::tunnel::TunnelClient::spawn(listener, Default::default(), from_modem, to_modem) {
            Ok(client) => {
                self.console(&format!("Proxy Internet sur {} (SOCKS5 et HTTP) — ouvre le navigateur", client.local_addr()), "info");
                self.tunnel = Some(ActiveTunnel { pipe, client, salon: None, salon_rx: None });
                self.tunnel_ready = false;
            }
            Err(e) => self.console(&format!("Tunnel impossible : {e}"), "info"),
        }
    }

    /// Parle dans le salon du FAI.
    fn salon_say(&mut self, text: String) {
        use std::io::Write;
        let sent = match self.tunnel.as_mut().and_then(|t| t.salon.as_mut()) {
            Some(s) => {
                let line = text.replace(['\r', '\n'], " ");
                s.write_all(format!("{line}\n").as_bytes()).is_ok()
            }
            None => false,
        };
        if sent {
            if !text.starts_with('/') {
                self.chat("local", text);
            }
        } else {
            self.chat("system", "Le salon du FAI n'est pas (encore) ouvert : attends « Tunnel Internet établi ».".into());
        }
    }

    fn stop_tunnel(&mut self) {
        if let Some(mut t) = self.tunnel.take() {
            if let Some(s) = t.salon.take() {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
            t.client.stop();
        }
        self.tunnel_ready = false;
        self.counters.tunnel_open = 0;
    }

    /// Fait circuler le tunnel : octets vers le modem, événements à l'écran.
    fn pump_tunnel(&mut self) {
        let Some(t) = self.tunnel.as_mut() else { return };
        let mut out = Vec::new();
        while let Some(bytes) = t.pipe.take_for_modem() {
            out.extend(bytes);
            if out.len() > 4096 {
                break;
            }
        }
        let mut lines = Vec::new();
        while let Some(ev) = t.client.try_event() {
            use v34_net::tunnel::TunnelEvent;
            match ev {
                TunnelEvent::Text(txt) => {
                    let txt = txt.trim().to_string();
                    if !txt.is_empty() {
                        lines.push(txt);
                    }
                }
                TunnelEvent::Ready { max_streams } => {
                    self.tunnel_ready = true;
                    lines.push(format!("Tunnel Internet établi ({max_streams} connexions simultanées)"));
                    if t.salon_rx.is_none() {
                        let (tx, rx) = std::sync::mpsc::channel();
                        let proxy = t.client.local_addr();
                        if std::thread::Builder::new().name("salon".into()).spawn(move || salon_thread(proxy, tx)).is_ok() {
                            t.salon_rx = Some(rx);
                        }
                    }
                }
                TunnelEvent::Opened { target, .. } => {
                    self.counters.tunnel_open += 1;
                    self.counters.tunnel_opened += 1;
                    lines.push(format!("→ {target}"));
                }
                TunnelEvent::OpenFailed { target, reason, .. } => {
                    self.counters.tunnel_failed += 1;
                    lines.push(format!("✗ {target} : {reason}"));
                }
                TunnelEvent::Corruption => {
                    self.counters.tunnel_resets += 1;
                    self.counters.tunnel_open = 0;
                    lines.push("Erreur de transmission : connexions du tunnel réinitialisées".into());
                }
                TunnelEvent::Ended { reason } => lines.push(format!("Tunnel terminé : {reason}")),
                TunnelEvent::Closed { .. } => self.counters.tunnel_open = self.counters.tunnel_open.saturating_sub(1),
            }
        }
        let mut said = Vec::new();
        if let Some(rx) = t.salon_rx.as_ref() {
            while let Ok(m) = rx.try_recv() {
                match m {
                    SalonMsg::Connected(s) => {
                        t.salon = Some(s);
                        said.push(("system", "Tu es dans le SALON DU FAI : tout ce que tu tapes ici est lu par les abonnés connectés au FAI. /qui pour voir qui est là.".to_string()));
                    }
                    SalonMsg::Line(l) => match l.strip_prefix("* ") {
                        Some(notice) => said.push(("system", notice.to_string())),
                        None => said.push(("salon", l)),
                    },
                    SalonMsg::Closed(why) => {
                        t.salon = None;
                        said.push(("system", why));
                    }
                }
            }
        }
        if !out.is_empty() {
            self.link.command_local(ModemCommand::Send(out));
        }
        for l in lines {
            self.console(&l, "info");
        }
        for (from, text) in said {
            self.chat(from, text);
        }
    }

    /// Texte brut hors trames (ex. invite d'un serveur à l'ancienne) :
    /// reconstitué par lignes avant affichage.
    fn on_raw_text(&mut self, raw: &[u8]) {
        let text = self.decode_utf8(raw);
        if text.is_empty() {
            return;
        }
        if self.chat_line.is_empty() {
            self.chat_line_since = Some(Instant::now());
        }
        self.chat_line.push_str(&text);
        while let Some(i) = self.chat_line.find('\n') {
            let line: String = self.chat_line.drain(..=i).collect();
            self.chat("remote", line.trim_end_matches(['\r', '\n']).to_string());
            self.chat_line_since = Some(Instant::now());
        }
    }

    /// Confie des octets au modem local (et compte la position dans le flux).
    fn send_bytes(&mut self, bytes: Vec<u8>) {
        self.stream_pos += bytes.len() as u64;
        self.link.command_local(ModemCommand::Send(bytes));
    }

    /// Traite les octets reçus : trames BBS (chat, fichiers) et texte brut.
    fn on_data(&mut self, bytes: &[u8]) {
        use crate::bbs::{Frame, Item};
        for item in self.bbs_rx.push(bytes) {
            match item {
                Item::Frame(Frame::Chat(t)) => self.chat("remote", t),
                Item::Frame(Frame::FileOffer { id, size, name }) => {
                    self.chat("system", format!("Réception de « {name} » ({:.1} Ko)…", size as f64 / 1024.0));
                    self.incoming = Some(crate::bbs::Incoming { id, name, size, data: Vec::with_capacity(size.min(16 << 20) as usize), started: Instant::now() });
                }
                Item::Frame(Frame::FileData { id, data }) => {
                    if let Some(inc) = self.incoming.as_mut().filter(|i| i.id == id) {
                        inc.data.extend(data);
                    }
                }
                Item::Frame(Frame::FileEnd { id, crc }) => {
                    let Some(inc) = self.incoming.take().filter(|i| i.id == id) else { continue };
                    let secs = inc.started.elapsed().as_secs_f64().max(0.001);
                    let rate = inc.data.len() as f64 / 1024.0 / secs;
                    if crate::bbs::crc32(&inc.data) != crc || inc.data.len() as u64 != inc.size {
                        self.chat("system", format!("« {} » reçu abîmé (CRC faux) — ignoré", inc.name));
                        self.file_event("rx", &inc.name, inc.data.len() as u64, inc.size, rate, "error");
                        continue;
                    }
                    let dir = self.app.path().download_dir().unwrap_or_else(|_| std::env::temp_dir()).join("V34 reçus");
                    match crate::bbs::save_unique(&dir, &inc.name, &inc.data) {
                        Ok(path) => {
                            self.chat("system", format!("Fichier reçu : « {} » ({:.1} Ko, {:.2} Ko/s) → {}", inc.name, inc.size as f64 / 1024.0, rate, path.display()));
                            self.file_event("rx", &inc.name, inc.size, inc.size, rate, "done");
                        }
                        Err(e) => self.chat("system", format!("Impossible d'enregistrer « {} » : {e}", inc.name)),
                    }
                }
                Item::Frame(Frame::FileAbort { id }) => {
                    if self.incoming.as_ref().is_some_and(|i| i.id == id) {
                        let inc = self.incoming.take().unwrap();
                        self.chat("system", format!("Transfert de « {} » annulé par l'autre", inc.name));
                    }
                }
                Item::Raw(raw) => self.on_raw_text(&raw),
            }
        }
    }

    fn file_event(&self, dir: &'static str, name: &str, done: u64, total: u64, rate: f64, state: &'static str) {
        let _ = self.app.emit("v34:file", FileMsg { dir, name: name.to_string(), done, total, rate, state });
    }

    /// Progression des transferts, ~5 fois par seconde.
    fn update_transfers(&mut self) {
        if self.last_file_emit.elapsed() < Duration::from_millis(200) {
            return;
        }
        self.last_file_emit = Instant::now();
        let acked = self.status.link.as_ref().map(|l| l.bytes_acked).unwrap_or(0);
        while let Some(out) = self.outgoing.front() {
            let p = out.progress(acked);
            let done = (p * out.size as f64) as u64;
            let rate = done as f64 / 1024.0 / out.started.elapsed().as_secs_f64().max(0.001);
            if p >= 1.0 {
                let out = self.outgoing.pop_front().unwrap();
                self.chat("system", format!("« {} » envoyé et acquitté ({:.2} Ko/s)", out.name, rate));
                self.file_event("tx", &out.name, out.size, out.size, rate, "done");
            } else {
                self.file_event("tx", &out.name, done, out.size, rate, "progress");
                break;
            }
        }
        if self.outgoing.is_empty() {
            if let Some(inc) = self.incoming.as_ref() {
                let rate = inc.data.len() as f64 / 1024.0 / inc.started.elapsed().as_secs_f64().max(0.001);
                self.file_event("rx", &inc.name, inc.data.len() as u64, inc.size, rate, "progress");
            }
        }
    }

    fn start_outgoing(&mut self, number: &str) -> bool {
        if self.is_busy() {
            return false;
        }
        self.link.new_call(Role::Call, &self.cfg);
        // Appeler le FAI : le flux du modem portera un tunnel vers Internet.
        // En boucle locale, le modem distant simulé joue alors le FAI.
        let digits: String = number.chars().filter(|c| c.is_ascii_digit()).collect();
        let isp = self.link.net_info().and_then(|i| i.isp_number).unwrap_or_else(|| ISP_NUMBER.to_string());
        self.session = if digits == isp || digits == ISP_NUMBER { Session::Internet } else { Session::Bbs };
        self.link.set_remote_service(if self.session == Session::Internet { crate::link::RemoteService::Isp } else { crate::link::RemoteService::Echo });
        self.stop_tunnel();
        if self.link.is_network() {
            if !self.link.net_info().map(|i| i.registered).unwrap_or(false) {
                self.console("NO DIALTONE — pas connecté au central", "result");
                return false;
            }
            self.script = ToneScript::outgoing_net(&digits);
        } else {
            self.script = ToneScript::outgoing(number);
        }
        self.script_kind = ScriptKind::Dialing;
        self.reset_call_flags();
        let shown = if number.is_empty() { "(direct)".to_string() } else { number.to_string() };
        self.console(&format!("Numérotation {shown}…"), "info");
        true
    }

    fn start_incoming(&mut self) -> bool {
        if self.is_busy() {
            return false;
        }
        self.link.new_call(Role::Answer, &self.cfg);
        self.script = ToneScript::incoming();
        self.script_kind = ScriptKind::Ringing;
        self.reset_call_flags();
        self.console("Appel entrant simulé…", "info");
        true
    }

    fn hangup(&mut self) {
        self.user_hangup = true;
        self.script.clear();
        self.ringback = false;
        if self.link.is_network() && self.status.phase != Phase::Data {
            self.link.net_hangup();
        }
        if self.status.phase == Phase::Data {
            // Raccrochage propre (cleardown V.34) ; le distant suivra.
            self.link.command_local(ModemCommand::Hangup);
        } else {
            self.link.command_local(ModemCommand::Hangup);
            self.link.command_remote(ModemCommand::Hangup);
        }
    }

    fn do_action(&mut self, a: SetupAction) {
        match a {
            SetupAction::LocalDial => self.link.command_local(ModemCommand::Dial),
            SetupAction::RemoteAnswer => self.link.command_remote(ModemCommand::Answer),
            SetupAction::RemoteDial => self.link.command_remote(ModemCommand::Dial),
            SetupAction::LocalAnswer => self.link.command_local(ModemCommand::Answer),
            SetupAction::Console(s) => self.console(&s, "result"),
            SetupAction::NetDial(n) => {
                self.link.net_dial(&n);
                self.ringback = true;
            }
            SetupAction::NetAnswer => {
                self.link.net_answer();
            }
        }
    }

    // ------------------------------------------------------------ temps réel

    fn tick(&mut self) {
        self.update_transfers();
        self.pump_tunnel();
        for ev in self.link.poll_net() {
            self.on_net_event(ev);
        }
        if self.ringback && !self.script.is_active() {
            self.script = ToneScript::ringback();
        }
        // Ligne incomplète depuis 400 ms (ex. invite « login: ») : on l'affiche
        if let Some(t) = self.chat_line_since {
            if !self.chat_line.is_empty() && t.elapsed() > Duration::from_millis(400) {
                let line = std::mem::take(&mut self.chat_line);
                self.chat("remote", line);
                self.chat_line_since = None;
            }
        }
        // Échelle de temps : on traite `speed` blocs de 20 ms par tic de 20 ms.
        // Les délais de la norme sont comptés en échantillons : les modems ne
        // voient aucune différence ; seul le son est ralenti ou accéléré.
        self.blocks_due += self.speed + self.link.pace();
        let mut n = 0;
        while self.blocks_due >= 1.0 && n < 20 {
            self.process_block();
            self.blocks_due -= 1.0;
            n += 1;
        }
    }

    /// Un bloc de 20 ms de « temps de ligne ».
    fn process_block(&mut self) {
        let mut tones = [0.0f32; BLOCK];
        for a in self.script.render(&mut tones) {
            self.do_action(a);
        }

        let mut heard = [0.0f32; BLOCK];
        let (mut tx_e, mut rx_e) = (0.0f32, 0.0f32);
        for c in 0..BLOCK / SUB {
            let (mut tx, mut rx) = ([0.0f32; SUB], [0.0f32; SUB]);
            self.link.process(&mut tx, &mut rx);
            for k in 0..SUB {
                let i = c * SUB + k;
                heard[i] = tx[k] + rx[k] + tones[i];
                tx_e += tx[k] * tx[k];
                rx_e += rx[k] * rx[k];
            }
        }
        // Lissage des niveaux (≈ 100 ms)
        let a = 0.2;
        self.tx_rms = (1.0 - a) * self.tx_rms + a * (tx_e / BLOCK as f32).sqrt();
        self.rx_rms = (1.0 - a) * self.rx_rms + a * (rx_e / BLOCK as f32).sqrt();

        for (side, ev) in self.link.drain_events() {
            self.on_event(side, ev);
        }
        self.status = self.link.local_status();
        self.remote_phase = self.link.remote_phase();
        self.watch_hangup();

        for &s in heard.iter() {
            self.scope.pop_front();
            self.scope.push_back(s);
        }

        // HP : décroché (ou en numérotation) + mode ATM
        let offhook = self.status.leds.oh || self.script.is_active();
        let active = offhook
            && match self.spk_mode {
                0 => false,
                1 => !self.status.leds.cd,
                _ => true,
            };
        let target = if active { self.volume * LEVELS[self.spk_level as usize] } else { 0.0 };
        let step = 1.0 / (SAMPLE_RATE as f32 * 0.03); // fondu de 30 ms
        let mut out = [0.0f32; BLOCK];
        for (o, &s) in out.iter_mut().zip(heard.iter()) {
            self.spk_gain += (target - self.spk_gain).clamp(-step, step);
            *o = s * self.spk_gain;
        }
        self.speaker.push(&out);

        if !offhook && !in_call(self.status.phase) {
            self.offhook_since = None;
        }
    }

    /// Dans la boucle locale, si un bout raccroche, l'autre doit finir par le constater.
    fn watch_hangup(&mut self) {
        let local = self.status.phase;
        let remote = self.remote_phase.unwrap_or(Phase::Idle);
        self.local_was_active |= in_call(local);
        self.remote_was_active |= in_call(remote);
        let mismatch = (self.local_was_active && !in_call(local) && in_call(remote))
            || (self.remote_was_active && !in_call(remote) && in_call(local) && !self.script.is_active());
        if !mismatch {
            self.idle_mismatch_since = None;
            return;
        }
        let since = *self.idle_mismatch_since.get_or_insert_with(Instant::now);
        if since.elapsed() > Duration::from_millis(2500) {
            if in_call(remote) {
                self.link.command_remote(ModemCommand::Hangup);
            } else {
                self.link.command_local(ModemCommand::Hangup);
                if !self.user_hangup {
                    self.console("NO CARRIER", "result");
                    self.user_hangup = true;
                }
            }
            self.idle_mismatch_since = None;
        }
    }

    fn on_event(&mut self, side: Side, ev: ModemEvent) {
        if side == Side::Local {
            match &ev {
                ModemEvent::PhaseChanged(Phase::Retrain) => self.counters.recoveries += 1,
                ModemEvent::Connected { tx_rate, rx_rate } => {
                    self.console(&format!("CONNECT {rx_rate}/V34 (TX {tx_rate})"), "result");
                    if self.session == Session::Internet && self.tunnel.is_none() {
                        self.start_tunnel(*tx_rate);
                    }
                }
                ModemEvent::Disconnected { .. } | ModemEvent::PhaseChanged(Phase::Failed) => {
                    self.stop_tunnel();
                    if !self.user_hangup {
                        self.console("NO CARRIER", "result");
                        self.user_hangup = true;
                    }
                }
                ModemEvent::Data(bytes) => {
                    if let Some(t) = self.tunnel.as_mut() {
                        t.pipe.on_modem_data(bytes.clone());
                    } else {
                        let bytes = bytes.clone();
                        self.on_data(&bytes);
                    }
                }
                _ => {}
            }
        }
        let _ = self.app.emit("v34:event", EventMsg { side, event: ev });
    }

    /// Décodage UTF-8 incrémental (un caractère peut être coupé entre deux paquets).
    fn decode_utf8(&mut self, bytes: &[u8]) -> String {
        self.utf8_pending.extend_from_slice(bytes);
        let mut out = String::new();
        loop {
            match std::str::from_utf8(&self.utf8_pending) {
                Ok(s) => {
                    out.push_str(s);
                    self.utf8_pending.clear();
                    break;
                }
                Err(e) => {
                    let good = e.valid_up_to();
                    out.push_str(std::str::from_utf8(&self.utf8_pending[..good]).unwrap_or(""));
                    match e.error_len() {
                        Some(bad) => {
                            out.push('\u{FFFD}');
                            self.utf8_pending.drain(..good + bad);
                        }
                        None => {
                            self.utf8_pending.drain(..good);
                            break; // séquence incomplète : on attend la suite
                        }
                    }
                }
            }
        }
        out
    }

    // ------------------------------------------------------------ IHM

    fn console(&self, text: &str, kind: &'static str) {
        let _ = self.app.emit("v34:console", ConsoleLine { text: text.to_string(), kind });
    }

    fn chat(&self, from: &'static str, text: String) {
        let _ = self.app.emit("v34:chat", ChatMsg { from, text });
    }

    fn call_state(&self) -> &'static str {
        if self.script.is_active() {
            return match self.script_kind {
                ScriptKind::Dialing => "dialing",
                ScriptKind::Ringing => "ringing",
            };
        }
        match self.status.phase {
            Phase::Data => "online",
            p if in_call(p) => "connecting",
            _ => "idle",
        }
    }

    fn emit_state(&self) {
        let scope: Vec<f32> = self.scope.iter().copied().collect();
        let to_db = |r: f32| 20.0 * r.max(1e-5).log10();
        let state = UiState {
            status: self.status.clone(),
            remote_phase: self.remote_phase,
            call_state: self.call_state(),
            link: self.link.name(),
            speaker: SpeakerState {
                mode: self.spk_mode,
                level: self.spk_level,
                volume: self.volume,
                active: self.spk_gain > 0.01,
            },
            spectrum: fft::spectrum_db(&scope),
            scope,
            line: self.line.clone(),
            line_response: self.line_response.clone(),
            rx_level_db: to_db(self.rx_rms),
            tx_level_db: to_db(self.tx_rms),
            elapsed_s: self.offhook_since.map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.0),
            speed: self.speed,
            session: if self.session == Session::Internet { "internet" } else { "bbs" },
            net: self.link.net_info(),
            counters: self.counters,
            proxy: self.tunnel.as_ref().filter(|_| self.tunnel_ready).map(|t| t.client.local_addr().to_string()),
            nav: self.tunnel.as_ref().map(|t| t.client.progress()),
            audio_device: self.speaker.device_name(),
        };
        let _ = self.app.emit("v34:state", state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_panel_maps_to_modem_config() {
        // 56K au maximum : V.34 plafonné à 33 600, V.90 jusqu'à 56 000
        let c = modem_config(true, 56000);
        assert_eq!((c.max_rate, c.v90, c.max_rate_pcm), (33600, true, 56000));
        // Palier V.90 intermédiaire
        let c = modem_config(true, 45333);
        assert_eq!((c.max_rate, c.v90, c.max_rate_pcm), (33600, true, 45333));
        // Interrupteur 56K sur OFF : V.34 seul
        let c = modem_config(false, 56000);
        assert_eq!((c.max_rate, c.v90, c.max_rate_pcm), (33600, false, 56000));
        // 28 800 : V.90 encore possible (plancher à 28 000)
        let c = modem_config(true, 28800);
        assert_eq!((c.max_rate, c.v90, c.max_rate_pcm), (28800, true, 28800));
        // Sous 28 000 : plus de V.90, même sur ON
        let c = modem_config(true, 26400);
        assert_eq!((c.max_rate, c.v90, c.max_rate_pcm), (26400, false, 28000));
        let c = modem_config(true, 2400);
        assert_eq!((c.max_rate, c.v90, c.max_rate_pcm), (2400, false, 28000));
        // Les autres réglages gardent leurs valeurs par défaut
        let d = ModemConfig::default();
        assert_eq!(ModemConfig { max_rate: d.max_rate, v90: d.v90, max_rate_pcm: d.max_rate_pcm, ..c }, d);
        // Réglages d'usine du panneau = configuration par défaut du modem
        assert_eq!(modem_config(true, 56000), d);
    }
}
