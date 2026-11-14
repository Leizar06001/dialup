//! Récepteur PCM du modem analogique (l'abonné) : de Sd jusqu'aux données.
//!
//! - Sd puis S̄d : signal de période 6, puis inversé (§8.4.4) ; un
//!   détecteur de corrélation à 6 symboles repère la bascule ;
//! - TRN1d (signes connus) : calage exact par corrélation, puis égaliseur
//!   à retour de décision calculé par moindres carrés ; mesure du bruit ;
//! - Jd, J′d : bits portés par le signe de UINFO (différentiel, brouillés) ;
//! - DIL : symboles connus (notre descripteur) : niveaux et bruit mesurés ;
//! - Ri, R̄i : même détecteur ; TRN2d (connu) : nouveau calage, nouvel
//!   égaliseur, avec le jeu de la phase 4 choisi par nous (CPt) ;
//! - MP (à travers le décodeur PCM), Ed (deux trames de zéros), B1d puis
//!   données avec le jeu du mode données (CP).

use std::collections::VecDeque;

use super::codes::{self, Law, UCODES};
use super::dfe::Dfe;
use super::frame::{Decoder, Encoder, Mapping, Symbol, FRAME};
use super::pcmtx::{TRN1D_LEN, TRN2D_LEN};
use super::seq::{DilDescriptor, Jd, Mp90};
use crate::train::Scrambler;

/// Symboles d'apprentissage sûrs (longueur minimale de TRN1d / TRN2d).
const TRAIN_MIN: usize = 2040;
/// Égaliseur : FFE, retour, position du symbole dans la fenêtre.
const NF: usize = 32;
const NB: usize = 64;
const CURSOR: usize = 10;

