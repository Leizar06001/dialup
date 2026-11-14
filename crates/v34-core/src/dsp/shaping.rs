//! Modulation / démodulation en bande passante avec mise en forme
//! « racine de cosinus surélevé » (RRC), pour des rythmes symbole
//! quelconques (rationnels) par rapport aux 8 kHz d'échantillonnage.
//!
//! Émission : s(t) = Re{ Σ_k a_k · p(t − kT) · e^{j2πfc·t} }
//! Réception : mélange par e^{−j2πfc·t}, filtre adapté RRC, puis
//! interpolation aux instants voulus.

use std::collections::VecDeque;
use std::f64::consts::{PI, TAU};

use super::Cpx;
use crate::api::SAMPLE_RATE;

/// Réponse impulsionnelle RRC, t en périodes symbole.
pub fn rrc(t: f64, alpha: f64) -> f64 {
    let eps = 1e-9;
    if t.abs() < eps {
        return 1.0 - alpha + 4.0 * alpha / PI;
    }
    if alpha > 0.0 && ((4.0 * alpha * t).abs() - 1.0).abs() < 1e-6 {
        let a = alpha / 2f64.sqrt();
        return a * ((1.0 + 2.0 / PI) * (PI / (4.0 * alpha)).sin() + (1.0 - 2.0 / PI) * (PI / (4.0 * alpha)).cos());
    }
    let num = (PI * t * (1.0 - alpha)).sin() + 4.0 * alpha * t * (PI * t * (1.0 + alpha)).cos();
    let den = PI * t * (1.0 - (4.0 * alpha * t).powi(2));
    num / den
}

/// Table de la RRC sur [−span, span] symboles, suréchantillonnée.
#[derive(Clone, Debug)]
pub struct PulseTable {
    table: Vec<f32>,
    over: usize,
    span: usize,
}

impl PulseTable {
    pub fn new(alpha: f64, span: usize) -> Self {
        let over = 256;
        let n = 2 * span * over + 1;
        let mut table: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f64 / over as f64 - span as f64;
                // Fenêtre de Tukey : plate sur la moitié centrale (la RRC y
                // reste exacte, donc le couple émission/réception reste de
                // Nyquist), adoucie seulement en bout de queue.
                let flat = 0.5 * span as f64;
                let w = if t.abs() <= flat { 1.0 } else { 0.5 + 0.5 * (PI * (t.abs() - flat) / (span as f64 - flat + 0.5)).cos() };
                (rrc(t, alpha) * w) as f32
            })
            .collect();
        // Normalisation : énergie unitaire par symbole (Σ p(kT/over)² / over = 1)
        let e: f64 = table.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / over as f64;
        let k = (1.0 / e.sqrt()) as f32;
        table.iter_mut().for_each(|v| *v *= k);
        Self { table, over, span }
    }

    /// p(t), t en périodes symbole (interpolation linéaire).
    #[inline]
    pub fn eval(&self, t: f64) -> f32 {
        let x = (t + self.span as f64) * self.over as f64;
        if x < 0.0 {
            return 0.0;
        }
        let i = x as usize;
        if i + 1 >= self.table.len() {
            return 0.0;
        }
        let f = (x - i as f64) as f32;
        self.table[i] * (1.0 - f) + self.table[i + 1] * f
    }

    pub fn span(&self) -> usize {
        self.span
    }
}

/// Modulateur QAM/PSK générique.
///
/// Usage : tant que `needs_symbol()`, appeler `push_symbol()`, puis
/// `next_sample()`. Le retard de groupe est de `span` symboles.
#[derive(Clone, Debug)]
pub struct PassbandMod {
    pulse: PulseTable,
    /// Échantillons par symbole
    sps: f64,
    carrier_step: f64,
    carrier_phase: f64,
    /// Symboles en mémoire avec leur indice
    syms: VecDeque<(i64, Cpx)>,
    next_index: i64,
    /// Temps courant en périodes symbole
    t_sym: f64,
    n: u64,
    pub gain: f32,
}

