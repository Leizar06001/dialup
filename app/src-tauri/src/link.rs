//! Liaison entre le modem local et « l'autre bout ».
//!
//! `Link` abstrait la façon dont le son circule :
//! - `LocalLoop` (étape 1) : deux modems dans le même processus, reliés par
//!   deux simulateurs de ligne (un par sens) + l'écho local de chaque côté ;
//! - plus tard `NetLink` : un seul modem local, le son part en UDP (v34-net).

use v34_core::{
    EchoPath, LineParams, LineSim, Modem, ModemCommand, ModemConfig, ModemEvent, ModemStatus,
    Phase, Role,
};

/// Origine d'un événement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Local,
    Remote,
}

/// Rôle du modem distant simulé (boucle locale).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteService {
    /// Renvoie en écho tout ce qu'il reçoit (chat, fichiers)
    Echo,
    /// Joue le FAI : tunnel vers Internet depuis ce PC (code du serveur)
    Isp,
}

/// FAI simulé : le code du tunnel du serveur, branché sur le modem distant.
struct IspSim {
    pipe: v34_net::tunnel::ModemPipe,
}

pub trait Link: Send {
    /// Nom affiché ("boucle locale", "réseau"...).
    fn name(&self) -> &'static str;
    /// Prépare un nouvel appel (recrée les modems) avec le rôle du modem local.
    fn new_call(&mut self, local_role: Role, cfg: &ModemConfig);
    fn command_local(&mut self, cmd: ModemCommand);
    /// Commande pour le modem distant simulé (sans effet en mode réseau).
    fn command_remote(&mut self, cmd: ModemCommand);
    /// Fait avancer la liaison de `tx.len()` échantillons.
    /// `tx` reçoit ce qu'émet le modem local, `rx` ce qu'il reçoit de la ligne.
    fn process(&mut self, tx: &mut [f32], rx: &mut [f32]);
    fn local_status(&self) -> ModemStatus;
    /// Phase du modem distant si elle est connue (boucle locale).
    fn remote_phase(&self) -> Option<Phase>;
    fn drain_events(&mut self) -> Vec<(Side, ModemEvent)>;
    fn set_line(&mut self, p: &LineParams);
    fn line_response(&self, points: usize) -> Vec<(f32, f32)>;
    /// Rôle du distant simulé (sans effet en mode réseau).
    fn set_remote_service(&mut self, _svc: RemoteService) {}
    /// Mode réseau (central) ?
    fn is_network(&self) -> bool {
        false
    }
    /// Correction de cadence à chaque tic de 20 ms : +1 = traiter un bloc de
    /// plus, −1 = un de moins (réseau : on suit l'horloge du central).
    fn pace(&mut self) -> f64 {
        0.0
    }
    /// Réseau : redemander l'annuaire au central.
    fn net_refresh_directory(&mut self) {}
    /// Réseau : appeler ce numéro via le central.
    fn net_dial(&mut self, _number: &str) {}
    /// Réseau : décrocher l'appel entrant.
    fn net_answer(&mut self) {}
    /// Réseau : raccrocher côté central.
    fn net_hangup(&mut self) {}
    /// Réseau : événements de signalisation.
    fn poll_net(&mut self) -> Vec<NetEvent> {
        Vec::new()
    }
    /// Réseau : état de l'enregistrement.
    fn net_info(&self) -> Option<NetInfo> {
        None
    }
}

/// Événements du central, traduits pour le moteur.
#[derive(Clone, Debug)]
pub enum NetEvent {
    Registered { number: String },
    AuthFailed(String),
    ServerLost,
    Incoming { from: String },
    Ringing,
    Answered,
    Busy(String),
    Unknown(String),
    HungUp(v34_net::signal::HangupReason, String),
}

