//! Banc d'essai du 56k « à vide » (jalon 1) : codeur PCM du FAI → ligne
//! simulée → récepteur PCM de l'abonné, sans poignée de main.
//!
//! Pendant tout l'essai, l'abonné émet du V.34 (son écho revient dans ce
//! qu'il reçoit). Déroulé, comme dans la vraie poignée de main :
//! 1. l'abonné émet seul : apprentissage de l'annuleur d'écho ;
//! 2. le FAI émet une séquence connue à 2 niveaux (comme TRN1d) : calage,
//!    apprentissage de l'égaliseur, mesure du bruit ;
//! 3. l'abonné choisit les niveaux (comme CP) ;
//! 4. données : on compte les erreurs.
//!
//! `cargo test -p v34-core --test v90_pompe -- --nocapture` affiche le
//! débit obtenu sur chaque préréglage.

use v34_core::dsp::cpx::Cpx;
use v34_core::line::{EchoPath, LineParams, LineSim};
use v34_core::train::{quarter_point, QamTx};
use v34_core::v90::codes::{self, Law};
use v34_core::v90::design::{self, Channel, Limits};
use v34_core::v90::dfe::{Dfe, EchoCanceller};
use v34_core::v90::frame::{Decoder, Encoder, Symbol, FRAME};

const LAW: Law = Law::A;
/// Ucode de la séquence d'apprentissage à 2 niveaux (UINFO)
const UINFO: u8 = 96;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn bit(&mut self) -> bool {
        self.next() & 1 == 1
    }
}

/// Émission montante de l'abonné : V.34 à 3200 bauds, 4 points.
struct Upstream {
    tx: QamTx,
    rng: Rng,
}
impl Upstream {
    fn new() -> Upstream {
        Upstream { tx: QamTx::new(3200.0, 1829.0), rng: Rng(99) }
    }
    fn sample(&mut self) -> f32 {
        if self.tx.queued() < 8 {
            let pts: Vec<Cpx> = (0..32).map(|_| quarter_point((self.rng.next() % 4) as usize)).collect();
            self.tx.push(&pts);
        }
        self.tx.next_sample()
    }
}

/// La ligne vue par l'abonné : son descendant à travers la boucle locale,
/// plus l'écho de sa propre émission.
struct Line {
    down: LineSim,
    echo: EchoPath,
    up: Upstream,
}
impl Line {
    fn new(p: &LineParams) -> Line {
        Line { down: LineSim::with_seed(p.clone(), 42), echo: EchoPath::new(p.clone()), up: Upstream::new() }
    }
    /// Fait passer `down` (échantillons du FAI) ; renvoie (émis par
    /// l'abonné, reçu par l'abonné).
    fn run(&mut self, down: &[f32], upstream_on: bool) -> (Vec<f32>, Vec<f32>) {
        let mut rx = vec![0f32; down.len()];
        self.down.process(down, &mut rx);
        let tx: Vec<f32> = (0..down.len()).map(|_| if upstream_on { self.up.sample() } else { 0.0 }).collect();
        self.echo.process(&tx, &mut rx);
        (tx, rx)
    }
}

/// Décision : le niveau signé le plus proche dans l'ensemble de l'intervalle.
fn slice(y: f64, set: &[u8], law: Law) -> Symbol {
    let a = y.abs() * codes::SCALE as f64;
    let mut best = set[0];
    let mut dist = f64::MAX;
    for &u in set {
        let d = (codes::linear(law, u) as f64 - a).abs();
        if d < dist {
            dist = d;
            best = u;
        }
    }
    Symbol { ucode: best, positive: y >= 0.0 }
}

struct Result {
    rate: u32,
    sigma: f64,
    erle_db: f64,
    sym_err: f64,
    bit_err: f64,
}

