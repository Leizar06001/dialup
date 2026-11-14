//! Modem contre le central à travers un réseau « Wi-Fi » simulé : délais
//! variables, gels de 60 à 150 ms puis rafales, quelques pertes.
//!
//! Le test tourne en temps réel (~30 s), d'où `#[ignore]` :
//!     cargo test -p v34-server --test gigue -- --ignored --nocapture

use std::collections::VecDeque;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use v34_core::{Modem, ModemCommand, ModemConfig, ModemEvent, Role};
use v34_net::auth::PasswordHash;
use v34_net::central::{CallState, CentralClient, CentralConfig, CentralEvent};
use v34_net::{Concealment, Pacer, FRAME_SAMPLES};
use v34_server::log::{set_level, Level};
use v34_server::{Config, Server, ServerHandle};

const USER: (&str, &str) = ("charlie", "mot-de-passe-charlie");
const ECHO: &str = "0800000000";

fn start_server() -> ServerHandle {
    set_level(Level::Error);
    let h = PasswordHash::new(USER.1, 1000);
    let toml = format!(
        r#"
        [serveur]
        ecoute = "127.0.0.1:0"
        journal = "error"
        [modems]
        delai_decroche_ms = 100
        [[abonnes]]
        nom = "{}"
        numero = "0700000000"
        mot_de_passe_hash = "{h}"
        "#,
        USER.0
    );
    Server::bind(Config::from_toml(&toml).expect("configuration de test")).unwrap().spawn().unwrap()
}

/// Générateur pseudo-aléatoire minimal (xorshift), reproductible.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Un sens du lien « Wi-Fi » : ordre conservé, délai de base + gigue, et de
/// temps en temps un gel (tout attend, puis repart en rafale).
struct Direction {
    queue: VecDeque<(f64, Vec<u8>)>,
    last_depart: f64,
    frozen_until: f64,
    rng: Rng,
    lost: u64,
}

impl Direction {
    fn new(seed: u64) -> Self {
        Self { queue: VecDeque::new(), last_depart: 0.0, frozen_until: 0.0, rng: Rng(seed), lost: 0 }
    }
    fn push(&mut self, now: f64, bytes: Vec<u8>) {
        let audio = bytes.len() > 100;
        if audio && self.rng.next() < 0.003 {
            self.lost += 1;
            return;
        }
        if self.rng.next() < 1.0 / 150.0 {
            self.frozen_until = now + 0.060 + 0.090 * self.rng.next();
        }
        let depart = (now + 0.015 + 0.020 * self.rng.next()).max(self.frozen_until).max(self.last_depart);
        self.last_depart = depart;
        self.queue.push_back((depart, bytes));
    }
}

