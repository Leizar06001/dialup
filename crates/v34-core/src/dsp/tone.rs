//! Détection de ton pur et de ses inversions de phase (180°).
//!
//! Principe : on mélange le signal reçu avec e^{-j2πft}, puis on moyenne
//! sur N échantillons. On obtient un phaseur complexe z(n) dont le module
//! donne le niveau du ton et l'argument sa phase. Une inversion de phase
//! fait passer z par zéro et changer de signe : on la date au milieu de
//! la fenêtre (précision ≈ 1 échantillon = 125 µs).

use super::filters::BoxcarCpx;
use super::osc::Osc;
use super::Cpx;

#[derive(Clone, Debug)]
pub struct ToneDetector {
    osc: Osc,
    avg: BoxcarCpx,
    z: Cpx,
    /// Phaseur de référence (avant inversion), lissé
    reference: Cpx,
    ref_valid: bool,
    /// Compteur d'échantillons traités
    n: u64,
    /// Instant (échantillon) de la dernière inversion détectée
    last_reversal: Option<u64>,
    reversal_pending: bool,
    /// Phaseurs passés (une fenêtre en arrière) pour juger la stabilité
    past: std::collections::VecDeque<Cpx>,
}

impl ToneDetector {
    /// `window` : longueur de moyenne en échantillons (ex. 40 = 5 ms).
    pub fn new(freq_hz: f64, window: usize) -> Self {
        Self {
            osc: Osc::new(freq_hz),
            avg: BoxcarCpx::new(window),
            z: Cpx::ZERO,
            reference: Cpx::ZERO,
            ref_valid: false,
            n: 0,
            last_reversal: None,
            reversal_pending: false,
            past: std::collections::VecDeque::with_capacity(window + 1),
        }
    }

    /// Le ton est-il « propre » : phaseur stable sur une fenêtre (un ton
    /// pur ne tourne pas ; une modulation DPSK ou du bruit, si).
    pub fn stable(&self) -> bool {
        match self.past.front() {
            Some(&old) if self.past.len() >= self.avg.len().min(80) => {
                let d = self.z - old;
                d.norm_sqr() < 0.3 * self.z.norm_sqr()
            }
            _ => false,
        }
    }

    /// Traite un échantillon. Renvoie `Some(t)` si une inversion de phase
    /// vient d'être détectée, `t` étant l'instant estimé de l'inversion
    /// (en numéro d'échantillon, même base que `samples()`).
    #[inline]
    pub fn push(&mut self, x: f32) -> Option<u64> {
        let (c, s) = self.osc.next_iq();
        let mixed = Cpx::new(x * c, -x * s);
        self.z = self.avg.push(mixed);
        self.n += 1;
        self.past.push_back(self.z);
        if self.past.len() > self.avg.len().min(80) {
            self.past.pop_front();
        }

        let p = self.z.norm_sqr();
        let mut out = None;
        if self.ref_valid {
            let rp = self.reference.norm_sqr();
            let proj = self.z.re * self.reference.re + self.z.im * self.reference.im;
            if proj < 0.0 && rp > 1e-10 {
                // Le phaseur est passé de l'autre côté : inversion.
                // Le passage par zéro a lieu au milieu de la fenêtre.
                let t = self.n.saturating_sub(self.avg.len() as u64 / 2);
                if !self.reversal_pending {
                    self.reversal_pending = true;
                }
                // On attend que le niveau soit revenu pour valider (anti-bruit).
                if p > 0.5 * rp {
                    self.last_reversal = Some(t);
                    self.reference = self.z;
                    self.reversal_pending = false;
                    out = Some(t.saturating_sub(self.pending_latency()));
                }
            } else {
                self.reversal_pending = false;
                // Suivi lent de la référence (tolère un léger décalage de fréquence)
                self.reference = self.reference * 0.98 + self.z * 0.02;
            }
        } else if self.n as usize > self.avg.len() && p > 0.0 {
            self.reference = self.z;
            self.ref_valid = true;
        }
        out
    }

    /// Correction : quand on valide l'inversion après retour du niveau,
    /// l'instant `t` calculé est trop tardif d'environ un quart de fenêtre.
    #[inline]
    fn pending_latency(&self) -> u64 {
        // Pour une fenêtre rectangulaire, |z|² atteint 0.5·|ref|² à 3/4 de
        // la fenêtre après l'inversion (z linéaire de +1 à -1 : |z|=√0.5 à
        // ≈0.85·N) ; la valeur calculée ci-dessus vaut n - N/2. Écart ≈ 0.35 N.
        (self.avg.len() as f32 * 0.35) as u64
    }

    /// Puissance du ton (≈ A²/4 pour un ton d'amplitude A).
    #[inline]
    pub fn power(&self) -> f32 {
        self.z.norm_sqr()
    }

    /// Amplitude estimée du ton.
    #[inline]
    pub fn amplitude(&self) -> f32 {
        2.0 * self.z.abs()
    }

    #[inline]
    pub fn phasor(&self) -> Cpx {
        self.z
    }

    pub fn samples(&self) -> u64 {
        self.n
    }

    pub fn last_reversal(&self) -> Option<u64> {
        self.last_reversal
    }

    /// Réinitialise la référence de phase (ex. au début d'une nouvelle écoute).
    pub fn rearm(&mut self) {
        self.ref_valid = false;
        self.reversal_pending = false;
    }
}

/// Détecteur de puissance totale (moyenne glissante de x²).
#[derive(Clone, Debug)]
pub struct PowerMeter {
    avg: super::filters::Boxcar,
    p: f32,
}

impl PowerMeter {
    pub fn new(window: usize) -> Self {
        Self { avg: super::filters::Boxcar::new(window), p: 0.0 }
    }

    #[inline]
    pub fn push(&mut self, x: f32) -> f32 {
        self.p = self.avg.push(x * x);
        self.p
    }

    pub fn power(&self) -> f32 {
        self.p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::osc::Osc;

    #[test]
    fn detects_reversal_with_good_timing() {
        let mut osc = Osc::new(2400.0);
        let mut det = ToneDetector::new(2400.0, 40);
        let rev_at = 2000u64;
        let mut found = None;
        for n in 0..4000u64 {
            if n == rev_at {
                osc.reverse();
            }
            let x = 0.3 * osc.next_cos() + 0.2 * (n as f32 * 0.7).sin() * 0.0;
            if let Some(t) = det.push(x) {
                found = Some(t);
            }
        }
        let t = found.expect("inversion non détectée");
        let err = t as i64 - rev_at as i64;
        assert!(err.abs() <= 4, "erreur de datation {err} échantillons");
    }

    #[test]
    fn rejects_other_tone() {
        let mut a = Osc::new(1800.0);
        let mut det = ToneDetector::new(2400.0, 40);
        for _ in 0..800 {
            det.push(0.3 * a.next_cos());
        }
        assert!(det.amplitude() < 0.01);
    }
}