/// État de la connexion au central (pour l'IHM).
#[derive(Clone, Debug, serde::Serialize)]
pub struct NetInfo {
    pub server: String,
    pub user: String,
    pub number: Option<String>,
    pub registered: bool,
    pub error: Option<String>,
    pub isp_number: Option<String>,
    /// Statistiques audio de l'appel (paquets, gigue, pertes…)
    pub stats: Option<v34_net::NetStats>,
    /// Aller-retour vers le central (ms)
    pub rtt_ms: Option<f32>,
    /// Annuaire du central et numéros spéciaux (FAI, ligne de test)
    pub directory: Vec<v34_net::signal::DirEntry>,
    pub services: Vec<(String, String)>,
}

/// Mode réseau : un seul modem local, le son passe par le central
/// (dialup.tinyverse.fr). Le simulateur de ligne représente notre boucle
/// locale : il s'applique au son reçu ET au son émis (la boucle abîme les
/// deux sens), et l'écho de notre propre signal est ajouté localement.
pub struct NetLink {
    central: v34_net::central::CentralClient,
    modem: Modem,
    line: LineSim,
    /// Boucle locale dans le sens montant (notre émission vers le central)
    line_up: LineSim,
    up_buf: Vec<f32>,
    echo: EchoPath,
    last_tx: Vec<f32>,
    incoming: Vec<f32>,
    /// Le modem appelant démarre quand l'autre a décroché
    pending_dial: bool,
    info: NetInfo,
    /// Cadence calée sur l'horloge du central
    pacer: v34_net::Pacer,
    /// V34_TRACE : derniers compteurs réseau affichés
    trace_prev: Option<(u64, u64, u64, u64, u64, u64, u64, u64)>,
    /// V34_DUMP=fichier : (reçu du réseau, entrée du modem, sortie du modem) en f32
    dump: Option<std::io::BufWriter<std::fs::File>>,
}

impl NetLink {
    pub fn connect(server: &str, user: &str, password: &str, params: &LineParams) -> std::io::Result<Self> {
        let cfg = v34_net::central::CentralConfig::new(server, user, password);
        let central = v34_net::central::CentralClient::connect(cfg)?;
        Ok(Self {
            central,
            modem: Modem::new(Role::Call, ModemConfig::default()),
            line: LineSim::new(params.clone()),
            line_up: LineSim::with_seed(params.clone(), 0x5eed_0bad),
            up_buf: Vec::new(),
            echo: EchoPath::new(params.clone()),
            last_tx: Vec::new(),
            incoming: Vec::new(),
            pending_dial: false,
            pacer: v34_net::Pacer::new(),
            trace_prev: None,
            dump: std::env::var_os("V34_DUMP").and_then(|p| std::fs::File::create(p).ok()).map(std::io::BufWriter::new),
            info: NetInfo { server: server.to_string(), user: user.to_string(), number: None, registered: false, error: None, isp_number: None, stats: None, rtt_ms: None, directory: Vec::new(), services: Vec::new() },
        })
    }

    fn call_active(&self) -> bool {
        matches!(self.central.call_state(), v34_net::central::CallState::Active { .. })
    }
}

