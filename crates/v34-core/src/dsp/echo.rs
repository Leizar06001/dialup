//! Annuleur d'écho adaptatif (NLMS) : estime la réponse du chemin d'écho
//! (notre propre émission qui revient dans notre récepteur, à cause de la
//! ligne 2 fils / du « hybride ») et la soustrait du signal reçu.
//!
//! V.34 s'entraîne quand l'autre modem se tait : pendant L1/L2 (phase 2)
//! et S/PP/TRN (phase 3). En données, on le laisse suivre lentement.

/// Couverture : 32 ms de réponse d'écho (retards de traitement compris).
pub const EC_TAPS: usize = 256;

pub struct EchoCanceller {
    w: Vec<f32>,
    /// Historique circulaire des échantillons émis
    hist: Vec<f32>,
    pos: usize,
    /// Puissance glissante de l'historique (normalisation NLMS)
    power: f32,
    pub mu: f32,
    /// Estimation de l'atténuation d'écho obtenue (ERLE, dB)
    erle_in: f32,
    erle_out: f32,
}

impl Default for EchoCanceller {
    fn default() -> Self {
        Self::new()
    }
}

impl EchoCanceller {
    pub fn new() -> Self {
        Self { w: vec![0.0; EC_TAPS], hist: vec![0.0; EC_TAPS], pos: 0, power: 0.0, mu: 0.0, erle_in: 1e-9, erle_out: 1e-9 }
    }

    /// Mémorise l'échantillon que nous venons d'émettre.
    #[inline]
    pub fn push_tx(&mut self, y: f32) {
        let old = self.hist[self.pos];
        self.power += y * y - old * old;
        if self.power < 0.0 {
            self.power = 0.0;
        }
        self.hist[self.pos] = y;
        self.pos = (self.pos + 1) % EC_TAPS;
    }

    /// Retire l'écho estimé de l'échantillon reçu `x` et adapte le filtre
    /// si `mu > 0`. Renvoie le signal nettoyé.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        if self.power < 1e-9 && self.w.iter().all(|&v| v == 0.0) {
            return x;
        }
        // hist[pos-1] = plus récent ; w[0] s'applique au plus récent
        let mut est = 0.0f32;
        let n = EC_TAPS;
        let mut idx = (self.pos + n - 1) % n;
        for i in 0..n {
            est += self.w[i] * self.hist[idx];
            idx = if idx == 0 { n - 1 } else { idx - 1 };
        }
        let e = x - est;
        if self.mu > 0.0 && self.power > 1e-6 {
            let g = self.mu * e / (self.power + 1e-6);
            let mut idx = (self.pos + n - 1) % n;
            for i in 0..n {
                self.w[i] += g * self.hist[idx];
                idx = if idx == 0 { n - 1 } else { idx - 1 };
            }
            self.erle_in = 0.999 * self.erle_in + 0.001 * x * x;
            self.erle_out = 0.999 * self.erle_out + 0.001 * e * e;
        }
        e
    }

    /// Puissances (entrée, sortie) mesurées pendant l'entraînement.
    pub fn powers(&self) -> (f32, f32) {
        (self.erle_in, self.erle_out)
    }

    /// Norme des coefficients (débogage).
    pub fn weight_norm(&self) -> f32 {
        self.w.iter().map(|v| v * v).sum::<f32>().sqrt()
    }

    /// Atténuation d'écho mesurée pendant l'entraînement (dB).
    pub fn erle_db(&self) -> f32 {
        10.0 * (self.erle_in / self.erle_out.max(1e-12)).log10()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancels_a_filtered_delayed_echo() {
        let mut ec = EchoCanceller::new();
        ec.mu = 0.1;
        // Chemin d'écho : retard 30 éch., petit filtre, −20 dB
        let path = [0.0f32, 0.06, 0.1, 0.05, -0.02];
        let mut txh = vec![0.0f32; 64];
        let mut seed = 7u32;
        let mut residual = 0.0f32;
        let mut echo_p = 0.0f32;
        for n in 0..40000 {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let y = (seed as f32 / u32::MAX as f32 - 0.5) * 0.5;
            txh.rotate_right(1);
            txh[0] = y;
            let echo: f32 = path.iter().enumerate().map(|(i, h)| h * txh[30 + i]).sum();
            let out = ec.process(echo);
            ec.push_tx(y);
            if n > 30000 {
                residual += out * out;
                echo_p += echo * echo;
            }
        }
        let erle = 10.0 * (echo_p / residual).log10();
        assert!(erle > 30.0, "ERLE {erle:.1} dB");
    }
}

#[cfg(test)]
mod line_tests {
    use super::*;
    use crate::line::{EchoPath, LineParams};
    use crate::probe::ProbeGen;

    #[test]
    fn cancels_line_echo_of_probe_signal() {
        for preset in ["Urbaine", "Longue distance"] {
            let p = LineParams::preset(preset).unwrap();
            let mut path = EchoPath::new(p);
            let mut ec = EchoCanceller::new();
            ec.mu = 0.05;
            let mut g = ProbeGen::new(0.0);
            let mut prev_tx = [0.0f32; 16];
            let (mut ein, mut eout) = (0.0f64, 0.0f64);
            for blk in 0..400 {
                let mut rx = [0.0f32; 16];
                path.process(&prev_tx, &mut rx);
                let mut tx = [0.0f32; 16];
                for i in 0..16 {
                    let e = ec.process(rx[i]);
                    tx[i] = g.next_sample();
                    ec.push_tx(tx[i]);
                    if blk > 300 {
                        ein += (rx[i] as f64).powi(2);
                        eout += (e as f64).powi(2);
                    }
                }
                prev_tx = tx;
            }
            let erle = 10.0 * (ein / eout).log10();
            assert!(erle > 35.0, "{preset}: ERLE sondage {erle:.1} dB");
        }
    }
}

#[cfg(test)]
mod qam_tests {
    use super::*;
    use crate::line::{EchoPath, LineParams};
    use crate::train::{QamTx, TrainMapper};

    #[test]
    fn cancels_line_echo_of_trn() {
        for preset in ["Urbaine", "Longue distance"] {
            let p = LineParams::preset(preset).unwrap();
            let mut path = EchoPath::new(p);
            let mut ec = EchoCanceller::new();
            let mut tx = QamTx::new(3200.0, 1828.57);
            let mut m = TrainMapper::new(true);
            let syms: Vec<_> = (0..4000).map(|_| m.trn_symbol()).collect();
            tx.push(&syms);
            let mut prev_tx = [0.0f32; 16];
            let (mut ein, mut eout) = (0.0f64, 0.0f64);
            for blk in 0..300 {
                ec.mu = if blk < 50 { 1.0 } else if blk < 100 { 0.5 } else if blk < 200 { 0.2 } else { 0.05 };
                let mut rx = [0.0f32; 16];
                path.process(&prev_tx, &mut rx);
                let mut t = [0.0f32; 16];
                for i in 0..16 {
                    let e = ec.process(rx[i]);
                    t[i] = tx.next_sample();
                    ec.push_tx(t[i]);
                    if blk > 250 {
                        ein += (rx[i] as f64).powi(2);
                        eout += (e as f64).powi(2);
                    }
                }
                prev_tx = t;
            }
            let erle = 10.0 * (ein / eout).log10();
            assert!(erle > 25.0, "{preset}: ERLE TRN {erle:.1} dB");
        }
    }
}