impl PassbandMod {
    pub fn new(symbol_rate: f64, carrier_hz: f64, alpha: f64, span: usize) -> Self {
        let fs = SAMPLE_RATE as f64;
        Self {
            pulse: PulseTable::new(alpha, span),
            sps: fs / symbol_rate,
            carrier_step: TAU * carrier_hz / fs,
            carrier_phase: 0.0,
            syms: VecDeque::new(),
            next_index: 0,
            t_sym: 0.0,
            n: 0,
            gain: 1.0,
        }
    }

    /// Un symbole de plus est nécessaire pour calculer l'échantillon suivant.
    #[inline]
    pub fn needs_symbol(&self) -> bool {
        // On calcule s(t − span·T) : symboles jusqu'à t + span inclus
        (self.next_index as f64) <= self.t_sym + self.pulse.span() as f64
    }

    #[inline]
    pub fn push_symbol(&mut self, a: Cpx) {
        self.syms.push_back((self.next_index, a));
        self.next_index += 1;
    }

    /// Échantillon suivant. Le signal est retardé de `span` symboles.
    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        let span = self.pulse.span() as f64;
        let t = self.t_sym - span; // instant effectivement émis
        while let Some(&(k, _)) = self.syms.front() {
            if (k as f64) < t - span - 1.0 {
                self.syms.pop_front();
            } else {
                break;
            }
        }
        let mut acc = Cpx::ZERO;
        for &(k, a) in &self.syms {
            let d = t - k as f64;
            if d < -span {
                break;
            }
            acc += a * self.pulse.eval(d);
        }
        let (s, c) = self.carrier_phase.sin_cos();
        self.carrier_phase = (self.carrier_phase + self.carrier_step) % TAU;
        self.n += 1;
        self.t_sym = self.n as f64 / self.sps;
        // Re{acc · e^{jθ}} ; facteur pour que la puissance = |a|²/2·… normalisée
        self.gain * (acc.re * c as f32 - acc.im * s as f32) / (self.sps as f32).sqrt()
    }

    /// Instant (en échantillons depuis la création) où le centre du symbole
    /// d'indice `k` apparaît en sortie.
    pub fn symbol_center_sample(&self, k: i64) -> f64 {
        (k as f64 + self.pulse.span() as f64) * self.sps
    }

    pub fn samples(&self) -> u64 {
        self.n
    }

    pub fn symbols_pushed(&self) -> i64 {
        self.next_index
    }

    /// Nombre de symboles encore « en vol » (poussés mais pas encore sortis).
    pub fn pending_symbols(&self) -> f64 {
        (self.next_index as f64 - (self.t_sym - self.pulse.span() as f64)).max(0.0)
    }
}

/// Convertisseur vers la bande de base : mélange par e^{−j2πfc·t}, puis
/// filtre adapté RRC évalué EXACTEMENT à n'importe quel instant
/// fractionnaire (polyphase via la table de l'impulsion) — pas
/// d'interpolation approximative, indispensable à 2,33 échantillons/symbole.
///
/// Base de temps : indice d'échantillon d'entrée (0 = premier `push`).
#[derive(Clone, Debug)]
pub struct Downconverter {
    carrier_step: f64,
    carrier_phase: f64,
    pulse: PulseTable,
    /// Bande de base mélangée (non filtrée)
    bb: VecDeque<Cpx>,
    /// Indice de bb[0]
    bb_start: u64,
    n: u64,
    half: i64,
    sps: f64,
    keep: usize,
}

impl Downconverter {
    pub fn new(symbol_rate: f64, carrier_hz: f64, alpha: f64, span: usize) -> Self {
        let fs = SAMPLE_RATE as f64;
        let sps = fs / symbol_rate;
        let half = (span as f64 * sps).ceil() as i64;
        let keep = (2 * half) as usize + 1024;
        Self {
            carrier_step: TAU * carrier_hz / fs,
            carrier_phase: 0.0,
            pulse: PulseTable::new(alpha, span),
            bb: VecDeque::with_capacity(keep + 1),
            bb_start: 0,
            n: 0,
            half,
            sps,
            keep,
        }
    }

