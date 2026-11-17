//! Sondage de ligne L1/L2 (V.34 §10.1.2.4) et choix des paramètres.
//!
//! L1/L2 = somme de cosinus espacés de 150 Hz (150…3750 Hz, sans 900,
//! 1200, 1800, 2400 Hz), phases de la Table 17. Le signal est périodique
//! de période 160 échantillons (20 ms = 3 périodes de 150 Hz) : une DFT
//! sur 160 échantillons sépare parfaitement les tons (raies multiples de
//! 3·50 Hz) du bruit (les autres raies).

use std::f64::consts::{PI, TAU};

use crate::api::NOMINAL_RMS;
use crate::info::ProbeResult;

/// (fréquence Hz, phase en degrés) — Table 17/V.34.
pub const PROBE_TONES: [(u32, u32); 21] = [
    (150, 0), (300, 180), (450, 0), (600, 0), (750, 0), (1050, 0), (1350, 0),
    (1500, 0), (1650, 180), (1950, 0), (2100, 0), (2250, 180), (2550, 0), (2700, 180),
    (2850, 0), (3000, 180), (3150, 180), (3300, 180), (3450, 180), (3600, 0), (3750, 0),
];

/// Vitesses de symbole exactes (index 0..5) : S = (a/c)·2400.
pub const SYMBOL_RATES: [f64; 6] = [2400.0, 2400.0 * 8.0 / 7.0, 2400.0 * 7.0 / 6.0, 3000.0, 3200.0, 2400.0 * 10.0 / 7.0];
/// Valeurs arrondies pour l'affichage.
pub const SYMBOL_RATES_NOMINAL: [u32; 6] = [2400, 2743, 2800, 3000, 3200, 3429];
/// Rapports porteuse/S (d/e) : (basse, haute) — Table 2/V.34.
pub const CARRIER_RATIOS: [((u32, u32), (u32, u32)); 6] = [
    ((2, 3), (3, 4)),
    ((3, 5), (2, 3)),
    ((3, 5), (2, 3)),
    ((3, 5), (2, 3)),
    ((4, 7), (3, 5)),
    ((4, 7), (4, 7)),
];
/// Débit max par vitesse de symbole (Table 8), bit/s.
pub const MAX_RATE_FOR_SR: [u32; 6] = [21600, 26400, 26400, 28800, 31200, 33600];

/// Débit min par vitesse de symbole (Table 8) : 2400 bit/s seulement à
/// 2400 bauds, 4800 au-delà.
pub const fn min_rate_for_sr(sr_idx: usize) -> u32 {
    if sr_idx == 0 { 2400 } else { 4800 }
}

pub fn carrier_hz(sr_idx: usize, high: bool) -> f64 {
    let (lo, hi) = CARRIER_RATIOS[sr_idx];
    let (d, e) = if high { hi } else { lo };
    SYMBOL_RATES[sr_idx] * d as f64 / e as f64
}

/// Générateur L1 / L2.
#[derive(Clone, Debug)]
pub struct ProbeGen {
    n: u64,
    amp: f32,
}

impl ProbeGen {
    /// `boost_db` : +6 pour L1, 0 pour L2.
    pub fn new(boost_db: f32) -> Self {
        // Puissance totale = 21·a²/2 = NOMINAL² (+ boost)
        let amp = NOMINAL_RMS * (2.0f32 / 21.0).sqrt() * 10f32.powf(boost_db / 20.0);
        Self { n: 0, amp }
    }

    pub fn set_boost(&mut self, boost_db: f32) {
        self.amp = NOMINAL_RMS * (2.0f32 / 21.0).sqrt() * 10f32.powf(boost_db / 20.0);
    }

    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        // Le signal est périodique sur 160 échantillons : on calcule la phase
        // modulo 160 pour rester exact indéfiniment.
        let k = (self.n % 160) as f64;
        let mut v = 0.0f64;
        for &(f, ph) in PROBE_TONES.iter() {
            v += (TAU * f as f64 * k / 8000.0 + ph as f64 * PI / 180.0).cos();
        }
        self.n += 1;
        self.amp * v as f32
    }
}

/// Analyseur des signaux de sondage reçus.
#[derive(Clone, Debug)]
pub struct ProbeAnalyzer {
    block: Vec<f32>,
    /// Puissance accumulée par raie (k = 0..80, pas de 50 Hz)
    power: Vec<f64>,
    /// Spectres de puissance de chaque bloc (pour écarter les blocs
    /// pollués par un clic ou une coupure)
    per_block: Vec<Vec<f64>>,
    blocks: u32,
    /// Phaseur de la raie 1050 Hz au bloc précédent (décalage de fréquence)
    prev_1050: Option<(f64, f64)>,
    phase_drift: Vec<f64>,
    /// Gain cos/sin précalculé (160 × 80)
    twiddle: Vec<(f32, f32)>,
}

