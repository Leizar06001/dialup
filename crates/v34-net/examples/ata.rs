//! Banc « vrai modem derrière un adaptateur » : un modem à SON quartz,
//! branché sur un ATA (horloge à lui, tampon de gigue fixe, écho de son
//! hybride), relié par Internet (délai, gigue, coupures d'une demi-seconde
//! suivies de la rafale des paquets bloqués) à notre modem, qui reçoit par
//! le vrai [`JitterBuffer`] et suit l'ATA avec le vrai [`Pacer`], comme le
//! FAI du central.
//!
//!     cargo run --release -p v34-net --example ata -- [s] [coupure tous les s] [coupure ms] [ppm modem] [ppm serveur]

use std::collections::{BTreeMap, VecDeque};

use v34_core::{EchoPath, LineParams, LineSim, Modem, ModemCommand, ModemConfig, ModemEvent, Role};
use v34_net::g711::{alaw_to_linear, f32_to_i16, i16_to_f32, linear_to_alaw};
use v34_net::{JitterBuffer, Pacer, FRAME_SAMPLES};

const B: usize = FRAME_SAMPLES;

/// Interpolateur sinc fenêtré (Blackman, 32 points, tabulé) : la prise
/// analogique entre l'ATA et le modem, qui n'ont pas la même horloge.
struct Resampler {
    fifo: VecDeque<f32>,
    pos: f64,
    step: f64,
    taps: Vec<f64>,
}

const HALF: usize = 16;
const OVER: usize = 512;

impl Resampler {
    fn new(ppm: f64) -> Self {
        let n = 2 * HALF * OVER + 1;
        let taps = (0..n)
            .map(|i| {
                let x = (i as f64 - (HALF * OVER) as f64) / OVER as f64;
                let sinc = if x.abs() < 1e-12 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
                let r = (x / HALF as f64 + 1.0) / 2.0;
                sinc * (0.42 - 0.5 * (2.0 * std::f64::consts::PI * r).cos() + 0.08 * (4.0 * std::f64::consts::PI * r).cos())
            })
            .collect();
        Self { fifo: VecDeque::from(vec![0.0; 4 * HALF]), pos: HALF as f64, step: 1.0 / (1.0 + ppm * 1e-6), taps }
    }

    fn process(&mut self, input: &[f32], out: &mut [f32]) {
        self.fifo.extend(input.iter().copied());
        for o in out.iter_mut() {
            let i0 = self.pos.floor() as isize;
            let frac = self.pos - i0 as f64;
            let mut acc = 0.0f64;
            for k in -(HALF as isize) + 1..=HALF as isize {
                let idx = i0 + k;
                if idx < 0 || idx as usize >= self.fifo.len() {
                    continue;
                }
                let t = ((k as f64 - frac + HALF as f64) * OVER as f64).round() as usize;
                acc += self.fifo[idx as usize] as f64 * self.taps[t.min(self.taps.len() - 1)];
            }
            *o = acc as f32;
            self.pos += self.step;
        }
        let drop = (self.pos.floor() as usize).saturating_sub(2 * HALF);
        self.fifo.drain(..drop.min(self.fifo.len()));
        self.pos -= drop as f64;
    }
}

/// Un sens d'Internet : délai, gigue (sans déséquencement) et coupures
/// pendant lesquelles tout est retenu, puis livré d'un coup.
struct Net {
    queue: VecDeque<(f64, u32, Vec<u8>)>,
    base_ms: f64,
    jitter_ms: f64,
    rng: u64,
    last: f64,
}

impl Net {
    fn new(base_ms: f64, jitter_ms: f64, seed: u64) -> Self {
        Self { queue: VecDeque::new(), base_ms, jitter_ms, rng: seed, last: 0.0 }
    }
    fn uniform(&mut self) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 11) as f64 / (1u64 << 53) as f64
    }
    fn send(&mut self, now_ms: f64, seq: u32, payload: Vec<u8>, outage: Option<(f64, f64)>) {
        let mut t = now_ms + self.base_ms + self.uniform() * self.jitter_ms;
        if let Some((a, b)) = outage {
            if now_ms >= a && now_ms < b {
                t = t.max(b + self.base_ms);
            }
        }
        t = t.max(self.last + 0.05);
        self.last = t;
        self.queue.push_back((t, seq, payload));
    }
    fn recv(&mut self, now_ms: f64) -> Vec<(u32, Vec<u8>)> {
        let mut out = Vec::new();
        while self.queue.front().is_some_and(|p| p.0 <= now_ms) {
            let (_, s, p) = self.queue.pop_front().unwrap();
            out.push((s, p));
        }
        out
    }
}