#[derive(Clone, Debug, PartialEq)]
pub enum RxEvent {
    /// Signal reconnu (affichage)
    Signal(&'static str),
    /// Bascule Sd → S̄d : l'abonné arrête Ja
    SdBar,
    /// Égaliseur appris sur TRN1d ; bruit (linéaire 16 bits)
    Trained { sigma: f64 },
    Jd(Jd),
    JdPrime,
    /// Un passage complet du DIL mesuré
    DilDone { sigma: f64, gain: f64 },
    /// Bascule Ri → R̄i : l'abonné passe de CPt à CP
    RiBar,
    /// Égaliseur réappris sur TRN2d
    Retrained { sigma: f64 },
    Mp(Mp90),
    /// Ed reçu : B1d puis données suivent
    Ed,
    /// B1d terminé : mode données
    DataStarted,
    /// Échec (calage impossible…)
    Lost(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum St {
    WaitSd,
    /// Accumulation de TRN1d avant l'apprentissage
    Trn1dCollect,
    /// Égaliseur en marche : TRN1d, Jd, J′d
    Bits,
    Dil,
    WaitRi,
    Trn2dCollect,
    /// Égaliseur en marche : TRN2d, MP, Ed, B1d, données
    Frames,
    Idle,
}

/// Détecteur de signal de période 6 (Sd, Ri) et de son inversion (S̄d, R̄i).
///
/// Pendant le signal périodique, on mémorise sa période (gabarit de 6
/// échantillons) ; la bascule se voit ensuite comme une corrélation
/// négative durable avec ce gabarit (S̄d et R̄i sont eux aussi périodiques :
/// seule la comparaison au gabarit révèle l'inversion).
#[derive(Clone, Debug, Default)]
struct Period6 {
    hist: VecDeque<f32>,
    t: u64,
    c6: f64,
    p: f64,
    /// Échantillons consécutifs franchement périodiques
    run: u32,
    tmpl: [f64; 6],
    /// Corrélation avec le gabarit (lissée)
    cq: f64,
    armed: bool,
    since: u32,
}

impl Period6 {
    /// Renvoie vrai à la bascule (période 6 établie puis inversée).
    fn push(&mut self, x: f32) -> bool {
        let x = x as f64;
        let ph = (self.t % 6) as usize;
        self.t += 1;
        self.hist.push_back(x as f32);
        if self.hist.len() <= 6 {
            return false;
        }
        let old = self.hist.pop_front().unwrap() as f64;
        let a = 1.0 / 24.0;
        self.c6 += a * (x * old - self.c6);
        self.p += a * (x * x - self.p);
        if self.p < 1e-9 {
            self.run = 0;
            self.armed = false;
            return false;
        }
        let r = self.c6 / self.p;
        if r > 0.8 && !(self.armed && self.cq < 0.0) {
            self.run += 1;
            self.tmpl[ph] += 0.1 * (x - self.tmpl[ph]);
            if self.run >= 96 {
                self.armed = true;
                self.since = 0;
            }
        } else {
            self.run = 0;
        }
        if !self.armed {
            return false;
        }
        let tp = self.tmpl.iter().map(|v| v * v).sum::<f64>() / 6.0;
        if tp < 1e-9 {
            return false;
        }
        self.cq += 0.125 * (x * self.tmpl[ph] / (self.p * tp).sqrt() - self.cq);
        if self.cq < -0.5 {
            self.armed = false;
            self.run = 0;
            self.cq = 0.0;
            return true;
        }
        // signal qui n'est plus périodique sans s'être inversé : on désarme
        if r < 0.5 {
            self.since += 1;
            if self.since > 64 {
                self.armed = false;
                self.cq = 0.0;
            }
        } else {
            self.since = 0;
        }
        false
    }
}

/// Séquence de signes de TRN1d : uns brouillés par GPC remis à zéro.
fn trn1d_signs(n: usize) -> Vec<bool> {
    let mut s = Scrambler::new(true);
    (0..n).map(|_| s.scramble(true)).collect()
}

/// Symboles de TRN2d : uns à travers le codeur (brouilleur à zéro).
fn trn2d_symbols(map: &Mapping, n: usize) -> Vec<Symbol> {
    let mut enc = Encoder::new(map.clone());
    let d = map.bits_per_frame() as usize;
    let mut v = Vec::with_capacity(n + FRAME);
    while v.len() < n {
        v.extend(enc.frame(&vec![true; d]));
    }
    v.truncate(n);
    v
}

/// Décision : niveau signé le plus proche dans un ensemble de Ucodes.
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

fn value(s: &Symbol, law: Law) -> f64 {
    s.linear(law) as f64 / codes::SCALE as f64
}

pub struct PcmRx {
    law: Law,
    uinfo: u8,
    st: St,
    /// Échantillons reçus depuis la création
    t: u64,
    p6: Period6,
    /// Derniers échantillons (pour amorcer un tampon d'apprentissage)
    recent: VecDeque<f32>,
    /// Tampon d'apprentissage et instant de son premier échantillon
    buf: Vec<f32>,
    buf_t0: u64,
    /// Instant approché de la bascule (S̄d ou R̄i)
    t_flip: u64,
    dfe: Dfe,
    /// Instant (échantillon) où arrive le symbole 0 (TRN1d ou TRN2d),
    /// moins 4 (marge de précurseur)
    t_sym0: u64,
    /// Indice du prochain symbole à sortir de l'égaliseur
    k: i64,
    events: Vec<RxEvent>,
    // --- TRN1d / Jd / J′d
    trn1d: Vec<bool>,
    descr: Scrambler,
    last_sign: bool,
    bits: VecDeque<bool>,
    jd_seen: bool,
    // --- DIL
    dil_k0: i64,
    dil_sched: Vec<(u8, bool)>,
    dil_sum: [f64; UCODES],
    dil_n: [u32; UCODES],
    dil_e2: f64,
    dil_cnt: u64,
    pub sigma: f64,
    // --- phase 4 et données
    train_map: Option<Mapping>,
    data_map: Option<Mapping>,
    trn2d: Vec<Symbol>,
    dec: Option<Decoder>,
    frame: Vec<Symbol>,
    mp_frame: Option<i64>,
    ed_frames: u32,
    b1d_left: u32,
    in_data: bool,
    /// Bits de données reçus (pour la couche liaison)
    pub data: Vec<bool>,
    /// Erreur quadratique moyenne de décision en données (linéaire 16 bits)
    pub mse: f64,
}

impl PcmRx {
    pub fn new(law: Law, uinfo: u8, dil: Option<DilDescriptor>) -> PcmRx {
        let dil_sched = dil.as_ref().map_or(Vec::new(), |d| {
            let mut v = Vec::with_capacity(d.total_len());
            for n in 0..d.train.len() {
                for i in 0..d.segment_len(n) {
                    v.push(d.symbol(n, i));
                }
            }
            v
        });
        PcmRx {
            law,
            uinfo,
            st: St::WaitSd,
            t: 0,
            p6: Period6::default(),
            recent: VecDeque::new(),
            buf: Vec::new(),
            buf_t0: 0,
            t_flip: 0,
            dfe: Dfe::new(NF, NB, CURSOR),
            t_sym0: 0,
            k: 0,
            events: Vec::new(),
            trn1d: trn1d_signs(TRN1D_LEN),
            descr: Scrambler::new(true),
            last_sign: true,
            bits: VecDeque::new(),
            jd_seen: false,
            dil_k0: 0,
            dil_sched,
            dil_sum: [0.0; UCODES],
            dil_n: [0; UCODES],
            dil_e2: 0.0,
            dil_cnt: 0,
            sigma: 0.0,
            train_map: None,
            data_map: None,
            trn2d: Vec::new(),
            dec: None,
            frame: Vec::with_capacity(FRAME),
            mp_frame: None,
            ed_frames: 0,
            b1d_left: 0,
            in_data: false,
            data: Vec::new(),
            mse: 0.0,
        }
    }

    pub fn take_events(&mut self) -> Vec<RxEvent> {
        std::mem::take(&mut self.events)
    }

    /// Jeu de la phase 4 (notre CPt) : à fixer avant la fin de Ri.
    pub fn set_train_map(&mut self, m: Mapping) {
        self.trn2d = trn2d_symbols(&m, TRN2D_LEN);
        self.train_map = Some(m);
    }

    /// Jeu du mode données (notre CP).
    pub fn set_data_map(&mut self, m: Mapping) {
        self.data_map = Some(m);
    }

    pub fn in_data(&self) -> bool {
        self.in_data
    }

    /// Débit descendant en cours (bit/s), 0 avant les données.
    pub fn rate(&self) -> u32 {
        if self.in_data {
            self.data_map.as_ref().map_or(0, |m| m.rate())
        } else {
            0
        }
    }

    pub fn push(&mut self, x: f32) {
        let t = self.t;
        self.t += 1;
        self.recent.push_back(x);
        if self.recent.len() > 256 {
            self.recent.pop_front();
        }
        match self.st {
            St::WaitSd | St::WaitRi => {
                if self.st == St::WaitRi {
                    // l'égaliseur suit encore le DIL (symboles connus)
                    self.run_dfe(x);
                }
                if self.p6.push(x) {
                    self.t_flip = t;
                    self.buf = self.recent.iter().copied().collect();
                    self.buf_t0 = t + 1 - self.buf.len() as u64;
                    if self.st == St::WaitSd {
                        self.events.push(RxEvent::Signal("S̄d"));
                        self.events.push(RxEvent::SdBar);
                        self.st = St::Trn1dCollect;
                    } else {
                        self.events.push(RxEvent::Signal("R̄i"));
                        self.events.push(RxEvent::RiBar);
                        self.st = St::Trn2dCollect;
                    }
                }
            }
            St::Trn1dCollect | St::Trn2dCollect => {
                self.buf.push(x);
                // bascule + 48T (S̄d) ou 24T (R̄i) + apprentissage + marge
                let need = self.t_flip + 48 + TRAIN_MIN as u64 + 200;
                if t >= need {
                    if self.st == St::Trn1dCollect {
                        self.learn_trn1d();
                    } else {
                        self.learn_trn2d();
                    }
                }
            }
            St::Bits | St::Dil | St::Frames => self.run_dfe(x),
            St::Idle => {}
        }
    }

    /// Calage : instant du symbole 0 d'une séquence connue (valeurs `v`)
    /// près de l'instant attendu, par corrélation.
    fn align(&self, v: &[f64], expect: u64) -> Option<u64> {
        let lo = expect.saturating_sub(96).max(self.buf_t0);
        let hi = expect + 160;
        let mut best = (0u64, 0.0f64);
        for l in lo..hi {
            let o = (l - self.buf_t0) as usize;
            if o + 1024 > self.buf.len() {
                break;
            }
            let c: f64 = (0..1024).map(|k| v[k] * self.buf[o + k] as f64).sum::<f64>();
            if c > best.1 {
                best = (l, c);
            }
        }
        (best.1 > 0.0).then_some(best.0)
    }

    fn learn_trn1d(&mut self) {
        let a = codes::linear(self.law, self.uinfo) as f64 / codes::SCALE as f64;
        let known: Vec<f64> = self.trn1d.iter().map(|&s| if s { a } else { -a }).collect();
        let Some(l) = self.align(&known, self.t_flip + 48) else {
            self.lost("calage sur TRN1d impossible");
            return;
        };
        self.t_sym0 = l.saturating_sub(4);
        let delay = (self.t_sym0 - self.buf_t0) as usize;
        self.dfe = Dfe::new(NF, NB, CURSOR);
        let Some(mse) = self.dfe.train(&self.buf, &known, delay, 100, TRAIN_MIN - 40) else {
            self.lost("égaliseur : apprentissage impossible");
            return;
        };
        self.sigma = mse.sqrt() * codes::SCALE as f64;
        self.events.push(RxEvent::Trained { sigma: self.sigma });
        self.events.push(RxEvent::Signal("TRN1d"));
        self.st = St::Bits;
        self.replay();
    }

    fn learn_trn2d(&mut self) {
        let Some(map) = self.train_map.clone() else {
            self.lost("TRN2d reçu sans jeu de phase 4");
            return;
        };
        let known: Vec<f64> = self.trn2d.iter().map(|s| value(s, self.law)).collect();
        let Some(l) = self.align(&known, self.t_flip + 24) else {
            self.lost("calage sur TRN2d impossible");
            return;
        };
        self.t_sym0 = l.saturating_sub(4);
        let delay = (self.t_sym0 - self.buf_t0) as usize;
        self.dfe = Dfe::new(NF, NB, CURSOR);
        let Some(mse) = self.dfe.train(&self.buf, &known, delay, 100, TRAIN_MIN - 40) else {
            self.lost("égaliseur : nouvel apprentissage impossible");
            return;
        };
        self.sigma = mse.sqrt() * codes::SCALE as f64;
        self.events.push(RxEvent::Retrained { sigma: self.sigma });
        self.events.push(RxEvent::Signal("TRN2d"));
        self.dec = Some(Decoder::new(map));
        self.frame.clear();
        self.mp_frame = None;
        self.ed_frames = 0;
        self.st = St::Frames;
        self.replay();
    }

    /// Repasse le tampon dans l'égaliseur en continu, depuis le début.
    fn replay(&mut self) {
        let buf = std::mem::take(&mut self.buf);
        let t_end = self.t;
        self.dfe.clear();
        self.k = 0;
        self.t = self.buf_t0;
        for &x in &buf {
            self.t += 1;
            self.run_dfe(x);
        }
        debug_assert_eq!(self.t, t_end);
    }

    fn lost(&mut self, why: &str) {
        self.events.push(RxEvent::Lost(why.into()));
        self.st = St::Idle;
    }

    /// Un échantillon dans l'égaliseur ; traite le symbole qui en sort.
    /// `self.t` vaut déjà l'instant de l'échantillon + 1.
    fn run_dfe(&mut self, x: f32) {
        self.dfe.push(x);
        let t = self.t - 1;
        let first = self.t_sym0 + CURSOR as u64;
        if t < first {
            return;
        }
        let k = (t - first) as i64;
        if k < self.k {
            return;
        }
        self.k = k + 1;
        let y = self.dfe.output();
        match self.st {
            St::Bits => self.on_bits_symbol(k, y),
            St::Dil | St::WaitRi => self.on_dil_symbol(k, y),
            St::Frames => self.on_frame_symbol(k, y),
            _ => {}
        }
    }

    fn on_bits_symbol(&mut self, k: i64, y: f64) {
        let a = codes::linear(self.law, self.uinfo) as f64 / codes::SCALE as f64;
        let ku = k as usize;
        let sign = if ku < TRAIN_MIN { self.trn1d[ku] } else { y >= 0.0 };
        self.dfe.decided(y, if sign { a } else { -a });
        // bits : signe différentiel puis désembrouillage (Jd, J′d)
        let b = self.descr.descramble(sign ^ self.last_sign);
        self.last_sign = sign;
        if ku < TRAIN_MIN {
            return;
        }
        self.bits.push_back(b);
        if self.bits.len() > 200 {
            self.bits.pop_front();
        }
        let n = self.bits.len();
        if n >= Jd::LEN {
            let w: Vec<bool> = self.bits.range(n - Jd::LEN..).copied().collect();
            if let Some(jd) = Jd::from_bits(&w) {
                if !self.jd_seen {
                    self.jd_seen = true;
                    self.events.push(RxEvent::Signal("Jd"));
                    self.events.push(RxEvent::Jd(jd));
                }
            }
        }
        // J′d : douze zéros juste après un Jd complet
        if self.jd_seen && n >= Jd::LEN + 12 && self.bits.range(n - 12..).all(|&b| !b) {
            let w: Vec<bool> = self.bits.range(n - 12 - Jd::LEN..n - 12).copied().collect();
            if Jd::from_bits(&w).is_some() {
                self.events.push(RxEvent::Signal("J′d"));
                self.events.push(RxEvent::JdPrime);
                self.dil_k0 = k + 1;
                self.st = if self.dil_sched.is_empty() { St::WaitRi } else { St::Dil };
                if self.st == St::Dil {
                    self.events.push(RxEvent::Signal("DIL"));
                }
            }
        }
    }

    fn on_dil_symbol(&mut self, k: i64, y: f64) {
        if self.dil_sched.is_empty() {
            self.dfe.decided(y, 0.0);
            return;
        }
        let j = (k - self.dil_k0).max(0) as usize;
        let (u, pos) = self.dil_sched[j % self.dil_sched.len()];
        let s = Symbol { ucode: u, positive: pos };
        let v = value(&s, self.law);
        self.dfe.decided(y, v);
        if self.st != St::Dil {
            return;
        }
        let sgn = if pos { 1.0 } else { -1.0 };
        self.dil_sum[u as usize] += y * sgn;
        self.dil_n[u as usize] += 1;
        self.dil_e2 += (y - v) * (y - v);
        self.dil_cnt += 1;
        if j + 1 >= self.dil_sched.len() {
            // un passage complet : gain moyen mesuré / nominal, et bruit
            let mut g = 0.0;
            let mut w = 0.0;
            for u in 0..UCODES {
                if self.dil_n[u] > 0 {
                    let nominal = codes::linear(self.law, u as u8) as f64 / codes::SCALE as f64;
                    let meas = self.dil_sum[u] / self.dil_n[u] as f64;
                    g += meas * nominal;
                    w += nominal * nominal;
                }
            }
            let gain = if w > 0.0 { g / w } else { 1.0 };
            let sigma = (self.dil_e2 / self.dil_cnt.max(1) as f64).sqrt() * codes::SCALE as f64;
            self.sigma = self.sigma.max(sigma);
            self.events.push(RxEvent::DilDone { sigma, gain });
            self.st = St::WaitRi;
            self.p6 = Period6::default();
        }
    }

    fn on_frame_symbol(&mut self, k: i64, y: f64) {
        let ku = k as usize;
        let i = ku % FRAME;
        let map = if self.ed_frames >= 2 { self.data_map.clone() } else { self.train_map.clone() };
        let Some(map) = map else { return };
        let s = if ku < TRAIN_MIN { self.trn2d[ku] } else { slice(y, &map.sets[i], self.law) };
        let v = value(&s, self.law);
        if ku >= TRAIN_MIN {
            let e = (y - v) * codes::SCALE as f64;
            self.mse = 0.995 * self.mse + 0.005 * e * e;
            self.dfe.mu = 0.0005;
        }
        self.dfe.decided(y, v);
        self.frame.push(s);
        if self.frame.len() < FRAME {
            return;
        }
        let f: [Symbol; FRAME] = std::array::from_fn(|j| self.frame[j]);
        self.frame.clear();
        let fi = k / FRAME as i64;
        let Some(dec) = self.dec.as_mut() else { return };
        let bits = dec.frame(&f);
        if self.in_data {
            self.data.extend(bits);
            return;
        }
        if self.b1d_left > 0 {
            self.b1d_left -= 1;
            if self.b1d_left == 0 {
                self.in_data = true;
                self.events.push(RxEvent::DataStarted);
            }
            return;
        }
        if self.ed_frames == 1 {
            // seconde trame de Ed, puis B1d avec le jeu des données
            self.ed_frames = 2;
            match self.data_map.clone() {
                Some(m) => {
                    self.dec = Some(Decoder::new(m));
                    self.b1d_left = 48;
                    self.events.push(RxEvent::Signal("B1d"));
                }
                None => self.lost("Ed reçu sans jeu de données"),
            }
            return;
        }
        // Ed : une trame entièrement nulle juste après une MP
        if self.mp_frame == Some(fi - 1) && bits.iter().all(|&b| !b) {
            self.ed_frames = 1;
            self.events.push(RxEvent::Signal("Ed"));
            self.events.push(RxEvent::Ed);
            return;
        }
        for b in bits {
            self.bits.push_back(b);
            if self.bits.len() > 200 {
                self.bits.pop_front();
            }
            let n = self.bits.len();
            // MP + bit de remplissage 85
            if n >= Mp90::LEN {
                let w: Vec<bool> = self.bits.range(n - Mp90::LEN..).copied().collect();
                if let Some(mp) = Mp90::from_bits(&w) {
                    self.mp_frame = Some(fi);
                    self.events.push(RxEvent::Signal(if mp.ack { "MP′" } else { "MP" }));
                    self.events.push(RxEvent::Mp(mp));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v90::pcmtx::PcmTx;

    #[test]
    fn bascule_sd_reperee() {
        let mut tx = PcmTx::new(Law::A, 96);
        let mut p6 = Period6::default();
        let mut hit = None;
        for n in 0..1400 {
            if n == 100 {
                tx.start_phase3(&Jd::upto(56000), None);
            }
            let x = tx.next_sample(&mut || true);
            let flip = p6.push(x);
            if std::env::var("P6_DEBUG").is_ok() && (480..560).contains(&n) {
                eprintln!("{n} x={x:+.3} r={:+.2} cq={:+.2} armed={}", p6.c6 / p6.p, p6.cq, p6.armed);
            }
            if flip && hit.is_none() {
                hit = Some(n);
            }
        }
        // S̄d commence à 100 + 384 = 484
        let n = hit.expect("bascule vue");
        assert!((484..484 + 40).contains(&n), "bascule à {n}");
    }
}
