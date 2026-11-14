//! Modems des numéros spéciaux : un `v34_core::Modem` en rôle appelé par
//! appel, cadencé en temps réel (blocs de 20 ms) dans son propre thread.
//!
//! L'audio de l'appelant lui est remis par le central (canal), et il répond
//! directement à l'adresse de l'appelant par la socket du central (clone).
//! Une fois CONNECT obtenu, le flux d'octets du modem est raccordé par un
//! [`ModemPipe`] au service :
//! - [`Service::Fai`] : bout serveur du tunnel ([`crate::tunnel`]) ;
//! - [`Service::Echo`] : ligne de test, renvoie tout ce qu'elle reçoit ;
//! - [`Service::Oracle`] : 3615 ORACLE ([`crate::oracle`]), questions et
//!   réponses en trames de chat, comme un abonné.
//!
//! Pour ajouter un service (BBS…) : une variante de [`Service`] dans la
//! configuration et un thread qui consomme/produit des octets ici.

use std::net::{SocketAddr, UdpSocket};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use v34_core::{LineParams, LineSim, Modem, ModemCommand, ModemConfig, ModemEvent, Role};
use v34_net::jitter::{Concealment, JitterBuffer};
use v34_net::signal::HangupReason;
use v34_net::tunnel::ModemPipe;
use v34_net::{Codec, Packet, FRAME_SAMPLES};

use crate::config::Service;
use crate::oracle::{self, Oracle};
use crate::tunnel::{self, TunnelPolicy};
use crate::{debug, info, warn};

const PERIOD: Duration = Duration::from_millis(20);
/// Profondeur du tampon de gigue côté FAI (ms)
const MODEM_JITTER_MS: u32 = 140;
/// Temps laissé au modem pour émettre ses derniers octets avant de raccrocher.
const FLUSH_GRACE: Duration = Duration::from_millis(1500);
/// Durée maximale du raccrochage propre (cleardown).
const CLEARDOWN_MAX: Duration = Duration::from_secs(4);

/// Ce que le central envoie au thread d'un appel vers un numéro spécial.
pub enum ModemInput {
    Audio { pkt: Packet, from: SocketAddr },
    Hangup,
}

/// Ce que le thread signale au central.
#[derive(Debug)]
pub enum ModemNotice {
    Ended { call_id: u32, reason: HangupReason, detail: String },
}

pub struct ModemCall {
    pub call_id: u32,
    pub service: Service,
    /// Nom de l'abonné appelant (déjà authentifié par le central)
    pub caller: String,
    pub caller_number: String,
    pub peer: SocketAddr,
    pub sock: UdpSocket,
    pub max_rate: u32,
    /// 56k (V.90) proposé, et son débit descendant maximal
    pub v90: bool,
    pub max_rate_pcm: u32,
    pub carrier_timeout: Duration,
    pub line: Option<LineParams>,
    pub policy: Arc<TunnelPolicy>,
    pub oracle: Arc<Oracle>,
    /// Dossier de données du central (numéros cachés : répondeur, scores)
    pub donnees: Option<std::path::PathBuf>,
    /// Succès gagnés dans les numéros cachés (abonné, succès), relevés par le central
    pub exploits: crate::caches::Exploits,
}

pub fn spawn(call: ModemCall, rx: Receiver<ModemInput>, notify: Sender<ModemNotice>) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new().name(format!("modem-{:08x}", call.call_id)).spawn(move || {
        let call_id = call.call_id;
        let (reason, detail) = run(call, rx);
        let _ = notify.send(ModemNotice::Ended { call_id, reason, detail });
    })
}

/// Ligne de test : message d'accueil, puis écho de tout ce qui arrive.
fn echo(from_modem: Receiver<Vec<u8>>, to_modem: std::sync::mpsc::SyncSender<Vec<u8>>) -> String {
    let hello = b"\r\nLigne de test V34NET : tout ce que vous envoyez revient en echo.\r\n".to_vec();
    if to_modem.send(hello).is_err() {
        return "liaison modem terminée".into();
    }
    for bytes in from_modem {
        if to_modem.send(bytes).is_err() {
            break;
        }
    }
    "liaison modem terminée".into()
}