fn essai(p: &LineParams, data_frames: usize, limits: &Limits) -> Option<Result> {
    let mut line = Line::new(p);
    let mut rng = Rng(0x5eed_1234);

    // 1. l'abonné émet seul (1 s) : annuleur d'écho
    let (tx, rx) = line.run(&vec![0.0; 8000], true);
    let mut ec = EchoCanceller::new(160);
    let resid = ec.train(&tx, &rx).unwrap_or(1.0);
    let erle_db = -10.0 * resid.max(1e-12).log10();
    // on rejoue la fin pour amorcer l'historique de l'annuleur
    for (&t, &r) in tx.iter().zip(&rx).skip(7900) {
        ec.process(t, r);
    }
    ec.mu = 0.0;

    // 2. séquence connue à 2 niveaux (±UINFO), 2 s
    let n_train = 16000;
    let train_sym: Vec<Symbol> = (0..n_train).map(|_| Symbol { ucode: UINFO, positive: rng.bit() }).collect();
    let train_tx: Vec<f32> = train_sym.iter().map(|s| codes::sample(LAW, s.ucode, s.positive)).collect();
    let (tx, rx) = line.run(&train_tx, true);
    let clean: Vec<f32> = tx.iter().zip(&rx).map(|(&t, &r)| ec.process(t, r)).collect();
    // calage : corrélation avec la séquence connue
    let known: Vec<f64> = train_tx.iter().map(|&x| x as f64).collect();
    let mut best = (0usize, 0.0f64);
    for lag in 0..700 {
        let c: f64 = (0..4000).map(|k| known[k] * clean[k + lag] as f64).sum::<f64>().abs();
        if c > best.1 {
            best = (lag, c);
        }
    }
    let delay = best.0;
    let mut dfe = Dfe::new(32, 64, 10);
    let mse = dfe.train(&clean, &known, delay.saturating_sub(4), 200, n_train - 200)?;
    let sigma = mse.sqrt() * codes::SCALE as f64;
    if std::env::var("V90_DEBUG").is_ok() {
        eprintln!("  σ {sigma:.1} (lin), annulation d'écho {erle_db:.1} dB, calage {delay}");
    }

    // 3. choix des niveaux
    let map = design::design(&Channel::nominal(LAW, sigma), limits)?;

    // 4. données
    let d = map.bits_per_frame() as usize;
    let mut enc = Encoder::new(map.clone());
    let mut dec = Decoder::new(map.clone());
    let mut sent_bits = Vec::with_capacity(data_frames * d);
    let mut data_sym = Vec::with_capacity(data_frames * FRAME);
    for _ in 0..data_frames {
        let b: Vec<bool> = (0..d).map(|_| rng.bit()).collect();
        data_sym.extend(enc.frame(&b));
        sent_bits.extend(b);
    }
    let data_tx: Vec<f32> = data_sym.iter().map(|s| codes::sample(LAW, s.ucode, s.positive)).collect();
    let (tx2, rx2) = line.run(&data_tx, true);
    // l'égaliseur tourne en continu sur toute la réception (apprentissage
    // puis données) ; décisions connues pendant l'apprentissage
    let all_sym: Vec<f64> = known.iter().copied().chain(data_tx.iter().map(|&x| x as f64)).collect();
    let all_rx: Vec<f32> = clean.iter().copied().chain(tx2.iter().zip(&rx2).map(|(&t, &r)| ec.process(t, r))).collect();
    let lag = delay.saturating_sub(4) + dfe.cursor;
    dfe.mu = 0.001;
    let mut decided: Vec<Symbol> = Vec::with_capacity(data_sym.len());
    let mut sym_err = 0usize;
    for (t, &r) in all_rx.iter().enumerate() {
        dfe.push(r);
        let Some(k) = t.checked_sub(lag) else { continue };
        if k >= all_sym.len() {
            break;
        }
        let y = dfe.output();
        if k < n_train {
            dfe.decided(y, all_sym[k]);
            continue;
        }
        let i = k - n_train;
        let s = slice(y, &map.sets[i % FRAME], LAW);
        if s != data_sym[i] {
            sym_err += 1;
        }
        dfe.decided(y, s.linear(LAW) as f64 / codes::SCALE as f64);
        decided.push(s);
    }
    let mut got = Vec::with_capacity(sent_bits.len());
    for f in decided.chunks_exact(FRAME) {
        got.extend(dec.frame(&[f[0], f[1], f[2], f[3], f[4], f[5]]));
    }
    let n = got.len().min(sent_bits.len());
    let bit_err = (0..n).filter(|&i| got[i] != sent_bits[i]).count();
    Some(Result {
        rate: map.rate(),
        sigma,
        erle_db,
        sym_err: sym_err as f64 / decided.len().max(1) as f64,
        bit_err: bit_err as f64 / n.max(1) as f64,
    })
}

fn preset(name: &str) -> LineParams {
    LineParams::preset(name).unwrap()
}

#[test]
fn tableau_des_debits() {
    let lim = Limits { max_rate: 56000, ..Limits::default() };
    println!("\n{:<16} {:>9} {:>9} {:>8} {:>10} {:>10}", "ligne", "débit", "σ (lin)", "écho dB", "err. symb", "err. bits");
    for (name, p) in LineParams::presets() {
        match essai(&p, 8000 / FRAME * 3, &lim) {
            Some(r) => println!("{:<16} {:>9} {:>9.1} {:>8.1} {:>10.2e} {:>10.2e}", name, r.rate, r.sigma, r.erle_db, r.sym_err, r.bit_err),
            None => println!("{name:<16} pas de V.90 (repli V.34)"),
        }
    }
}

#[test]
fn parfaite_a_56k_sans_erreur() {
    let r = essai(&preset("Parfaite"), 2000, &Limits::default()).expect("V.90 sur ligne parfaite");
    assert_eq!(r.rate, 56000);
    assert_eq!(r.bit_err, 0.0, "σ {}", r.sigma);
}

#[test]
fn urbaine_au_dessus_du_v34() {
    let r = essai(&preset("Urbaine"), 2000, &Limits::default()).expect("V.90 sur ligne urbaine");
    assert!(r.rate > 33600, "{} bit/s", r.rate);
    assert!(r.bit_err < 1e-4, "taux d'erreur {}", r.bit_err);
}

#[test]
fn debit_plafonne() {
    let lim = Limits { max_rate: 44000, ..Limits::default() };
    let r = essai(&preset("Parfaite"), 600, &lim).unwrap();
    assert_eq!(r.rate, 44000);
    assert_eq!(r.bit_err, 0.0);
}
