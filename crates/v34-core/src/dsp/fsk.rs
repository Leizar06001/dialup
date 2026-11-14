//! Modem FSK V.21 (300 bit/s), utilisé par V.8 pour CM / JM / CJ.
//!
//! Canal 1 (bas, appelant)  : 1 = 980 Hz,  0 = 1180 Hz
//! Canal 2 (haut, appelé)   : 1 = 1650 Hz, 0 = 1850 Hz

use super::filters::BoxcarCpx;
use super::osc::Osc;
use super::Cpx;
use crate::api::SAMPLE_RATE;

pub const V21_BAUD: f64 = 300.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum V21Channel {
    Low,
    High,
}

impl V21Channel {
    /// (fréquence du 1 « mark », fréquence du 0 « space »)
    pub fn freqs(self) -> (f64, f64) {
        match self {
            V21Channel::Low => (980.0, 1180.0),
            V21Channel::High => (1650.0, 1850.0),
        }
    }
}

/// Modulateur FSK à phase continue.
#[derive(Clone, Debug)]
pub struct FskMod {
    osc: Osc,
    mark: f64,
    space: f64,
    /// Position dans le bit courant, en fraction de bit
    frac: f64,
    step: f64,
    current: bool,
}

impl FskMod {
    pub fn new(ch: V21Channel) -> Self {
        let (mark, space) = ch.freqs();
        Self { osc: Osc::new(mark), mark, space, frac: 1.0, step: V21_BAUD / SAMPLE_RATE as f64, current: true }
    }

    /// Indique si un nouveau bit est nécessaire avant le prochain échantillon.
    pub fn needs_bit(&self) -> bool {
        self.frac >= 1.0
    }

    /// Fournit le bit suivant (à appeler quand `needs_bit()`).
    pub fn set_bit(&mut self, bit: bool) {
        self.frac -= 1.0;
        self.current = bit;
        self.osc.set_freq(if bit { self.mark } else { self.space });
    }

    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        self.frac += self.step;
        self.osc.next_cos()
    }
}

/// Démodulateur FSK : discriminateur à deux corrélateurs + récupération
/// du rythme bit sur les transitions.
#[derive(Clone, Debug)]
pub struct FskDemod {
    osc_m: Osc,
    osc_s: Osc,
    avg_m: BoxcarCpx,
    avg_s: BoxcarCpx,
    /// Phase du rythme bit (0..1), échantillonnage à 0.5
    phase: f64,
    step: f64,
    last_sign: bool,
    /// Niveau du signal (pour le squelch)
    level: f32,
}

impl FskDemod {
    pub fn new(ch: V21Channel) -> Self {
        let (mark, space) = ch.freqs();
        // Fenêtre ≈ 1 bit (26.67 échantillons)
        let w = (SAMPLE_RATE as f64 / V21_BAUD).round() as usize;
        Self {
            osc_m: Osc::new(mark),
            osc_s: Osc::new(space),
            avg_m: BoxcarCpx::new(w),
            avg_s: BoxcarCpx::new(w),
            phase: 0.0,
            step: V21_BAUD / SAMPLE_RATE as f64,
            last_sign: true,
            level: 0.0,
        }
    }

    /// Niveau de la porteuse FSK reçue (somme des puissances mark+space).
    pub fn level(&self) -> f32 {
        self.level
    }

    /// Traite un échantillon ; renvoie un bit quand il est échantillonné.
    #[inline]
    pub fn push(&mut self, x: f32) -> Option<bool> {
        let (cm, sm) = self.osc_m.next_iq();
        let (cs, ss) = self.osc_s.next_iq();
        let zm = self.avg_m.push(Cpx::new(x * cm, -x * sm));
        let zs = self.avg_s.push(Cpx::new(x * cs, -x * ss));
        let pm = zm.norm_sqr();
        let ps = zs.norm_sqr();
        self.level = pm + ps;
        let sign = pm > ps;

        // Récupération de rythme : une transition doit tomber en phase 0.
        if sign != self.last_sign {
            // Erreur de phase : ramener vers 0 (ou 1).
            let err = if self.phase < 0.5 { self.phase } else { self.phase - 1.0 };
            self.phase -= 0.35 * err;
            self.last_sign = sign;
        }
        let prev = self.phase;
        self.phase += self.step;
        let mut out = None;
        if prev < 0.5 && self.phase >= 0.5 {
            out = Some(sign);
        }
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_bits() {
        for ch in [V21Channel::Low, V21Channel::High] {
            let mut m = FskMod::new(ch);
            let mut d = FskDemod::new(ch);
            // Motif pseudo-aléatoire
            let mut lfsr: u32 = 0xACE1;
            let mut sent = Vec::new();
            let mut got = Vec::new();
            for _ in 0..(8000 * 2) {
                if m.needs_bit() {
                    let b = lfsr & 1 == 1;
                    lfsr = (lfsr >> 1) ^ (if b { 0xB400 } else { 0 });
                    sent.push(b);
                    m.set_bit(b);
                }
                let x = 0.3 * m.next_sample();
                if let Some(b) = d.push(x) {
                    got.push(b);
                }
            }
            // Aligne : cherche le décalage qui donne le moins d'erreurs
            let mut best = usize::MAX;
            for off in 0..4 {
                let errs = sent
                    .iter()
                    .skip(50)
                    .zip(got.iter().skip(50 - off))
                    .take(400)
                    .filter(|(a, b)| a != b)
                    .count();
                best = best.min(errs);
            }
            assert_eq!(best, 0, "canal {ch:?}");
        }
    }
}