    /// Retard minimal (échantillons) entre l'arrivée d'un instant et la
    /// possibilité de l'évaluer avec `at()`.
    pub fn delay(&self) -> f64 {
        self.half as f64
    }

    pub fn samples_per_symbol(&self) -> f64 {
        self.sps
    }

    pub fn set_carrier(&mut self, carrier_hz: f64) {
        self.carrier_step = TAU * carrier_hz / SAMPLE_RATE as f64;
    }

    /// Nombre d'échantillons traités.
    pub fn samples(&self) -> u64 {
        self.n
    }

    /// Ajoute un échantillon. Renvoie la sortie filtrée centrée sur
    /// l'instant `samples() − 1 − delay()` (ou zéro au démarrage).
    #[inline]
    pub fn push(&mut self, x: f32) -> Cpx {
        let (s, c) = self.carrier_phase.sin_cos();
        self.carrier_phase = (self.carrier_phase + self.carrier_step) % TAU;
        self.bb.push_back(Cpx::new(2.0 * x * c as f32, -2.0 * x * s as f32));
        if self.bb.len() > self.keep {
            self.bb.pop_front();
            self.bb_start += 1;
        }
        self.n += 1;
        let t = self.n as f64 - 1.0 - self.half as f64;
        self.at(t).unwrap_or(Cpx::ZERO)
    }

    /// Sortie du filtre adapté centrée sur l'instant `t` (fractionnaire).
    /// `None` si les échantillons nécessaires ne sont pas (ou plus) là.
    pub fn at(&self, t: f64) -> Option<Cpx> {
        let lo = (t - self.half as f64).ceil() as i64;
        let hi = (t + self.half as f64).floor() as i64;
        if lo < self.bb_start as i64 || hi >= self.n as i64 || t < 0.0 {
            return None;
        }
        let inv = 1.0 / self.sps;
        let mut acc = Cpx::ZERO;
        for m in lo..=hi {
            let v = self.bb[(m - self.bb_start as i64) as usize];
            acc += v * self.pulse.eval((t - m as f64) * inv);
        }
        // Normalisation : un symbole a émis par PassbandMod ressort ≈ a
        Some(acc * (1.0 / (self.sps as f32).sqrt()))
    }

    /// Dernier instant évaluable par `at()`.
    pub fn latest(&self) -> f64 {
        self.n as f64 - 1.0 - self.half as f64
    }
}

/// Estimateur de rythme symbole « en boucle ouverte » (Oerder & Meyr) :
/// la puissance |y|² du signal filtré oscille au rythme symbole ; la phase
/// de cette composante donne l'instant optimal d'échantillonnage.
#[derive(Clone, Debug)]
pub struct TimingEstimator {
    step: f64,
    acc: Cpx,
    decay: f32,
    n: u64,
}

impl TimingEstimator {
    /// `memory_symbols` : constante de temps du lissage, en symboles.
    pub fn new(symbol_rate: f64, memory_symbols: f64) -> Self {
        let sps = SAMPLE_RATE as f64 / symbol_rate;
        Self { step: TAU / sps, acc: Cpx::ZERO, decay: (1.0 - 1.0 / (memory_symbols * sps)) as f32, n: 0 }
    }

    /// `t` : instant (en échantillons) auquel correspond `y`.
    #[inline]
    pub fn push(&mut self, t: f64, y: Cpx) {
        let p = y.norm_sqr();
        let ph = self.step * t;
        let (s, c) = ph.sin_cos();
        self.acc = self.acc * self.decay + Cpx::new(p * c as f32, -p * s as f32);
        self.n += 1;
    }

    /// Oublie l'historique (début d'une nouvelle salve).
    pub fn reset(&mut self) {
        self.acc = Cpx::ZERO;
    }

    /// Change la mémoire (en symboles) : courte pour l'acquisition.
    pub fn set_memory(&mut self, symbol_rate: f64, memory_symbols: f64) {
        let sps = SAMPLE_RATE as f64 / symbol_rate;
        self.decay = (1.0 - 1.0 / (memory_symbols * sps)) as f32;
    }

