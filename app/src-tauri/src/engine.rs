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

use v34_net::tarif::{self, Categorie};

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
pub const ISP_NUMBER: &str = "0836154286";
/// 3615 ORACLE (08 36 « ORACLE ») : un correspondant ordinaire pour le
/// modem (chat BBS) ; en boucle locale, le distant simulé joue l'oracle.
pub const ORACLE_NUMBER: &str = v34_server::config::ORACLE_NUMERO;
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

/// Étapes de la fenêtre « Connexion à… » de Windows 98 (instant de fin, texte).
const PPP_STEPS: [(f32, &str); 2] = [
    (1.8, "Vérification du nom d'utilisateur et du mot de passe…"),
    (4.2, "Enregistrement de votre ordinateur sur le réseau…"),
];

/// Étape PPP en cours d'après le temps écoulé (`None` : fini).
fn ppp_stage(e: std::time::Duration) -> Option<usize> {
    PPP_STEPS.iter().position(|(fin, _)| e.as_secs_f32() < *fin)
}

/// Démarre le salon du FAI (connexion par le tunnel).
fn start_salon(t: &mut ActiveTunnel) {
    if t.salon_rx.is_none() {
        let (tx, rx) = std::sync::mpsc::channel();
        let proxy = t.client.local_addr();
        if std::thread::Builder::new().name("salon".into()).spawn(move || salon_thread(proxy, tx)).is_ok() {
            t.salon_rx = Some(rx);
        }
    }
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
    /// Panneau CONFIG : configuration du modem, réponse automatique
    SetConfig(ModemConfig, bool),
    /// Visiophone : caméra (ou mire) allumée / éteinte
    VisioCamera(bool),
    /// Visiophone : une image hallucinée à envoyer, si la ligne le permet
    VisioFrame(Vec<u8>),
    /// Télécopie : compose ce numéro et envoie cette page (lignes de 1728
    /// points, vrai = noir ; fine : 7,7 lignes par mm)
    EnvoyerFax { numero: String, lignes: Vec<Vec<bool>>, fine: bool },
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
pub fn modem_config(v90: bool, max_rate: u32, express: bool) -> ModemConfig {
    // (la réponse automatique, DIP 2, est un réglage de l'appli, pas du modem)
    ModemConfig {
        max_rate: max_rate.min(33600),
        v90: v90 && max_rate >= V90_MIN_RATE,
        max_rate_pcm: max_rate.clamp(V90_MIN_RATE, 56000),
        express,
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
    /// Fenêtre « Connexion à… » façon Windows 98 : étape en cours
    pub ppp: Option<&'static str>,
    /// Appel entrant qui sonne (réponse manuelle) : numéro de l'appelant
    pub incoming: Option<String>,
    /// Visiophone : correspondant, cadence, débits
    pub visio: crate::visio::VisioUi,
    /// L'appel en cours est pour 3615 ORACLE
    pub oracle: bool,
    /// Compteur de taxes de l'appel en cours (ou du dernier)
    pub taxe: Option<TaxeUi>,
}

/// Préréglage « Orage » (succès « Dans la tempête »)
static ORAGE: std::sync::LazyLock<LineParams> = std::sync::LazyLock::new(|| LineParams::preset("Orage").expect("préréglage Orage"));

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
    /// Interrupteur DIP 2 : décrocher tout seul aux appels entrants
    auto_answer: bool,
    /// Appel entrant qui sonne, en attente de ATA (réponse manuelle)
    ringing_from: Option<String>,
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
    /// Connexion « Accès réseau à distance » en cours (lenteurs d'époque)
    ppp: Option<Instant>,
    ppp_shown: usize,
    /// Compteurs de l'appel en cours (moniteur de liaison)
    counters: Counters,
    /// Réseau : on joue le retour d'appel tant que l'autre n'a pas décroché
    ringback: bool,
    visio: crate::visio::Visio,
    /// Appel en cours vers 3615 ORACLE (mode d'emploi affiché à CONNECT)
    oracle_call: bool,
    /// Numéro déjà annoncé dans la messagerie : une reconnexion au central
    /// (même compte) ne le répète pas
    numero_annonce: Option<String>,
    /// Compteur de taxes de l'appel en cours (ou du dernier, figé)
    taxe: Option<Taxe>,
    /// Catégorie tarifaire de l'appel qu'on compose (`None` : appel reçu)
    taxe_cat: Option<Categorie>,
    /// Succès déjà connus : on n'annonce que les nouveaux (`None` avant le
    /// premier COMPTE du central)
    succes_connus: Option<std::collections::BTreeSet<String>>,
    /// Exploits de l'appel en cours : temps en données sur la ligne Orage,
    /// réentraînements
    orage_s: f32,
    retrains: u32,
    /// Télécopie qui arrive : lignes déjà imprimées, résolution, expéditeur
    fax_recu: Vec<Vec<bool>>,
    fax_fine: bool,
    fax_de: Option<String>,
    /// Lignes déjà montrées à l'IHM (page en cours)
    fax_y: usize,
    /// Balayage de numéros en cours (AT*B)
    balayage: Option<Balayage>,
}

/// Balayage de numéros (AT*B) : chaque numéro d'une plage est composé à son
/// tour, et l'on note qui décroche, comme les « wardialers » d'époque.
struct Balayage {
    suivant: u64,
    fin: u64,
    chiffres: usize,
    /// Numéro composé et depuis quand
    en_cours: Option<(String, Instant)>,
    /// Pas de nouvel appel avant…
    pause_jusqua: Instant,
    composes: u32,
    porteuses: Vec<String>,
    occupes: Vec<String>,
}

/// Plus de numéros par balayage
const BALAYAGE_MAX: u64 = 100;

/// « 01 23 45 67 89 »
fn num_lisible(n: &str) -> String {
    n.as_bytes().chunks(2).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join(" ")
}

/// Compteur de taxes d'un appel (tarifs : [`v34_net::tarif`]).
struct Taxe {
    cat: Categorie,
    debut_unix: u64,
    debut: Instant,
    fin: Option<Instant>,
    /// Appel reçu : c'est l'appelant qui paie
    recu: bool,
}

/// Le compteur, pour l'IHM (ligne TAXES du panneau).
#[derive(Clone, Serialize)]
pub struct TaxeUi {
    pub centimes: u32,
    /// Tarif à la minute en ce moment (centimes)
    pub minute: u32,
    pub libelle: &'static str,
    pub heures_creuses: bool,
    pub recu: bool,
    pub fini: bool,
}

impl Taxe {
    fn ui(&self) -> TaxeUi {
        let fin = self.fin.unwrap_or_else(Instant::now);
        let s = fin.saturating_duration_since(self.debut).as_secs() as u32;
        let now = self.debut_unix + s as u64;
        let (centimes, minute) = if self.recu { (0, 0) } else { (tarif::cout_centimes(self.cat, self.debut_unix, s), tarif::tarif_minute(self.cat, now)) };
        TaxeUi { centimes, minute, libelle: self.cat.libelle(), heures_creuses: tarif::heures_creuses(now), recu: self.recu, fini: self.fin.is_some() }
    }
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
            auto_answer: true,
            ringing_from: None,
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
            ppp: None,
            ppp_shown: 0,
            counters: Counters::default(),
            ringback: false,
            visio: Default::default(),
            oracle_call: false,
            numero_annonce: None,
            taxe: None,
            taxe_cat: None,
            succes_connus: None,
            orage_s: 0.0,
            retrains: 0,
            fax_recu: Vec::new(),
            fax_fine: true,
            fax_de: None,
            fax_y: 0,
            balayage: None,
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
            self.suivre_appel(n as u64);
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
            if now > next + Duration::from_millis(60) && crate::link::trace_on() {
                eprintln!("[{:>6}] moteur en retard de {} ms", trace_ms(), (now - next).as_millis());
            }
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
                    if self.answer_ringing() {
                        return;
                    }
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
                self.numero_annonce = None;
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
            EngineCmd::SetConfig(cfg, auto) => {
                self.cfg = cfg;
                self.auto_answer = auto;
            }
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
            EngineCmd::VisioCamera(on) => {
                if let Some(b) = self.visio.set_camera(on, &self.visio_ctx(), Instant::now()) {
                    self.send_bytes(b);
                }
            }
            EngineCmd::VisioFrame(img) => {
                if let Some(b) = self.visio.offer(img, &self.visio_ctx(), Instant::now()) {
                    self.send_bytes(b);
                }
            }
            EngineCmd::EnvoyerFax { numero, lignes, fine } => {
                let n = lignes.len();
                if !self.start_outgoing(&numero) {
                    self.console("ERROR", "result");
                    return;
                }
                // le modem créé pour l'appel enverra la page au lieu de négocier
                self.link.command_local(ModemCommand::PreparerFax { lignes, fine });
                self.fax_recu.clear();
                self.console(&format!("Télécopie vers {numero} : {n} lignes, en {}", if fine { "fin" } else { "standard" }), "info");
            }
        }
    }

    /// Visiophone : ce qu'il doit savoir de la liaison (appel entre
    /// abonnés en données, file d'émission, débit, fichier en cours).
    fn visio_ctx(&self) -> crate::visio::LinkCtx {
        crate::visio::LinkCtx {
            online: self.session == Session::Bbs && self.tunnel.is_none() && self.status.phase == Phase::Data,
            tx_queue: self.status.link.as_ref().map_or(0, |l| l.tx_queue),
            rate_bps: self.status.params.data_rate_tx.unwrap_or(0),
            file_busy: !self.outgoing.is_empty(),
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
        if let Some(arg) = up.strip_prefix("AT*B") {
            return self.commande_balayage(arg.trim());
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
                    if self.answer_ringing() {
                        responded = true;
                        i = body.len();
                        continue;
                    }
                    if !self.start_incoming() {
                        self.console("ERROR", "result");
                        return;
                    }
                    responded = true;
                }
                'H' => {
                    if num(&mut i).unwrap_or(0) == 0 {
                        if self.balayage.take().is_some() {
                            self.console("BALAYAGE ARRÊTÉ", "result");
                        }
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
        self.taxe = None;
        self.orage_s = 0.0;
        self.retrains = 0;
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
        self.visio.reset();
    }

    /// Signalisation du central (mode réseau).
    fn on_net_event(&mut self, ev: crate::link::NetEvent) {
        use crate::link::NetEvent as N;
        match ev {
            N::Registered { number } => {
                self.console(&format!("Connecté au central — votre numéro : {number}"), "result");
                // identité de notre télécopieur (TSI / CSI)
                self.cfg.ident = number.clone();
                // Après une coupure, le central nous redonne le même numéro :
                // la messagerie le sait déjà.
                if self.numero_annonce.as_deref() != Some(number.as_str()) {
                    self.chat("system", format!("Central : votre numéro est le {number}. Le FAI est au {ISP_NUMBER}."));
                    self.numero_annonce = Some(number);
                    // autre compte : ses succès ne sont pas « nouveaux »
                    self.succes_connus = None;
                }
            }
            N::AuthFailed(r) => self.console(&format!("Central : connexion refusée ({r})"), "result"),
            N::ServerLost => self.console("Central injoignable — nouvelle tentative…", "info"),
            N::Compte(c) => {
                let nouveaux: Vec<String> = match &self.succes_connus {
                    Some(k) => c.succes.iter().filter(|s| !k.contains(*s)).cloned().collect(),
                    None => Vec::new(),
                };
                self.succes_connus = Some(c.succes.iter().cloned().collect());
                for id in nouveaux {
                    if let Some(s) = v34_net::succes::trouver(&id) {
                        self.console(&format!("★ SUCCÈS : {} — {}", s.titre, s.description), "info");
                        let _ = self.app.emit("v34:succes", s);
                    }
                }
            }
            N::Incoming { from } => {
                if self.is_busy() {
                    return;
                }
                self.link.new_call(Role::Answer, &self.cfg);
                self.session = Session::Bbs;
                self.oracle_call = false;
                self.taxe_cat = None;
                self.stop_tunnel();
                self.reset_call_flags();
                if self.auto_answer {
                    self.script = ToneScript::ringing_net(&from);
                } else {
                    // DIP 2 sur OFF : ça sonne jusqu'à ATA
                    self.script = ToneScript::ringing_manual(&from);
                    self.ringing_from = Some(from);
                }
                self.script_kind = ScriptKind::Ringing;
            }
            N::Ringing => self.console("Ça sonne chez votre correspondant…", "info"),
            N::Answered if self.balayage.as_ref().is_some_and(|b| b.en_cours.is_some()) => {
                // une porteuse ! on note et on raccroche aussitôt
                self.ringback = false;
                self.script.clear();
                self.hangup();
                self.balayage_resultat(Some("PORTEUSE !"), 2000);
            }
            N::Answered => {
                self.ringback = false;
                self.script.clear();
                self.console("Décroché", "info");
            }
            N::Busy(_) if self.balayage.as_ref().is_some_and(|b| b.en_cours.is_some()) => {
                self.ringback = false;
                self.script.clear();
                self.balayage_resultat(Some("OCCUPÉ"), 500);
            }
            N::Unknown(_) if self.balayage.as_ref().is_some_and(|b| b.en_cours.is_some()) => {
                self.ringback = false;
                self.script.clear();
                self.balayage_resultat(None, 300);
            }
            N::Busy(n) => {
                // Tonalité d'occupation, puis BUSY (le modem l'a « entendue »)
                self.ringback = false;
                self.script = ToneScript::busy(format!("BUSY ({n} est occupé)"));
                self.script_kind = ScriptKind::Dialing;
            }
            N::Unknown(n) => {
                // Les trois notes du numéro non attribué
                self.ringback = false;
                self.script = ToneScript::unassigned(format!("NO ANSWER — le {n} n'est pas attribué"));
                self.script_kind = ScriptKind::Dialing;
            }
            N::HungUp(reason, r) if self.balayage.as_ref().is_some_and(|b| b.en_cours.is_some()) => {
                // le central a coupé (accès refusé, réseau encombré…) : ça a répondu quand même
                self.ringback = false;
                self.script.clear();
                self.user_hangup = true;
                let q = if reason == v34_net::signal::HangupReason::Forbidden { "PORTEUSE (accès refusé)".to_string() } else { format!("coupé ({r})") };
                self.balayage_resultat(Some(&q), 1000);
            }
            N::HungUp(reason, r) => {
                use v34_net::signal::HangupReason as H;
                self.ringback = false;
                self.script.clear();
                self.stop_tunnel();
                if self.ringing_from.take().is_some() {
                    self.console("L'appelant a raccroché avant qu'on décroche", "info");
                    self.user_hangup = true;
                    return;
                }
                if self.user_hangup {
                    return;
                }
                self.user_hangup = true;
                let connected = matches!(self.status.phase, Phase::Data | Phase::Phase2 | Phase::Phase3 | Phase::Phase4 | Phase::Retrain | Phase::Cleardown);
                match reason {
                    // FAI non activé : il « sonne occupé » (comme le dit le site)
                    H::Forbidden if !connected => {
                        self.script = ToneScript::busy(format!("BUSY ({r})"));
                        self.script_kind = ScriptKind::Dialing;
                    }
                    // Plus de modem libre, limite atteinte : réseau encombré
                    H::Rejected if !connected => {
                        self.script = ToneScript::congestion(format!("NO CARRIER ({r})"));
                        self.script_kind = ScriptKind::Dialing;
                    }
                    H::NoAnswer => self.console("NO ANSWER", "result"),
                    _ => self.console(&format!("NO CARRIER ({r})"), "result"),
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
        self.ppp = None;
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
        self.ppp = None;
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
                    lines.push(format!("Tunnel Internet établi ({max_streams} connexions simultanées)"));
                    if self.cfg.express {
                        self.tunnel_ready = true;
                        start_salon(t);
                    } else {
                        // Comme en 1998 : l'Accès réseau à distance de Windows
                        // négocie encore (PPP) avant qu'on puisse surfer.
                        self.ppp = Some(Instant::now());
                        self.ppp_shown = 0;
                        lines.push(PPP_STEPS[0].1.to_string());
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
                Item::Frame(Frame::Chat(t)) => self.chat("remote", crate::bbs::chat_recu(t, self.oracle_call)),
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
                Item::Frame(Frame::Visio { camera }) => {
                    if let Some(b) = self.visio.on_announce(camera, &self.visio_ctx(), Instant::now()) {
                        self.send_bytes(b);
                    }
                }
                Item::Frame(Frame::Video(img)) => {
                    if let Some(f) = self.visio.on_video(&img, Instant::now()) {
                        let _ = self.app.emit("v34:visio", f);
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
                if !self.oracle_call {
                    self.exploit("fichier");
                    if out.size >= v34_net::succes::GROS_COLIS {
                        self.exploit("gros-colis");
                    }
                }
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
        // 3615 ORACLE : un correspondant comme un autre (chat BBS) ; en
        // boucle locale, le distant simulé le joue (hors ligne).
        let services = self.link.net_info().map(|i| i.services).unwrap_or_default();
        self.oracle_call = digits == ORACLE_NUMBER || services.iter().any(|(n, k)| *n == digits && k == "oracle");
        // Compteur de taxes : le FAI au tarif local, l'oracle (et les 08 36)
        // au kiosque, les numéros verts gratuits. Le central fait foi.
        self.taxe_cat = Some(if self.session == Session::Internet {
            Categorie::Local
        } else if self.oracle_call || (digits.starts_with("0836") && !services.iter().any(|(n, _)| *n == digits)) {
            Categorie::Kiosque
        } else if digits.starts_with("0800") || services.iter().any(|(n, k)| *n == digits && k == "echo") {
            Categorie::Vert
        } else {
            Categorie::Local
        });
        self.link.set_remote_service(match self.session {
            Session::Internet => crate::link::RemoteService::Isp,
            Session::Bbs if self.oracle_call => crate::link::RemoteService::Oracle,
            Session::Bbs => crate::link::RemoteService::Echo,
        });
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

    /// ATA pendant qu'un appel du central sonne (réponse manuelle).
    fn answer_ringing(&mut self) -> bool {
        if self.ringing_from.take().is_none() {
            return false;
        }
        self.script.clear();
        self.console("Décroché", "info");
        self.link.net_answer();
        true
    }

    fn start_incoming(&mut self) -> bool {
        if self.is_busy() {
            return false;
        }
        self.link.new_call(Role::Answer, &self.cfg);
        if std::mem::take(&mut self.oracle_call) {
            // l'appel simulé vient d'un abonné, pas de l'oracle
            self.link.set_remote_service(crate::link::RemoteService::Echo);
        }
        self.script = ToneScript::incoming();
        self.script_kind = ScriptKind::Ringing;
        self.taxe_cat = None;
        self.reset_call_flags();
        self.console("Appel entrant simulé…", "info");
        true
    }

    fn hangup(&mut self) {
        self.user_hangup = true;
        self.ringing_from = None;
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
            SetupAction::Fail(result) => {
                self.console(&result, "result");
                self.link.command_local(ModemCommand::Hangup);
            }
        }
    }

    // ------------------------------------------------------------ temps réel

    /// Fenêtre « Connexion à… » : étapes successives, puis Internet prêt.
    fn ppp_step(&mut self) {
        let Some(t0) = self.ppp else { return };
        let e = t0.elapsed();
        match ppp_stage(e) {
            Some(i) => {
                if i > self.ppp_shown {
                    self.ppp_shown = i;
                    self.console(PPP_STEPS[i].1, "info");
                }
            }
            None => {
                self.ppp = None;
                self.tunnel_ready = true;
                if let Some(t) = self.tunnel.as_mut() {
                    start_salon(t);
                }
                let r = self.status.params.data_rate_rx.unwrap_or(0);
                self.console(&format!("Connecté à TINYVERSE à {r} bit/s — bonne navigation !"), "result");
            }
        }
    }

    /// Succès constaté par l'appli : le central le note (mode réseau).
    fn exploit(&mut self, id: &str) {
        if self.link.is_network() {
            self.link.net_exploit(id);
        }
    }

    /// Compteur de taxes et exploits de l'appel en cours (à chaque tic de 20 ms).
    fn suivre_appel(&mut self, n: u64) {
        let en_appel = in_call(self.status.phase);
        match &mut self.taxe {
            None if en_appel && self.offhook_since.is_some() => {
                let cat = self.taxe_cat.unwrap_or(Categorie::Local);
                let debut_unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                self.taxe = Some(Taxe { cat, debut_unix, debut: Instant::now(), fin: None, recu: self.taxe_cat.is_none() });
            }
            Some(t) if t.fin.is_none() && !en_appel => t.fin = Some(Instant::now()),
            _ => {}
        }
        if self.status.phase != Phase::Data {
            return;
        }
        if self.orage_s < v34_net::succes::TEMPETE_S && self.line == *ORAGE {
            self.orage_s += 0.02;
            if self.orage_s >= v34_net::succes::TEMPETE_S {
                self.exploit("tempete");
            }
        }
        if self.retrains >= v34_net::succes::TENACE_RETRAINS {
            self.exploit("tenace");
        }
        if n % 50 == 0 {
            let v = self.visio.ui(&self.visio_ctx(), Instant::now());
            if v.online && v.camera && v.peer_camera {
                self.exploit("visio");
            }
        }
    }

    /// AT*B<début>-<fin> ou AT*B<préfixe>XX : balayer une plage (100 numéros
    /// au plus) ; AT*B seul l'arrête.
    fn commande_balayage(&mut self, arg: &str) {
        if arg.is_empty() || arg == "0" {
            if self.balayage.take().is_some() {
                self.console("BALAYAGE ARRÊTÉ", "result");
            } else {
                self.console("AT*B<début>-<fin> (ex. AT*B0836202600-0836202699) ou AT*B08362026XX : balaye une plage de 100 numéros au plus.", "resp");
                self.console("OK", "result");
            }
            return;
        }
        let (debut, fin) = if let Some((a, b)) = arg.split_once('-') {
            (a.trim().to_string(), b.trim().to_string())
        } else if arg.contains('X') {
            (arg.replace('X', "0"), arg.replace('X', "9"))
        } else {
            (arg.to_string(), arg.to_string())
        };
        let ok = |s: &str| !s.is_empty() && s.len() <= 20 && s.chars().all(|c| c.is_ascii_digit());
        let (Ok(a), Ok(b)) = (debut.parse::<u64>(), fin.parse::<u64>()) else {
            self.console("ERROR — plage illisible (ex. AT*B0836202600-0836202699)", "result");
            return;
        };
        if !ok(&debut) || !ok(&fin) || debut.len() != fin.len() || b < a {
            self.console("ERROR — plage illisible (ex. AT*B0836202600-0836202699)", "result");
            return;
        }
        if b - a + 1 > BALAYAGE_MAX {
            self.console(&format!("ERROR — {} numéros au plus par balayage", BALAYAGE_MAX), "result");
            return;
        }
        if !self.link.is_network() || !self.link.net_info().is_some_and(|i| i.registered) {
            self.console("ERROR — le balayage se fait sur le central (en boucle locale, tous les numéros répondent)", "result");
            return;
        }
        if self.is_busy() {
            self.console("ERROR — raccroche d'abord", "result");
            return;
        }
        self.console(&format!("BALAYAGE de {} numéro(s) : {} → {}. ATH pour arrêter.", b - a + 1, num_lisible(&debut), num_lisible(&fin)), "result");
        self.balayage = Some(Balayage {
            suivant: a,
            fin: b,
            chiffres: debut.len(),
            en_cours: None,
            pause_jusqua: Instant::now(),
            composes: 0,
            porteuses: Vec::new(),
            occupes: Vec::new(),
        });
    }

    /// Résultat du numéro en cours de balayage (`None` : rien du tout).
    fn balayage_resultat(&mut self, quoi: Option<&str>, pause_ms: u64) {
        let Some(b) = self.balayage.as_mut() else { return };
        let Some((n, _)) = b.en_cours.take() else { return };
        b.pause_jusqua = Instant::now() + Duration::from_millis(pause_ms);
        match quoi {
            Some(q) => {
                if q.starts_with("OCCUPÉ") {
                    b.occupes.push(n.clone());
                } else {
                    b.porteuses.push(n.clone());
                }
                self.console(&format!("{} : {q}", num_lisible(&n)), "result");
            }
            None => self.console(&format!("{} : —", num_lisible(&n)), "info"),
        }
    }

    /// Le balayage avance : numéro suivant, ou bilan.
    fn suivre_balayage(&mut self) {
        let Some(b) = self.balayage.as_ref() else { return };
        if let Some((_, depuis)) = b.en_cours.as_ref() {
            // ça sonne sans réponse : on n'attend pas plus de 20 s
            if depuis.elapsed() > Duration::from_secs(20) {
                self.hangup();
                self.balayage_resultat(Some("SONNE, personne ne décroche"), 1500);
            }
            return;
        }
        if Instant::now() < b.pause_jusqua || self.is_busy() {
            return;
        }
        if b.suivant > b.fin {
            let b = self.balayage.take().expect("balayage");
            let p: Vec<String> = b.porteuses.iter().map(|n| num_lisible(n)).collect();
            self.console(
                &format!(
                    "BALAYAGE TERMINÉ : {} numéro(s) composé(s), {} porteuse(s){}{}",
                    b.composes,
                    p.len(),
                    if p.is_empty() { String::new() } else { format!(" : {}", p.join(", ")) },
                    if b.occupes.is_empty() { String::new() } else { format!(", {} occupé(s)", b.occupes.len()) }
                ),
                "result",
            );
            if b.composes >= 50 {
                self.exploit("wardialer");
            }
            return;
        }
        let n = format!("{:0w$}", b.suivant, w = b.chiffres);
        if let Some(b) = self.balayage.as_mut() {
            b.suivant += 1;
            b.composes += 1;
            b.en_cours = Some((n.clone(), Instant::now()));
        }
        if !self.start_outgoing(&n) {
            self.balayage_resultat(Some("ERREUR"), 1000);
        }
    }

    fn tick(&mut self) {
        self.update_transfers();
        self.suivre_balayage();
        self.pump_tunnel();
        self.ppp_step();
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

        // Réseau : entre la numérotation et le décroché du correspondant, le
        // modem n'est pas encore en ligne mais l'appel continue (retour
        // d'appel) ; sans ça, DURÉE repartait de zéro et restait à 00:00.
        if !offhook && !in_call(self.status.phase) && !self.ringback {
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
        if let ModemEvent::Fax(f) = ev {
            // (les lignes reçues sont lourdes : l'IHM les reçoit empaquetées)
            return self.on_fax(side, f);
        }
        if side == Side::Local {
            match &ev {
                ModemEvent::PhaseChanged(Phase::Retrain) => {
                    self.counters.recoveries += 1;
                    self.retrains += 1;
                }
                ModemEvent::Connected { tx_rate, rx_rate } => {
                    self.console(&format!("CONNECT {rx_rate}/V34 (TX {tx_rate})"), "result");
                    if *rx_rate > 33_600 {
                        self.exploit("v90");
                    } else if *rx_rate == 33_600 {
                        self.exploit("v34");
                    }
                    if self.session == Session::Internet && self.tunnel.is_none() {
                        self.start_tunnel(*tx_rate);
                    }
                    if self.oracle_call && self.session == Session::Bbs {
                        self.console("3615 ORACLE en ligne : ouvre la MESSAGERIE (sa LED clignote) et pose ta question", "info");
                        self.chat("system", "3615 ORACLE en ligne : tape ta question en bas, puis Entrée (la touche ENVOI). /fin pour raccrocher.".into());
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
        if std::env::var_os("V34_TRACE").is_some() {
            if let ModemEvent::Log { msg, .. } = &ev {
                eprintln!("[{:>6}] {:?} {msg}", trace_ms(), side);
            }
        }
        let _ = self.app.emit("v34:event", EventMsg { side, event: ev });
    }

    /// Télécopie : étapes, page reçue au fil de l'eau, fin. En boucle
    /// locale, c'est le correspondant simulé qui reçoit : on montre aussi
    /// sa page, pour voir que ça marche.
    fn on_fax(&mut self, side: Side, f: v34_core::fax::session::FaxEvenement) {
        use v34_core::fax::session::FaxEvenement as F;
        let local = side == Side::Local;
        match f {
            F::Etape(s) => {
                if local {
                    self.console(&format!("FAX · {s}"), "info");
                }
                let _ = self.app.emit("v34:fax", serde_json::json!({ "type": "etape", "texte": s, "local": local }));
            }
            F::Correspondant(c) => {
                if local {
                    self.console(&format!("FAX · correspondant : {c}"), "info");
                }
                self.fax_de = Some(c.clone());
                let _ = self.app.emit("v34:fax", serde_json::json!({ "type": "correspondant", "numero": c, "local": local }));
            }
            F::Resolution { fine } => {
                self.fax_fine = fine;
                self.fax_recu.clear();
                self.fax_y = 0;
                let _ = self.app.emit("v34:fax", serde_json::json!({ "type": "debut", "fine": fine, "de": self.fax_de }));
            }
            F::Ligne(pixels) => {
                // seules les lignes reçues par notre modem sont gardées (pour
                // le PNG) ; celles du correspondant simulé sont seulement montrées
                let y = self.fax_y;
                self.fax_y += 1;
                let octets = crate::fax::empaqueter(&pixels);
                if local {
                    self.fax_recu.push(pixels);
                }
                let _ = self.app.emit("v34:fax", serde_json::json!({ "type": "ligne", "y": y, "octets": octets, "fine": self.fax_fine }));
            }
            F::Progression { faites, total } => {
                let _ = self.app.emit("v34:fax", serde_json::json!({ "type": "progression", "faites": faites, "total": total }));
            }
            F::Fin { ok, raison } => {
                let recu = !self.fax_recu.is_empty();
                let mut fichier = None;
                // (en boucle locale, c'est le correspondant simulé qui a reçu :
                // on a montré sa page, sans l'enregistrer chez nous)
                if recu && local {
                    let dir = self.app.path().download_dir().unwrap_or_else(|_| std::env::temp_dir()).join("V34 reçus");
                    let png = crate::fax::png(&self.fax_recu, self.fax_fine);
                    let de = self.fax_de.clone().filter(|d| !d.is_empty()).unwrap_or_else(|| "inconnu".into());
                    let nom = format!("fax de {de}.png");
                    match crate::bbs::save_unique(&dir, &nom, &png) {
                        Ok(p) => {
                            self.console(&format!("Télécopie reçue ({} lignes) → {}", self.fax_recu.len(), p.display()), "info");
                            fichier = Some(p.display().to_string());
                        }
                        Err(e) => self.console(&format!("Télécopie reçue, mais impossible de l'enregistrer : {e}"), "info"),
                    }
                }
                self.fax_recu.clear();
                if local {
                    self.console(&format!("FAX · {raison}"), "result");
                    if ok && !recu {
                        self.exploit("fax");
                    }
                }
                self.fax_de = None;
                let _ = self.app.emit("v34:fax", serde_json::json!({ "type": "fin", "ok": ok, "raison": raison, "recu": recu, "fichier": fichier, "local": local }));
            }
        }
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
        if std::env::var_os("V34_TRACE").is_some() {
            eprintln!("[{:>6}] console {text}", trace_ms());
        }
        let _ = self.app.emit("v34:console", ConsoleLine { text: text.to_string(), kind });
    }

    fn chat(&self, from: &'static str, text: String) {
        if std::env::var_os("V34_TRACE").is_some() {
            eprintln!("[{:>6}] chat {from} « {} »", trace_ms(), text.trim_end_matches('\n').replace('\n', "⏎"));
        }
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
            Phase::Fax => "fax",
            p if in_call(p) => "connecting",
            _ => "idle",
        }
    }

    fn emit_state(&self) {
        let scope: Vec<f32> = self.scope.iter().copied().collect();
        let to_db = |r: f32| 20.0 * r.max(1e-5).log10();
        let mut status = self.status.clone();
        // LED AA, comme sur un vrai modem : au repos, allumée quand il
        // décrochera tout seul (DIP 2 « RÉP. AUTO ») ; en appel, quand c'est
        // lui qui a répondu.
        if !in_call(status.phase) {
            status.leds.aa = self.auto_answer;
        }
        let state = UiState {
            status,
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
            incoming: self.ringing_from.clone(),
            ppp: self.ppp.map(|t0| ppp_stage(t0.elapsed()).map_or("Connecté", |i| PPP_STEPS[i].1)),
            audio_device: self.speaker.device_name(),
            visio: self.visio.ui(&self.visio_ctx(), Instant::now()),
            oracle: self.oracle_call && self.session == Session::Bbs,
            taxe: self.taxe.as_ref().map(Taxe::ui),
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
        let c = modem_config(true, 56000, false);
        assert_eq!((c.max_rate, c.v90, c.max_rate_pcm), (33600, true, 56000));
        // Palier V.90 intermédiaire
        let c = modem_config(true, 45333, false);
        assert_eq!((c.max_rate, c.v90, c.max_rate_pcm), (33600, true, 45333));
        // Interrupteur 56K sur OFF : V.34 seul
        let c = modem_config(false, 56000, false);
        assert_eq!((c.max_rate, c.v90, c.max_rate_pcm), (33600, false, 56000));
        // 28 800 : V.90 encore possible (plancher à 28 000)
        let c = modem_config(true, 28800, false);
        assert_eq!((c.max_rate, c.v90, c.max_rate_pcm), (28800, true, 28800));
        // Sous 28 000 : plus de V.90, même sur ON
        let c = modem_config(true, 26400, false);
        assert_eq!((c.max_rate, c.v90, c.max_rate_pcm), (26400, false, 28000));
        let c = modem_config(true, 2400, false);
        assert_eq!((c.max_rate, c.v90, c.max_rate_pcm), (2400, false, 28000));
        // Les autres réglages gardent leurs valeurs par défaut
        let d = ModemConfig::default();
        assert_eq!(ModemConfig { max_rate: d.max_rate, v90: d.v90, max_rate_pcm: d.max_rate_pcm, ..c }, d);
        // Réglages d'usine du panneau = configuration par défaut du modem
        assert_eq!(modem_config(true, 56000, false), d);
    }
}

/// Millisecondes depuis le lancement, pour les traces V34_TRACE.
pub(crate) fn trace_ms() -> u128 {
    static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    T0.get_or_init(std::time::Instant::now).elapsed().as_millis()
}