struct Proxy {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<(u64, u64)>>,
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Relais UDP entre le client et le central, avec le comportement Wi-Fi.
fn start_proxy(server: SocketAddr) -> Proxy {
    let front = UdpSocket::bind("127.0.0.1:0").unwrap();
    let back = UdpSocket::bind("127.0.0.1:0").unwrap();
    front.set_nonblocking(true).unwrap();
    back.set_nonblocking(true).unwrap();
    let addr = front.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let s2 = stop.clone();
    let thread = std::thread::spawn(move || {
        let t0 = Instant::now();
        let (mut up, mut down) = (Direction::new(0x9E37_79B9_7F4A_7C15), Direction::new(0xD1B5_4A32_D192_ED03));
        let mut client: Option<SocketAddr> = None;
        let mut buf = [0u8; 2048];
        while !s2.load(Ordering::Relaxed) {
            let now = t0.elapsed().as_secs_f64();
            while let Ok((n, from)) = front.recv_from(&mut buf) {
                client = Some(from);
                up.push(now, buf[..n].to_vec());
            }
            while let Ok((n, _)) = back.recv_from(&mut buf) {
                down.push(now, buf[..n].to_vec());
            }
            while up.queue.front().is_some_and(|(t, _)| *t <= now) {
                let (_, b) = up.queue.pop_front().unwrap();
                let _ = back.send_to(&b, server);
            }
            while down.queue.front().is_some_and(|(t, _)| *t <= now) {
                let (_, b) = down.queue.pop_front().unwrap();
                if let Some(c) = client {
                    let _ = front.send_to(&b, c);
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        (up.lost, down.lost)
    });
    Proxy { addr, stop, thread: Some(thread) }
}

struct Outcome {
    connected: Option<(u32, u32)>,
    echoed: bool,
    seconds_to_echo: f32,
    problems: Vec<String>,
    jitter: v34_net::JitterStats,
    pacer: (u64, u64),
}

/// Appelle la ligne de test (écho) à travers le relais, envoie `payload` dès
/// la connexion et attend qu'il revienne.
fn echo_call(old_style: bool, payload: &[u8]) -> Outcome {
    let server = start_server();
    let proxy = start_proxy(server.local_addr());
    let mut cfg = CentralConfig::new(proxy.addr.to_string(), USER.0, USER.1);
    if old_style {
        // Comportement d'avant : tampon « voix sur IP » de 60 ms.
        cfg.jitter_target_ms = 60;
        cfg.concealment = Concealment::Repeat;
    }
    let mut c = CentralClient::connect(cfg).unwrap();
    let mut modem = Modem::new(Role::Call, ModemConfig::default());
    let mut pacer = Pacer::new();
    let (mut rx, mut tx) = (vec![0.0f32; FRAME_SAMPLES], vec![0.0f32; FRAME_SAMPLES]);
    let mut out = Outcome { connected: None, echoed: false, seconds_to_echo: 0.0, problems: Vec::new(), jitter: Default::default(), pacer: (0, 0) };
    let mut got = Vec::new();
    let mut dialed = false;
    let t0 = Instant::now();
    let mut connected_at: Option<Instant> = None;
    let mut next = Instant::now();
    let mut due = 0.0;
    while t0.elapsed() < Duration::from_secs(45) {
        let _ = c.poll();
        while let Some(ev) = c.next_event() {
            match ev {
                CentralEvent::Registered { .. } if !dialed => {
                    dialed = true;
                    c.dial(ECHO).unwrap();
                }
                CentralEvent::Answered { .. } => modem.command(ModemCommand::Dial),
                CentralEvent::HungUp { reason, .. } => out.problems.push(format!("raccroché : {reason:?}")),
                _ => {}
            }
        }
        let active = matches!(c.call_state(), CallState::Active { .. });
        due += 1.0 + if old_style { 0.0 } else { pacer.update(c.jitter_level()) };
        while due >= 1.0 {
            if active {
                c.recv_block(&mut rx);
            } else {
                rx.fill(0.0);
            }
            modem.process(&rx, &mut tx);
            if active {
                let _ = c.send_block(&tx);
            }
            due -= 1.0;
        }
        for ev in modem.drain_events() {
            match ev {
                ModemEvent::Connected { tx_rate, rx_rate } => {
                    out.connected.get_or_insert((tx_rate, rx_rate));
                    connected_at.get_or_insert_with(Instant::now);
                    modem.command(ModemCommand::Send(payload.to_vec()));
                }
                ModemEvent::Data(d) => got.extend(d),
                ModemEvent::Disconnected { reason } => out.problems.push(format!("NO CARRIER {reason}")),
                ModemEvent::Log { msg, .. } if msg.contains("renégociation") || msg.contains("Retrain") || msg.contains("retrain") || msg.contains("réapprentissage") => {
                    out.problems.push(msg)
                }
                _ => {}
            }
        }
        if got.ends_with(payload) {
            out.echoed = true;
            out.seconds_to_echo = connected_at.map_or(0.0, |t| t.elapsed().as_secs_f32());
            break;
        }
        next += Duration::from_millis(20);
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        }
    }
    out.jitter = c.stats().jitter;
    out.pacer = (pacer.extra, pacer.skipped);
    let _ = c.hangup();
    out
}

fn payload() -> Vec<u8> {
    let motif = b"Le modem traverse le Wi-Fi sans glisser. ";
    (0..4000).map(|i| motif[i % motif.len()]).collect()
}

fn report(label: &str, o: &Outcome) {
    eprintln!(
        "{label} : CONNECT {:?}, écho {} en {:.1} s, problèmes {:?}\n  gigue {:.1} ms, sous-débits {}, retards {}, pertes {}, recalages {}, trames jetées {}, cadence +{} / −{}",
        o.connected, if o.echoed { "OK" } else { "ÉCHEC" }, o.seconds_to_echo, o.problems,
        o.jitter.jitter_ms, o.jitter.underruns, o.jitter.late, o.jitter.lost, o.jitter.resyncs, o.jitter.dropped, o.pacer.0, o.pacer.1
    );
}

#[test]
#[ignore = "temps réel (~30 s)"]
fn modem_survit_a_la_gigue_wifi() {
    let p = payload();
    let o = echo_call(false, &p);
    report("tampon modem", &o);
    assert!(o.connected.is_some(), "pas de CONNECT : {:?}", o.problems);
    assert!(o.echoed, "l'écho n'est pas revenu : {:?}", o.problems);
    assert_eq!(o.jitter.resyncs, 0, "glissement");
    assert_eq!(o.jitter.dropped, 0, "trames jetées");
}

/// Comparaison avec l'ancien tampon (pas d'assertion : il échoue souvent).
#[test]
#[ignore = "temps réel, comparaison"]
fn ancien_tampon_sous_la_meme_gigue() {
    let o = echo_call(true, &payload());
    report("ancien tampon", &o);
}