fn run(call: ModemCall, rx: Receiver<ModemInput>) -> (HangupReason, String) {
    let label = format!("{} {:08x} ({} / {})", call.service.name().to_uppercase(), call.call_id, call.caller, call.caller_number);
    // Le FAI est raccordé en numérique : il peut jouer le modem numérique
    // du 56k (V.90) si l'abonné le demande.
    let cfg = ModemConfig { max_rate: call.max_rate, v90: call.v90, digital: true, max_rate_pcm: call.max_rate_pcm, ..ModemConfig::default() };
    let mut modem = Modem::new(Role::Answer, cfg);
    modem.command(ModemCommand::Answer);
    info!("{label} : décroché");

    let epoch = Instant::now();
    // Tampon « modem » : profond, sans glissement. Le serveur est l'horloge
    // de référence (comme un central) : c'est le client qui se cale sur lui.
    let mut jb = JitterBuffer::new(MODEM_JITTER_MS, Concealment::Modem);
    let mut sim = call.line.clone().map(LineSim::new);
    let mut peer = call.peer;
    let mut codec = Codec::Alaw;
    let (mut seq, mut ts) = (0u32, 0u32);
    let mut tx = vec![0.0f32; FRAME_SAMPLES];
    let mut tx_line = vec![0.0f32; FRAME_SAMPLES];
    let mut tx_hist: std::collections::VecDeque<Vec<f32>> = std::collections::VecDeque::new();

    let mut pipe: Option<ModemPipe> = None;
    let mut tunnel: Option<JoinHandle<String>> = None;
    let mut connected = false;
    let mut hangup_at: Option<Instant> = None;
    let mut hard_stop: Option<Instant> = None;
    let mut outcome: Option<(HangupReason, String)> = None;
    let mut next = Instant::now();

    let result = loop {
        // 1. audio et ordres du central
        let mut caller_gone = false;
        loop {
            match rx.try_recv() {
                Ok(ModemInput::Audio { pkt, from }) => {
                    peer = from;
                    codec = pkt.codec;
                    jb.push_with_backup(pkt.seq, pkt.timestamp, pkt.samples, &pkt.backup, epoch.elapsed().as_secs_f64());
                }
                Ok(ModemInput::Hangup) | Err(TryRecvError::Disconnected) => {
                    caller_gone = true;
                    break;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        if caller_gone {
            break (HangupReason::Normal, "raccroché par l'appelant".to_string());
        }

        // 2. un bloc de modem
        let (frame, _) = jb.pop_frame();
        modem.process(&frame, &mut tx);
        let out: &[f32] = match sim.as_mut() {
            Some(s) => {
                s.process(&tx, &mut tx_line);
                &tx_line
            }
            None => &tx,
        };
        // chaque paquet répète les deux trames précédentes : un paquet perdu
        // en route est rattrapé avec le suivant (le 56k ne supporte pas un
        // trou de 20 ms)
        let pkt = Packet { codec, call_id: call.call_id, seq, timestamp: ts, samples: out.to_vec(), backup: tx_hist.iter().cloned().collect() };
        let _ = call.sock.send_to(&pkt.encode(), peer);
        tx_hist.push_front(out.to_vec());
        tx_hist.truncate(v34_net::packet::BACKUP_FRAMES);
        seq = seq.wrapping_add(1);
        ts = ts.wrapping_add(FRAME_SAMPLES as u32);

        // 3. événements du modem
        let now = Instant::now();
        for ev in modem.drain_events() {
            match ev {
                ModemEvent::Connected { tx_rate, rx_rate } => {
                    info!("{label} : CONNECT {tx_rate}/{rx_rate} bit/s");
                    connected = true;
                    let (p, tin, tout) = ModemPipe::new(tx_rate);
                    let policy = call.policy.clone();
                    let orc = call.oracle.clone();
                    let l = label.clone();
                    let user = call.caller.clone();
                    let service = call.service;
                    let donnees = call.donnees.clone();
                    let exploits = call.exploits.clone();
                    match std::thread::Builder::new().name(format!("svc-{:08x}", call.call_id)).spawn(move || match service {
                        Service::Fai => tunnel::run(policy, l, &user, tin, tout),
                        Service::Echo => echo(tin, tout),
                        Service::Oracle => oracle::session(orc, &l, &user, tin, tout),
                        Service::Cache(i) => crate::caches::session(i, &l, &user, donnees.as_deref(), &exploits, tin, tout),
                    }) {
                        Ok(h) => {
                            tunnel = Some(h);
                            pipe = Some(p);
                        }
                        Err(e) => {
                            warn!("{label} : impossible de lancer le service : {e}");
                            hangup_at.get_or_insert(now);
                        }
                    }
                }
                ModemEvent::Data(d) => {
                    if let Some(p) = pipe.as_mut() {
                        p.on_modem_data(d);
                    }
                }
                ModemEvent::Disconnected { reason } => {
                    outcome.get_or_insert_with(|| {
                        if connected {
                            (HangupReason::Normal, format!("NO CARRIER ({reason})"))
                        } else {
                            (HangupReason::Failure, format!("échec de la négociation : {reason}"))
                        }
                    });
                }
                ModemEvent::PhaseChanged(p) => debug!("{label} : phase {p:?}"),
                ModemEvent::Log { msg, .. } => debug!("{label} : modem : {msg}"),
                _ => {}
            }
        }
        if let Some(o) = outcome.take() {
            break o;
        }

        // 4. octets vers le modem, fin du tunnel
        if let Some(p) = pipe.as_mut() {
            if let Some(bytes) = p.take_for_modem() {
                modem.command(ModemCommand::Send(bytes));
            }
            if p.tunnel_finished() && hangup_at.is_none() {
                if let Some(h) = tunnel.take() {
                    let end = h.join().unwrap_or_else(|_| "service arrêté".into());
                    info!("{label} : fin du service ({end}), raccrochage");
                }
                hangup_at = Some(now + FLUSH_GRACE);
            }
        }
        if let Some(t) = hangup_at {
            if now >= t && hard_stop.is_none() {
                modem.command(ModemCommand::Hangup);
                hard_stop = Some(now + CLEARDOWN_MAX);
            }
        }
        if hard_stop.is_some_and(|t| now >= t) {
            break (HangupReason::Normal, "raccroché par le central".into());
        }
        if !connected && epoch.elapsed() > call.carrier_timeout {
            break (HangupReason::Failure, "pas de porteuse".into());
        }

        // 5. cadence de 20 ms
        next += PERIOD;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else if now - next > Duration::from_secs(2) {
            // Très gros retard (machine surchargée) : on abandonne le
            // rattrapage. Sinon on rattrape bloc par bloc, sans sauter de
            // temps de ligne (un saut = glissement pour les deux modems).
            next = now;
        }
    };
    // Fermer le tuyau arrête le tunnel (s'il tourne encore) et ses connexions.
    drop(pipe);
    if let Some(h) = tunnel.take() {
        let _ = h.join();
    }
    info!("{label} : terminé ({})", result.1);
    result
}