    /// Phase de rythme estimée : fraction de période (0..1) à laquelle se
    /// trouvent les centres des symboles (relative à l'instant 0), et
    /// la « qualité » (amplitude relative de la composante).
    pub fn phase(&self) -> (f64, f32) {
        let a = self.acc.arg() as f64; // = −2π·τ/T
        let mut frac = -a / TAU;
        frac -= frac.floor();
        (frac, self.acc.abs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rrc_is_nyquist_after_matched_filter() {
        // p * p (convolution) doit s'annuler aux multiples non nuls de T
        let p = PulseTable::new(0.5, 8);
        let over = 64;
        let conv = |k: f64| -> f64 {
            let mut s = 0.0;
            let n = 16 * over;
            for i in -(n as i64)..=(n as i64) {
                let t = i as f64 / over as f64;
                s += p.eval(t) as f64 * p.eval(k - t) as f64;
            }
            s / over as f64
        };
        let c0 = conv(0.0);
        assert!((c0 - 1.0).abs() < 0.02, "énergie {c0}");
        for k in 1..4 {
            assert!(conv(k as f64).abs() < 0.02, "ISI à {k}T");
        }
    }

    #[test]
    fn mod_demod_recovers_symbols() {
        for &(rate, fc, alpha) in &[(600.0, 1200.0, 0.5), (3200.0, 1920.0, 0.15), (3429.0, 1959.0, 0.1), (2400.0, 1800.0, 0.15)] {
            let rate: f64 = rate;
            let span = if alpha < 0.3 { 16 } else { 8 };
            let mut m = PassbandMod::new(rate, fc, alpha, span);
            let mut d = Downconverter::new(rate, fc, alpha, span);
            let pts = [Cpx::new(1.0, 1.0), Cpx::new(-1.0, 1.0), Cpx::new(-1.0, -1.0), Cpx::new(1.0, -1.0)];
            let mut sent = Vec::new();
            let mut lfsr = 0x1234u32;
            for _ in 0..(8000) {
                while m.needs_symbol() {
                    lfsr = lfsr.wrapping_mul(1103515245).wrapping_add(12345);
                    let a = pts[((lfsr >> 16) & 3) as usize];
                    sent.push(a);
                    m.push_symbol(a);
                }
                d.push(m.next_sample());
            }
            // Symbole k : centre à l'échantillon (k+span)·sps + retard filtre.
            // L'historique ne garde que 256 échantillons : on vérifie la fin.
            let mut max_err = 0.0f32;
            let last = d.latest();
            let mut checked = 0;
            for k in (0..sent.len() as i64).rev() {
                let t = m.symbol_center_sample(k);
                if t > last {
                    continue;
                }
                match d.at(t) {
                    Some(y) => {
                        let e = (y - sent[k as usize]).abs();
                        max_err = max_err.max(e);
                        checked += 1;
                        if checked > 40 {
                            break;
                        }
                    }
                    None => break,
                }
            }
            assert!(checked > 30, "pas assez de symboles vérifiés ({checked})");
            assert!(max_err < 0.1, "débit {rate} : erreur max {max_err}");
        }
    }

    #[test]
    fn timing_estimator_finds_phase() {
        let rate = 600.0;
        let mut m = PassbandMod::new(rate, 1200.0, 0.5, 8);
        let mut d = Downconverter::new(rate, 1200.0, 0.5, 8);
        let mut te = TimingEstimator::new(rate, 16.0);
        let mut sign = 1.0f32;
        for _ in 0..8000 {
            while m.needs_symbol() {
                sign = -sign; // alternance : DPSK « 1111 »
                m.push_symbol(Cpx::new(sign, 0.0));
            }
            let y = d.push(m.next_sample());
            te.push(d.latest(), y);
        }
        let sps = 8000.0 / rate;
        let expect = (m.symbol_center_sample(0) / sps).fract();
        let (got, _) = te.phase();
        let mut err = (got - expect).abs();
        if err > 0.5 {
            err = 1.0 - err;
        }
        assert!(err < 0.05, "phase {got} attendue {expect}");
    }
}