fn encode(x: &[f32]) -> Vec<u8> {
    x.iter().map(|&v| linear_to_alaw(f32_to_i16(v))).collect()
}
fn decode(b: &[u8]) -> Vec<f32> {
    b.iter().map(|&a| i16_to_f32(alaw_to_linear(a))).collect()
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let arg = |i: usize, d: f64| a.get(i).map_or(d, |s| s.parse().unwrap());
    let seconds = arg(1, 300.0);
    let every = arg(2, 60.0);
    let outage_ms = arg(3, 500.0);
    let ppm_modem = arg(4, 54.0);
    let ppm_server = arg(5, 30.0);
    let verbose = std::env::var("TRACE").is_ok();

    // le « vrai modem » appelle le FAI
    let mut usr = Modem::new(Role::Call, ModemConfig::default());
    let mut fai = Modem::new(Role::Answer, ModemConfig::default());
    usr.command(ModemCommand::Dial);
    fai.command(ModemCommand::Answer);

    // prise de l'ATA : boucle analogique courte, écho de l'hybride
    let lp = LineParams { delay_ms: 1.0, snr_db: 40.0, ..LineParams::perfect() };
    let (mut loop_down, mut loop_up) = (LineSim::with_seed(lp.clone(), 1), LineSim::with_seed(lp, 2));
    let hybrid_db: f32 = std::env::var("HYBRID_DB").ok().and_then(|v| v.parse().ok()).unwrap_or(-20.0);
    let mut hybrid = EchoPath::new(LineParams { echo_db: hybrid_db, echo_delay_ms: 1.0, ..LineParams::perfect() });
    let mut usr_near = EchoPath::new(LineParams { echo_db: -15.0, echo_delay_ms: 2.0, ..LineParams::perfect() });
    let (mut to_usr, mut from_usr) = (Resampler::new(ppm_modem), Resampler::new(-ppm_modem));

    let (mut up, mut down) = (Net::new(30.0, 8.0, 7), Net::new(30.0, 8.0, 11));
    // ATA : tampon fixe (≈ 100 ms), lecture au rythme de son horloge, trou
    // pour un paquet absent, paquet en retard jeté
    let ata_depth = 5i64;
    let mut ata_buf: BTreeMap<i64, Vec<f32>> = BTreeMap::new();
    let mut ata_play: Option<i64> = None;
    let (mut ata_seq, mut ata_holes, mut ata_late) = (0u32, 0u64, 0u64);

    // notre côté : comme services.rs (FAI du central)
    let mut jb = JitterBuffer::new(140, v34_net::Concealment::Modem);
    let mut pacer = Pacer::anchored(1.0);
    let mut our_seq = 0u32;
    let mut server_clock = 0.0f64;
    let mut usr_clock = 0.0f64;
    let mut usr_done = 0usize;

    let mut usr_rx = Vec::new();
    let mut usr_tx = Vec::new();
    let mut fai_tx = vec![0.0f32; B];
    let mut connected = [false, false];
    let mut dead = [false, false];
    let mut got = [0usize; 2];
    let mut win = [0usize; 2];
    let mut next_byte = [0u8; 2];
    let ticks = (seconds * 50.0) as usize;
    for tick in 0..ticks {
        let now = tick as f64 * 20.0; // horloge de l'ATA
        let s = now / 1000.0;
        let outage = if every > 0.0 {
            let k = ((now / 1000.0) / every).floor();
            (k >= 1.0).then(|| (k * every * 1000.0, k * every * 1000.0 + outage_ms))
        } else {
            None
        };

        // --- côté ATA (son horloge = le temps de la simulation)
        for (seq, p) in down.recv(now) {
            let key = seq as i64;
            if ata_play.is_none() {
                ata_play = Some(key - ata_depth);
            }
            if key < ata_play.unwrap() {
                ata_late += 1;
            } else {
                ata_buf.insert(key, decode(&p));
            }
        }
        let played = match ata_play {
            Some(k) => {
                ata_play = Some(k + 1);
                ata_buf.remove(&k).unwrap_or_else(|| {
                    if k >= 0 {
                        ata_holes += 1;
                    }
                    vec![0.0; B]
                })
            }
            None => vec![0.0; B],
        };
        // le modem, à son quartz, traite 160·(1+ppm) échantillons pendant ce temps
        usr_clock += B as f64 * (1.0 + ppm_modem * 1e-6);
        let n = usr_clock.floor() as usize - usr_done;
        usr_done += n;
        let mut on_line = vec![0.0f32; B];
        loop_down.process(&played, &mut on_line);
        usr_rx.resize(n, 0.0);
        usr_tx.resize(n, 0.0);
        to_usr.process(&on_line, &mut usr_rx);
        usr_near.process(&usr_tx, &mut usr_rx);
        usr.process(&usr_rx, &mut usr_tx);
        let mut back = vec![0.0f32; B];
        from_usr.process(&usr_tx, &mut back);
        let mut up_frame = vec![0.0f32; B];
        loop_up.process(&back, &mut up_frame);
        hybrid.process(&played, &mut up_frame);
        up.send(now, ata_seq, encode(&up_frame), outage);
        ata_seq += 1;

        // --- côté serveur (son horloge bat ppm_server plus vite)
        for (seq, p) in up.recv(now) {
            jb.push(seq, seq.wrapping_mul(B as u32), decode(&p), s);
        }
        server_clock += 1.0 + ppm_server * 1e-6;
        while server_clock >= 1.0 {
            server_clock -= 1.0;
            let blocks = 1.0 + pacer.update(jb.level());
            for _ in 0..blocks as usize {
                let (rx, _) = jb.pop_frame();
                fai.process(&rx, &mut fai_tx);
                down.send(now, our_seq, encode(&fai_tx), outage);
                our_seq += 1;
            }
        }

        for (i, m) in [&mut usr, &mut fai].into_iter().enumerate() {
            let who = if i == 0 { "USR " } else { "FAI " };
            for ev in m.drain_events() {
                match ev {
                    ModemEvent::Connected { tx_rate, rx_rate } => {
                        connected[i] = true;
                        println!("[{s:7.2} s] {who} CONNECT {tx_rate}/{rx_rate}");
                    }
                    ModemEvent::Data(d) => {
                        got[i] += d.len();
                        win[i] += d.len();
                    }
                    ModemEvent::Log { msg, .. } => {
                        let l = msg.to_lowercase();
                        if verbose || ["renégoc", "réentra", "trop d'erreurs", "recal", "frmr", "perte", "porteuse"].iter().any(|k| l.contains(k)) {
                            println!("[{s:7.2} s] {who} {msg}");
                        }
                    }
                    ModemEvent::Disconnected { reason } => {
                        dead[i] = true;
                        println!("[{s:7.2} s] {who} NO CARRIER {reason}");
                    }
                    _ => {}
                }
            }
        }
        if tick % 50 == 0 {
            for (i, m) in [&mut usr, &mut fai].into_iter().enumerate() {
                if connected[i] && !dead[i] {
                    let chunk: Vec<u8> = (0..1200).map(|_| {
                        next_byte[i] = next_byte[i].wrapping_add(1);
                        next_byte[i]
                    }).collect();
                    m.command(ModemCommand::Send(chunk));
                }
            }
        }
        if outage.is_some_and(|(a, _)| (now - a).abs() < 1.0) {
            println!("[{s:7.2} s] ---- coupure réseau de {outage_ms} ms dans les deux sens");
        }
        if tick % 500 == 499 {
            let st = jb.stats();
            println!(
                "[{s:7.2} s] bilan 10 s : reçus usr {} / fai {} o ; tampon {} ms, recalages {}, retards {}, jetées {} ; ATA trous {ata_holes}, retards {ata_late} ; cadence +{} −{}",
                win[0], win[1], st.depth_ms, st.resyncs, st.late, st.dropped, pacer.extra, pacer.skipped
            );
            win = [0, 0];
        }
        if dead[0] && dead[1] {
            break;
        }
    }
    println!("total reçu : usr {} o, fai {} o", got[0], got[1]);
}