impl Link for NetLink {
    fn name(&self) -> &'static str {
        "réseau"
    }

    fn new_call(&mut self, local_role: Role, cfg: &ModemConfig) {
        self.modem = Modem::new(local_role, cfg.clone());
        self.last_tx.fill(0.0);
        self.pending_dial = false;
    }

    fn command_local(&mut self, cmd: ModemCommand) {
        let hang = matches!(cmd, ModemCommand::Hangup);
        self.modem.command(cmd);
        if hang && self.modem.status().phase == Phase::Idle {
            let _ = self.central.hangup();
        }
    }

    fn command_remote(&mut self, _cmd: ModemCommand) {}

    fn process(&mut self, tx: &mut [f32], rx: &mut [f32]) {
        let n = tx.len();
        resize(&mut self.last_tx, n);
        resize(&mut self.incoming, n);
        let _ = self.central.poll();
        let active = self.call_active();
        // Hors appel : silence. Pendant l'appel, recv_block met déjà du
        // silence à la place des trous (on garde le vrai son autour).
        if !active {
            self.incoming.fill(0.0);
        } else {
            self.central.recv_block(&mut self.incoming);
        }
        // Ligne simulée sur le son reçu + écho de notre propre émission
        self.line.process(&self.incoming, rx);
        self.echo.process(&self.last_tx, rx);
        self.modem.process(rx, tx);
        if active {
            // notre émission traverse elle aussi la boucle locale
            resize(&mut self.up_buf, n);
            self.line_up.process(tx, &mut self.up_buf);
            let _ = self.central.send_block(&self.up_buf);
        }
        self.last_tx.copy_from_slice(tx);
        if active {
            if let Some(f) = self.dump.as_mut() {
                use std::io::Write;
                let mut buf = Vec::with_capacity(n * 12);
                for i in 0..n {
                    for v in [self.incoming[i], rx[i], tx[i]] {
                        buf.extend_from_slice(&v.to_le_bytes());
                    }
                }
                let _ = f.write_all(&buf);
            }
        }
        if active && trace_on() {
            let j = self.central.stats().jitter;
            let k = (j.lost, j.late, j.underruns, j.dropped, j.resyncs, self.pacer.extra, self.pacer.skipped, j.recovered);
            if self.trace_prev.is_some_and(|p| p != k) {
                eprintln!("[réseau] perdus {} · rattrapés {} · en retard {} · trous {} · jetés {} · recalages {} · blocs +{} −{} · tampon {:.0}/{:.0} ms · gigue {:.1} ms", k.0, k.7, k.1, k.2, k.3, k.4, k.5, k.6, j.depth_ms, j.target_ms, j.jitter_ms);
            }
            self.trace_prev = Some(k);
        }
    }

    fn local_status(&self) -> ModemStatus {
        self.modem.status()
    }

    fn remote_phase(&self) -> Option<Phase> {
        None
    }

    fn drain_events(&mut self) -> Vec<(Side, ModemEvent)> {
        let evs: Vec<(Side, ModemEvent)> = self.modem.drain_events().into_iter().map(|e| (Side::Local, e)).collect();
        // Le modem a raccroché (fin d'un raccrochage propre, échec…) : on libère la ligne
        if evs.iter().any(|(_, e)| matches!(e, ModemEvent::Disconnected { .. })) && self.central.call_state().call_id().is_some() {
            let _ = self.central.hangup();
        }
        evs
    }

    fn set_line(&mut self, p: &LineParams) {
        self.line.set_params(p.clone());
        self.line_up.set_params(p.clone());
        self.echo.set_params(p.clone());
    }

    fn line_response(&self, points: usize) -> Vec<(f32, f32)> {
        self.line.frequency_response(points)
    }

    fn is_network(&self) -> bool {
        true
    }

    /// Le central (le FAI, l'autre abonné) donne l'horloge : on garde notre
    /// tampon de réception autour de sa cible en traitant ponctuellement un
    /// bloc de plus ou de moins. Les deux modems voient ainsi exactement les
    /// mêmes échantillons, sans glissement, quelle que soit la dérive des
    /// horloges des deux machines.
    fn pace(&mut self) -> f64 {
        self.pacer.update(self.central.jitter_level())
    }

    fn net_refresh_directory(&mut self) {
        self.central.refresh_directory();
    }

    fn net_dial(&mut self, number: &str) {
        self.pending_dial = true;
        if let Err(e) = self.central.dial(number) {
            self.info.error = Some(format!("appel impossible : {e}"));
        }
    }

    fn net_answer(&mut self) {
        let _ = self.central.answer();
        self.modem.command(ModemCommand::Answer);
    }

    fn net_hangup(&mut self) {
        let _ = self.central.hangup();
        self.pending_dial = false;
    }

    fn poll_net(&mut self) -> Vec<NetEvent> {
        use v34_net::central::CentralEvent as E;
        let mut out = Vec::new();
        while let Some(ev) = self.central.next_event() {
            out.push(match ev {
                E::Registered { number, services } => {
                    self.info.registered = true;
                    self.info.error = None;
                    self.info.number = Some(number.clone());
                    self.info.isp_number = services.iter().find(|(_, name)| name.to_lowercase().contains("fai") || name.to_lowercase().contains("isp")).map(|(n, _)| n.clone());
                    NetEvent::Registered { number }
                }
                E::AuthFailed { reason } => {
                    self.info.registered = false;
                    let r = format!("{reason:?}");
                    self.info.error = Some(r.clone());
                    NetEvent::AuthFailed(r)
                }
                E::ServerLost => {
                    self.info.registered = false;
                    NetEvent::ServerLost
                }
                E::Incoming { from, .. } => NetEvent::Incoming { from },
                E::Ringing { .. } => NetEvent::Ringing,
                E::Answered { .. } => {
                    if self.pending_dial {
                        self.pending_dial = false;
                        self.modem.command(ModemCommand::Dial);
                    }
                    NetEvent::Answered
                }
                E::Busy { number } => {
                    self.pending_dial = false;
                    NetEvent::Busy(number)
                }
                E::UnknownNumber { number } => {
                    self.pending_dial = false;
                    NetEvent::Unknown(number)
                }
                E::HungUp { reason, .. } => {
                    self.pending_dial = false;
                    self.modem.command(ModemCommand::Hangup);
                    NetEvent::HungUp(reason, match reason {
                        v34_net::signal::HangupReason::Forbidden => {
                            "accès Internet non activé pour ce compte : l'opérateur doit l'accorder (voir le site du fournisseur d'accès)".to_string()
                        }
                        r => r.describe().to_string(),
                    })
                }
            });
        }
        out
    }

    fn net_info(&self) -> Option<NetInfo> {
        let mut i = self.info.clone();
        i.stats = self.call_active().then(|| self.central.stats());
        i.rtt_ms = self.central.rtt_ms();
        i.directory = self.central.directory().to_vec();
        i.services = self.central.services().to_vec();
        Some(i)
    }
}

