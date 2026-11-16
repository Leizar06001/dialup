//! Deux modems à des horloges différentes : un vrai modem derrière un
//! adaptateur téléphonique suit SON quartz, l'adaptateur le sien (+54 ppm
//! mesurés entre un U.S. Robotics et un Cisco ATA). Sans suivi du rythme,
//! la grille d'échantillonnage glisse d'un demi-symbole en 3 s et la
//! réception s'effondre en phase 4 (SNR 11 dB, comme avec le vrai modem).

use std::collections::VecDeque;

use v34_core::{LineParams, LineSim, Modem, ModemCommand, ModemConfig, ModemEvent, Role};

/// Rééchantillonneur (sinus cardinal, fenêtre de Kaiser β = 8, 64 points :
/// erreur d'interpolation sous −70 dB jusqu'à 3,7 kHz, pour que le banc
/// n'ajoute pas son propre bruit) : l'horloge de sortie bat `1 + ppm·10⁻⁶`
/// fois plus vite que celle d'entrée.
struct Resampler {
    fifo: VecDeque<f32>,
    pos: f64,
    step: f64,
}

const HALF: usize = 32;
const BETA: f64 = 8.0;

/// Fonction de Bessel modifiée I₀ (série)
fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term, mut k) = (1.0, 1.0, 1.0);
    while term > 1e-12 * sum {
        term *= (x / (2.0 * k)) * (x / (2.0 * k));
        sum += term;
        k += 1.0;
    }
    sum
}

impl Resampler {
    fn new(ppm: f64) -> Self {
        Self { fifo: VecDeque::from(vec![0.0; 4 * HALF]), pos: HALF as f64, step: 1.0 / (1.0 + ppm * 1e-6) }
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
                let x = k as f64 - frac;
                let sinc = if x.abs() < 1e-9 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
                let r = x / HALF as f64;
                let w = if r.abs() >= 1.0 { 0.0 } else { bessel_i0(BETA * (1.0 - r * r).sqrt()) / bessel_i0(BETA) };
                acc += self.fifo[idx as usize] as f64 * sinc * w;
            }
            *o = acc as f32;
            self.pos += self.step;
        }
        // on oublie ce qui ne servira plus
        let drop = (self.pos.floor() as usize).saturating_sub(2 * HALF);
        self.fifo.drain(..drop.min(self.fifo.len()));
        self.pos -= drop as f64;
    }
}

struct Run {
    connected: [Option<(u32, u32)>; 2],
    got: [Vec<u8>; 2],
    log: Vec<String>,
}

/// L'appelé (le « vrai modem ») bat `ppm` plus vite que l'appelant.
fn run(ppm: f64, seconds: f32, payload: [&[u8]; 2]) -> Run {
    let p = LineParams { delay_ms: 60.0, snr_db: 40.0, ..LineParams::perfect() };
    let mut call = Modem::new(Role::Call, ModemConfig::default());
    let mut ans = Modem::new(Role::Answer, ModemConfig::default());
    let (mut c2a, mut a2c) = (LineSim::new(p.clone()), LineSim::new(p));
    // ce que l'appelé reçoit est vu à son horloge, et réciproquement
    let (mut to_ans, mut to_call) = (Resampler::new(ppm), Resampler::new(-ppm));
    call.command(ModemCommand::Dial);
    ans.command(ModemCommand::Answer);
    const B: usize = 160;
    let (mut call_rx, mut ans_rx) = ([0.0f32; B], [0.0f32; B]);
    let (mut call_tx, mut ans_tx) = ([0.0f32; B], [0.0f32; B]);
    let (mut tmp1, mut tmp2) = ([0.0f32; B], [0.0f32; B]);
    let mut r = Run { connected: [None, None], got: [Vec::new(), Vec::new()], log: Vec::new() };
    let mut sent = [false, false];
    for blk in 0..(seconds * 8000.0 / B as f32) as usize {
        call.process(&call_rx, &mut call_tx);
        ans.process(&ans_rx, &mut ans_tx);
        c2a.process(&call_tx, &mut tmp1);
        to_ans.process(&tmp1, &mut ans_rx);
        a2c.process(&ans_tx, &mut tmp2);
        to_call.process(&tmp2, &mut call_rx);
        for (i, m) in [&mut call, &mut ans].into_iter().enumerate() {
            for ev in m.drain_events() {
                match ev {
                    ModemEvent::Connected { tx_rate, rx_rate } => r.connected[i] = Some((tx_rate, rx_rate)),
                    ModemEvent::Data(d) => r.got[i].extend(d),
                    ModemEvent::Log { msg, .. } => r.log.push(format!("[{:6} ms] {} {msg}", blk * 20, if i == 0 { "APPELANT" } else { "APPELÉ  " })),
                    ModemEvent::Disconnected { reason } => r.log.push(format!("[{:6} ms] {i} NO CARRIER {reason}", blk * 20)),
                    _ => {}
                }
            }
            if r.connected[i].is_some() && !sent[i] {
                sent[i] = true;
                m.command(ModemCommand::Send(payload[i].to_vec()));
            }
        }
        if r.got[0].len() >= payload[1].len() && r.got[1].len() >= payload[0].len() && sent[0] && sent[1] {
            break;
        }
    }
    r
}

fn check(ppm: f64) {
    // assez de données pour tenir ~15 s en mode données : la dérive continue
    let a: Vec<u8> = "Appelant : horloge du central. ".repeat(600).into_bytes();
    let b: Vec<u8> = "Appelé : horloge de son quartz. ".repeat(600).into_bytes();
    let r = run(ppm, 70.0, [&a, &b]);
    let dump = r.log.join("\n");
    let (Some(c), Some(d)) = (r.connected[0], r.connected[1]) else { panic!("{ppm:+} ppm : pas de CONNECT\n{dump}") };
    for l in r.log.iter().filter(|l| l.contains("SNR")) {
        eprintln!("{ppm:+} ppm {l}");
    }
    eprintln!("{ppm:+} ppm : CONNECT {c:?} / {d:?}");
    assert_eq!(r.got[1].len(), a.len(), "{ppm:+} ppm : appelant → appelé incomplet\n{dump}");
    assert_eq!(r.got[1], a, "{ppm:+} ppm : appelant → appelé\n{dump}");
    assert_eq!(r.got[0], b, "{ppm:+} ppm : appelé → appelant\n{dump}");
    assert!(c.1 >= 21600 && d.1 >= 21600, "{ppm:+} ppm : débits trop bas {c:?} / {d:?}\n{dump}");
}

#[test]
fn horloge_plus_rapide_de_54_ppm() {
    check(54.0);
}

#[test]
fn horloge_plus_lente_de_120_ppm() {
    check(-120.0);
}

#[test]
fn meme_horloge() {
    check(0.0);
}

/// Le pire admis par la norme : ±0,01 % sur le rythme de chaque modem.
#[test]
fn horloge_plus_rapide_de_200_ppm() {
    check(200.0);
}