/// Résultat complet de l'analyse.
#[derive(Clone, Debug, Default)]
pub struct ProbeReport {
    /// (fréquence, gain dB relatif à l'émission, SNR dB dans 50 Hz) par ton
    pub tones: Vec<(f32, f32, f32)>,
    /// Décalage de fréquence mesuré (Hz)
    pub freq_offset_hz: Option<f32>,
    /// Résultats par vitesse de symbole (pour INFO1c / décision)
    pub results: [ProbeResult; 6],
    /// SNR équivalent (dB) retenu par vitesse de symbole
    pub snr_eq_db: [f32; 6],
}

impl Default for ProbeAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl ProbeAnalyzer {
    pub fn new() -> Self {
        let mut twiddle = Vec::with_capacity(160 * 81);
        for k in 0..=80 {
            for n in 0..160 {
                let w = TAU * k as f64 * n as f64 / 160.0;
                twiddle.push((w.cos() as f32, w.sin() as f32));
            }
        }
        Self { block: Vec::with_capacity(160), power: vec![0.0; 81], per_block: Vec::new(), blocks: 0, prev_1050: None, phase_drift: Vec::new(), twiddle }
    }

    pub fn blocks(&self) -> u32 {
        self.blocks
    }

    pub fn push(&mut self, x: f32) {
        self.block.push(x);
        if self.block.len() == 160 {
            let mut bp = vec![0.0f64; 81];
            for k in 1..=80usize {
                let (mut re, mut im) = (0.0f32, 0.0f32);
                let tw = &self.twiddle[k * 160..(k + 1) * 160];
                for (v, &(c, s)) in self.block.iter().zip(tw) {
                    re += v * c;
                    im -= v * s;
                }
                let (re, im) = (re as f64 / 160.0, im as f64 / 160.0);
                self.power[k] += re * re + im * im;
                bp[k] = re * re + im * im;
                if k == 21 {
                    if let Some((pr, pi)) = self.prev_1050 {
                        // arg(z · conj(prev))
                        let dr = re * pr + im * pi;
                        let di = im * pr - re * pi;
                        self.phase_drift.push(di.atan2(dr));
                    }
                    self.prev_1050 = Some((re, im));
                }
            }
            self.per_block.push(bp);
            self.blocks += 1;
            self.block.clear();
        }
    }