/// Deux modems dans le même processus.
pub struct LocalLoop {
    local: Modem,
    remote: Modem,
    /// local → distant
    line_lr: LineSim,
    /// distant → local
    line_rl: LineSim,
    echo_local: EchoPath,
    echo_remote: EchoPath,
    /// Dernier bloc émis par chaque modem (le son met un bloc à traverser).
    last_tx_local: Vec<f32>,
    last_tx_remote: Vec<f32>,
    scratch: Vec<f32>,
    /// Le distant renvoie ce qu'il reçoit (boucle de test façon V.54).
    pub remote_echo_data: bool,
    service: RemoteService,
    isp: Option<IspSim>,
}

impl LocalLoop {
    pub fn new(params: &LineParams) -> Self {
        let cfg = ModemConfig::default();
        Self {
            local: Modem::new(Role::Call, cfg.clone()),
            remote: Modem::new(Role::Answer, cfg),
            line_lr: LineSim::new(params.clone()),
            line_rl: LineSim::new(params.clone()),
            echo_local: EchoPath::new(params.clone()),
            echo_remote: EchoPath::new(params.clone()),
            last_tx_local: Vec::new(),
            last_tx_remote: Vec::new(),
            scratch: Vec::new(),
            remote_echo_data: true,
            service: RemoteService::Echo,
            isp: None,
        }
    }
}

fn resize(v: &mut Vec<f32>, n: usize) {
    if v.len() != n {
        v.clear();
        v.resize(n, 0.0);
    }
}

impl Link for LocalLoop {
    fn name(&self) -> &'static str {
        "boucle locale"
    }

    fn new_call(&mut self, local_role: Role, cfg: &ModemConfig) {
        let remote_role = match local_role {
            Role::Call => Role::Answer,
            Role::Answer => Role::Call,
        };
        self.local = Modem::new(local_role, cfg.clone());
        self.remote = Modem::new(remote_role, cfg.clone());
        self.last_tx_local.fill(0.0);
        self.last_tx_remote.fill(0.0);
        self.isp = None;
    }

    fn command_local(&mut self, cmd: ModemCommand) {
        self.local.command(cmd);
    }

    fn command_remote(&mut self, cmd: ModemCommand) {
        self.remote.command(cmd);
    }

    fn process(&mut self, tx: &mut [f32], rx: &mut [f32]) {
        let n = tx.len();
        resize(&mut self.last_tx_local, n);
        resize(&mut self.last_tx_remote, n);
        resize(&mut self.scratch, n);

        // Ce qui arrive chez le local : signal distant à travers la ligne + écho de son propre signal.
        self.line_rl.process(&self.last_tx_remote, rx);
        self.echo_local.process(&self.last_tx_local, rx);

        // Ce qui arrive chez le distant.
        let mut rx_remote = std::mem::take(&mut self.scratch);
        self.line_lr.process(&self.last_tx_local, &mut rx_remote);
        self.echo_remote.process(&self.last_tx_remote, &mut rx_remote);

        self.local.process(rx, tx);
        if let Some(isp) = self.isp.as_mut() {
            if let Some(bytes) = isp.pipe.take_for_modem() {
                self.remote.command(ModemCommand::Send(bytes));
            }
        }
        self.remote.process(&rx_remote, &mut self.last_tx_remote);
        self.last_tx_local.copy_from_slice(tx);
        self.scratch = rx_remote;
    }

    fn local_status(&self) -> ModemStatus {
        self.local.status()
    }

    fn remote_phase(&self) -> Option<Phase> {
        Some(self.remote.status().phase)
    }

    fn drain_events(&mut self) -> Vec<(Side, ModemEvent)> {
        let mut out: Vec<(Side, ModemEvent)> =
            self.local.drain_events().into_iter().map(|e| (Side::Local, e)).collect();
        for ev in self.remote.drain_events() {
            match &ev {
                ModemEvent::Connected { tx_rate, .. } if self.service == RemoteService::Isp && self.isp.is_none() => {
                    // Le distant devient FAI : même code que le serveur du VPS
                    let (pipe, from_modem, to_modem) = v34_net::tunnel::ModemPipe::new(*tx_rate);
                    let fai = v34_server::config::Fai { banniere: "V34NET (FAI simulé en boucle locale) - bonne navigation !".into(), ..Default::default() };
                    if let Ok(policy) = v34_server::tunnel::TunnelPolicy::from_config(&fai) {
                        let policy = std::sync::Arc::new(policy);
                        let _ = std::thread::Builder::new().name("fai-simule".into()).spawn(move || {
                            v34_server::tunnel::run(policy, "boucle locale".into(), "local", from_modem, to_modem)
                        });
                        self.isp = Some(IspSim { pipe });
                    }
                }
                ModemEvent::Data(bytes) => {
                    if let Some(isp) = self.isp.as_mut() {
                        isp.pipe.on_modem_data(bytes.clone());
                    } else if self.remote_echo_data {
                        self.remote.command(ModemCommand::Send(bytes.clone()));
                    }
                }
                ModemEvent::Disconnected { .. } => self.isp = None,
                _ => {}
            }
            out.push((Side::Remote, ev));
        }
        out
    }

    fn set_remote_service(&mut self, svc: RemoteService) {
        self.service = svc;
        self.isp = None;
        if svc == RemoteService::Isp && self.remote.role() == Role::Answer {
            // Le FAI simulé est raccordé en numérique, comme celui du
            // central : il propose le 56k (V.90) si notre modem le demande.
            let cfg = ModemConfig { digital: true, ..ModemConfig::default() };
            self.remote = Modem::new(Role::Answer, cfg);
        }
    }

    fn set_line(&mut self, p: &LineParams) {
        self.line_lr.set_params(p.clone());
        self.line_rl.set_params(p.clone());
        self.echo_local.set_params(p.clone());
        self.echo_remote.set_params(p.clone());
    }

    fn line_response(&self, points: usize) -> Vec<(f32, f32)> {
        self.line_rl.frequency_response(points)
    }
}

/// V34_TRACE : traces de diagnostic sur la sortie d'erreur
pub fn trace_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("V34_TRACE").is_some())
}