    /// Calcule les résultats. `max_rate` : débit max autorisé localement.
    /// `gap_db` : écart à la capacité de Shannon toléré par notre récepteur.
    pub fn report(&self, max_rate: u32, gap_db: f32) -> ProbeReport {
        // Énergie « hors tons » de chaque bloc : un clic ou une coupure la
        // fait exploser (ou chuter) ; on écarte les blocs aberrants.
        let is_tone_bin = |k: usize| k % 3 == 0 && PROBE_TONES.iter().any(|&(f, _)| f as usize == k * 50);
        let noise_e: Vec<f64> = self.per_block.iter().map(|b| (1..=80).filter(|&k| !is_tone_bin(k)).map(|k| b[k]).sum()).collect();
        let tone_e: Vec<f64> = self.per_block.iter().map(|b| (1..=80).filter(|&k| is_tone_bin(k)).map(|k| b[k]).sum()).collect();
        let median = |v: &[f64]| -> f64 {
            let mut s = v.to_vec();
            s.sort_by(|a, b| a.partial_cmp(b).unwrap());
            s.get(s.len() / 2).copied().unwrap_or(0.0)
        };
        let (mn, mt) = (median(&noise_e), median(&tone_e));
        let keep: Vec<usize> = (0..self.per_block.len()).filter(|&i| noise_e[i] <= 4.0 * mn + 1e-15 && tone_e[i] >= 0.5 * mt).collect();
        let p: Vec<f64> = if keep.is_empty() {
            let nb = self.blocks.max(1) as f64;
            self.power.iter().map(|v| v / nb).collect()
        } else {
            (0..81).map(|k| keep.iter().map(|&i| self.per_block[i][k]).sum::<f64>() / keep.len() as f64).collect()
        };
        let tx_tone = {
            let a = NOMINAL_RMS as f64 * (2.0f64 / 21.0).sqrt();
            a * a / 4.0 // |DFT|² d'un cosinus d'amplitude a = (a/2)²
        };
        let is_tone = |k: usize| k % 3 == 0 && PROBE_TONES.iter().any(|&(f, _)| f as usize == k * 50);
        // Bruit par raie : moyenne des raies non-tons voisines
        let noise_at = |k: usize| -> f64 {
            let mut s: f64 = 0.0;
            let mut c: f64 = 0.0;
            for d in 1..=4usize {
                for kk in [k.saturating_sub(d), k + d] {
                    if kk >= 1 && kk <= 80 && !is_tone(kk) {
                        s += p[kk];
                        c += 1.0;
                    }
                }
            }
            (s / c.max(1.0)).max(1e-14)
        };
        let mut tones = Vec::new();
        let mut snr_lin = Vec::new(); // (f, SNR densité normalisée à P/S·S → on garde S_tone/N)
        for &(f, _) in PROBE_TONES.iter() {
            let k = (f / 50) as usize;
            let n = noise_at(k);
            let s = (p[k] - n).max(1e-14);
            let gain_db = 10.0 * (s / tx_tone).log10();
            let snr = s / n;
            tones.push((f as f32, gain_db as f32, (10.0 * snr.log10()) as f32));
            snr_lin.push((f as f64, snr));
        }
        // SNR d'un signal de données au niveau nominal, en densité :
        // SNR(f) = S_tone(f)·21·50 / (S·N(f))  (voir doc du module)
        let snr_at = |f: f64, sym: f64| -> f64 {
            if f < 100.0 || f > 3800.0 {
                return 1e-3;
            }
            // interpolation linéaire en dB entre tons
            let mut lo = snr_lin[0];
            let mut hi = *snr_lin.last().unwrap();
            for w in snr_lin.windows(2) {
                if f >= w[0].0 && f <= w[1].0 {
                    lo = w[0];
                    hi = w[1];
                    break;
                }
            }
            let (l0, l1) = (10.0 * lo.1.log10(), 10.0 * hi.1.log10());
            let t = if hi.0 > lo.0 { ((f - lo.0) / (hi.0 - lo.0)).clamp(0.0, 1.0) } else { 0.0 };
            let db = l0 + t * (l1 - l0);
            let db = db.min(60.0); // plafond réaliste (quantification, etc.)
            10f64.powf(db / 10.0) * 21.0 * 50.0 / sym
        };
        let gap = 10f64.powf(gap_db as f64 / 10.0);
        let mut results = [ProbeResult::default(); 6];
        let mut snr_eq_db = [0.0f32; 6];
        for i in 0..6 {
            let sym = SYMBOL_RATES[i];
            let mut best = (0u8, false, 0.0f64);
            for high in [false, true] {
                let fc = carrier_hz(i, high);
                // Égaliseur linéaire MMSE : moyenne harmonique de (1+SNR)
                let steps = 64;
                let mut inv = 0.0;
                for s in 0..steps {
                    let f = fc - sym / 2.0 + sym * (s as f64 + 0.5) / steps as f64;
                    inv += 1.0 / (1.0 + snr_at(f, sym));
                }
                let snr_le = (steps as f64 / inv - 1.0).max(1e-6);
                let bits = (1.0 + snr_le / gap).log2() - 0.5;
                let rate = (sym * bits).max(0.0) as u32;
                let rate = rate.min(MAX_RATE_FOR_SR[i]).min(max_rate);
                let n = (rate / 2400) as u8;
                if n > best.0 || (n == best.0 && snr_le > best.2) {
                    best = (n, high, snr_le);
                }
            }
            results[i] = ProbeResult { high_carrier: best.1, preemph: 0, max_rate: best.0 };
            snr_eq_db[i] = (10.0 * best.2.log10()) as f32;
        }
        let freq_offset_hz = if self.phase_drift.len() >= 3 {
            let mut d = self.phase_drift.clone();
            d.sort_by(|a, b| a.partial_cmp(b).unwrap());
            Some((d[d.len() / 2] / (TAU * 0.02)) as f32)
        } else {
            None
        };
        ProbeReport { tones, freq_offset_hz, results, snr_eq_db }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_level_is_nominal() {
        let mut g = ProbeGen::new(0.0);
        let mut e = 0.0f64;
        for _ in 0..1600 {
            let x = g.next_sample() as f64;
            e += x * x;
        }
        let rms = (e / 1600.0).sqrt() as f32;
        assert!((rms - NOMINAL_RMS).abs() < 0.01, "rms {rms}");
    }

    #[test]
    fn perfect_line_gives_max_rates() {
        let mut g = ProbeGen::new(0.0);
        let mut a = ProbeAnalyzer::new();
        for _ in 0..(160 * 20) {
            a.push(g.next_sample());
        }
        let r = a.report(33600, 9.0);
        assert_eq!(r.results[5].max_rate, 14, "{:?}", r.results);
        for (_, g, _) in &r.tones {
            assert!(g.abs() < 0.5, "gain {g}");
        }
    }

    #[test]
    fn noisy_line_lowers_rates() {
        let mut g = ProbeGen::new(0.0);
        let mut a = ProbeAnalyzer::new();
        let mut seed = 1u32;
        for _ in 0..(160 * 20) {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let noise = (seed as f32 / u32::MAX as f32 - 0.5) * 0.06; // ≈ SNR 23 dB
            a.push(g.next_sample() + noise);
        }
        let r = a.report(33600, 9.0);
        let best = r.results.iter().map(|x| x.max_rate).max().unwrap();
        assert!(best < 12 && best > 3, "{:?} snr {:?}", r.results, r.snr_eq_db);
    }
}
