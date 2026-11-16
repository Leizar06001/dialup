//! Annuleur d'écho adaptatif (NLMS) : estime la réponse du chemin d'écho
//! (notre propre émission qui revient dans notre récepteur, à cause de la
//! ligne 2 fils / du « hybride ») et la soustrait du signal reçu.
//!
//! V.34 s'entraîne quand l'autre modem se tait : pendant L1/L2 (phase 2)
//! et S/PP/TRN (phase 3). En données, on le laisse suivre lentement.
//!
//! Deux annuleurs travaillent ensemble, sur la même erreur :
//! - l'écho **proche** ([`EchoCanceller`], 32 ms) : la ligne elle-même ;
//! - l'écho **lointain** ([`FarEchoCanceller`], 64 ms autour du délai
//!   aller-retour mesuré en phase 2) : à travers un adaptateur téléphonique,
//!   notre signal revient, réfléchi par l'hybride de sa prise, après tout
//!   l'aller-retour sur Internet (0,4 à 0,6 s mesurés avec un vrai modem).

/// Couverture : 32 ms de réponse d'écho (retards de traitement compris).
pub const EC_TAPS: usize = 256;
/// Écho lointain : 64 ms de réponse, de 56 ms avant le délai aller-retour
/// mesuré à 8 ms après. Ce délai compte aussi les temps de détection des
/// inversions de ton : ~40 ms de plus que l'écho entre deux de nos modems,
/// presque rien avec un U.S. Robotics (617,2 ms mesurés, écho à 616,9 ms).
pub const FAR_TAPS: usize = 512;
const FAR_BEFORE: usize = 448;
/// Historique de notre émission gardé pour l'écho lointain (2 s)
const FAR_SPAN: usize = 16000;
/// Pas minimal de l'écho lointain pendant son apprentissage
const FAR_MU_MIN: f32 = 0.2;
/// Atténuation (dB) en dessous de laquelle il n'y a pas d'écho lointain :
/// l'annuleur est alors coupé (voir [`FarEchoCanceller::disable`])
pub const FAR_MIN_ERLE_DB: f32 = 3.0;

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
        Self::with_taps(EC_TAPS)
    }

    pub fn with_taps(n: usize) -> Self {
        Self { w: vec![0.0; n], hist: vec![0.0; n], pos: 0, power: 0.0, mu: 0.0, erle_in: 1e-9, erle_out: 1e-9 }
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
        self.pos = (self.pos + 1) % self.hist.len();
    }

    /// Puissance de l'historique (normalisation NLMS).
    #[inline]
    pub fn power(&self) -> f32 {
        self.power
    }

    /// Écho estimé pour l'échantillon reçu à venir.
    #[inline]
    pub fn estimate(&self) -> f32 {
        if self.power < 1e-9 && self.w.iter().all(|&v| v == 0.0) {
            return 0.0;
        }
        // hist[pos-1] = plus récent ; w[0] s'applique au plus récent
        let n = self.w.len();
        let mut est = 0.0f32;
        let mut idx = (self.pos + n - 1) % n;
        for i in 0..n {
            est += self.w[i] * self.hist[idx];
            idx = if idx == 0 { n - 1 } else { idx - 1 };
        }
        est
    }

    /// Adapte le filtre sur l'erreur `e`, normalisée par `norm` (la
    /// puissance de tous les annuleurs qui partagent cette erreur).
    #[inline]
    pub fn adapt(&mut self, e: f32, norm: f32) {
        if self.mu <= 0.0 || norm <= 1e-6 {
            return;
        }
        let g = self.mu * e / (norm + 1e-6);
        let n = self.w.len();
        let mut idx = (self.pos + n - 1) % n;
        for i in 0..n {
            self.w[i] += g * self.hist[idx];
            idx = if idx == 0 { n - 1 } else { idx - 1 };
        }
    }

    /// Note les puissances avant / après annulation (pour l'ERLE).
    #[inline]
    pub fn note(&mut self, x: f32, e: f32) {
        self.erle_in = 0.999 * self.erle_in + 0.001 * x * x;
        self.erle_out = 0.999 * self.erle_out + 0.001 * e * e;
    }

    /// Retire l'écho estimé de l'échantillon reçu `x` et adapte le filtre
    /// si `mu > 0`. Renvoie le signal nettoyé.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let e = x - self.estimate();
        if self.mu > 0.0 && self.power > 1e-6 {
            self.adapt(e, self.power);
            self.note(x, e);
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

    fn clear(&mut self) {
        self.w.iter_mut().for_each(|v| *v = 0.0);
    }
}

/// Annuleur d'écho lointain : un [`EchoCanceller`] qui voit notre émission
/// retardée, sa fenêtre de 64 ms placée autour du délai aller-retour.
///
/// Il apprend quand l'écho de nos signaux d'apprentissage revient, soit un
/// aller-retour APRÈS que l'annuleur proche les a appris : son pas est celui
/// de l'annuleur proche au moment de l'émission (mémorisé avec chaque
/// échantillon). V.34 garantit que l'autre se tait encore alors : il ne
/// répond qu'à notre J, qui suit notre TRN.
pub struct FarEchoCanceller {
    hist: Vec<f32>,
    /// Pas de l'annuleur proche au moment de chaque émission
    mu_hist: Vec<f32>,
    pos: usize,
    /// Retard du début de la fenêtre (échantillons) ; `None` : aller-retour
    /// trop court, l'écho proche couvre déjà tout
    delay: Option<usize>,
    ec: EchoCanceller,
}

impl Default for FarEchoCanceller {
    fn default() -> Self {
        Self::new()
    }
}

impl FarEchoCanceller {
    pub fn new() -> Self {
        Self { hist: vec![0.0; FAR_SPAN], mu_hist: vec![0.0; FAR_SPAN], pos: 0, delay: None, ec: EchoCanceller::with_taps(FAR_TAPS) }
    }

    /// Place la fenêtre autour du délai aller-retour mesuré (échantillons).
    /// Un délai changé de plus de 8 ms remet le filtre à zéro.
    pub fn set_round_trip(&mut self, rtd: usize) {
        let d = rtd.saturating_sub(FAR_BEFORE);
        let new = (d >= EC_TAPS && d + FAR_TAPS < FAR_SPAN).then_some(d);
        let moved = match (self.delay, new) {
            (Some(a), Some(b)) => a.abs_diff(b) > 64,
            (a, b) => a != b,
        };
        if moved {
            self.delay = new;
            self.ec.clear();
            // l'historique retardé repart de ce que nous avons déjà émis
            self.ec = EchoCanceller::with_taps(FAR_TAPS);
            if let Some(d) = new {
                for k in (0..FAR_TAPS).rev() {
                    let i = (self.pos + 2 * FAR_SPAN - 1 - d - k) % FAR_SPAN;
                    self.ec.push_tx(self.hist[i]);
                }
            }
        }
    }

    /// Délai du début de la fenêtre, s'il y en a une.
    pub fn delay(&self) -> Option<usize> {
        self.delay
    }

    /// Pas d'écho lointain sur cette ligne (entre deux de nos modems, ou
    /// une ligne sans adaptateur) : le filtre n'a appris que du bruit, qu'il
    /// ajouterait au signal reçu. On l'oublie jusqu'au prochain
    /// [`set_round_trip`](Self::set_round_trip) (réentraînement).
    pub fn disable(&mut self) {
        self.delay = None;
        self.ec = EchoCanceller::with_taps(FAR_TAPS);
    }

    /// Note les puissances avant / après l'annulation de l'écho lointain.
    pub fn note(&mut self, x: f32, e: f32) {
        self.ec.note(x, e);
    }

    /// Atténuation de l'écho lointain mesurée pendant son apprentissage (dB).
    pub fn erle_db(&self) -> f32 {
        self.ec.erle_db()
    }

    pub fn set_mu(&mut self, mu: f32) {
        self.ec.mu = if self.delay.is_some() { mu } else { 0.0 };
    }

    /// Pas de l'annuleur lointain pour l'échantillon qui arrive ; le renvoie.
    ///
    /// Il n'apprend que si TOUTE sa fenêtre (nos 64 ms d'émission d'il y a
    /// un aller-retour) date de l'apprentissage de l'annuleur proche : sinon
    /// une fenêtre à moitié vide (silence) fausse la normalisation. Comme le
    /// bord récent de la fenêtre est 56 ms avant le délai mesuré (qui compte
    /// ~40 ms de détections), il a aussi fini avant que l'autre réponde à
    /// notre J (son signal détruirait l'apprentissage).
    pub fn follow_delayed_mu(&mut self) -> f32 {
        let mu = match self.delay {
            Some(d) => {
                let at = |k: usize| self.mu_hist[(self.pos + FAR_SPAN - 1 - k) % FAR_SPAN];
                let (newest, oldest) = (at(d), at(d + FAR_TAPS - 1));
                if newest > 0.0 && oldest > 0.0 {
                    // fenêtre courte (~0,5 s) pour 512 coefficients : un pas
                    // plancher, quand l'annuleur proche, lui, ralentit vite
                    newest.max(FAR_MU_MIN)
                } else {
                    0.0
                }
            }
            None => 0.0,
        };
        self.ec.mu = mu;
        mu
    }

    /// Notre échantillon émis, et le pas de l'annuleur proche à cet instant.
    #[inline]
    pub fn push_tx(&mut self, y: f32, near_mu: f32) {
        self.hist[self.pos] = y;
        self.mu_hist[self.pos] = near_mu;
        self.pos = (self.pos + 1) % FAR_SPAN;
        if let Some(d) = self.delay {
            let i = (self.pos + FAR_SPAN - 1 - d) % FAR_SPAN;
            self.ec.push_tx(self.hist[i]);
        }
    }

    #[inline]
    pub fn power(&self) -> f32 {
        if self.delay.is_some() { self.ec.power() } else { 0.0 }
    }

    #[inline]
    pub fn estimate(&self) -> f32 {
        if self.delay.is_some() { self.ec.estimate() } else { 0.0 }
    }

    #[inline]
    pub fn adapt(&mut self, e: f32, norm: f32) {
        if self.delay.is_some() {
            self.ec.adapt(e, norm);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Écho proche + écho lointain (600 ms) : les deux annuleurs, sur la
    /// même erreur, enlèvent les deux.
    #[test]
    fn cancels_near_and_far_echo_together() {
        let (mut near, mut far) = (EchoCanceller::new(), FarEchoCanceller::new());
        let rtd = 4800; // 600 ms
        far.set_round_trip(rtd);
        let mut txh = vec![0.0f32; 6000];
        let mut seed = 11u32;
        let (mut residual, mut echo_p) = (0.0f32, 0.0f32);
        for n in 0..60000 {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let y = (seed as f32 / u32::MAX as f32 - 0.5) * 0.5;
            txh.rotate_right(1);
            txh[0] = y;
            // proche : 3 ms ; lointain : 2 ms avant l'aller-retour, −20 dB
            let echo = 0.05 * txh[24] + 0.1 * txh[rtd - 16] - 0.04 * txh[rtd - 15];
            near.mu = 0.5;
            far.set_mu(0.5);
            let est = near.estimate() + far.estimate();
            let out = echo - est;
            let norm = near.power() + far.power();
            near.adapt(out, norm);
            far.adapt(out, norm);
            near.push_tx(y);
            far.push_tx(y, 0.5);
            if n > 50000 {
                residual += out * out;
                echo_p += echo * echo;
            }
        }
        let erle = 10.0 * (echo_p / residual).log10();
        assert!(erle > 30.0, "ERLE {erle:.1} dB");
    }

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

    /// L'écho lointain d'un ATA (500 ms, filtré par la ligne, −20 dB),
    /// appris sur 0,5 s de TRN comme en phase 3 : il doit tomber nettement.
    #[test]
    fn cancels_far_echo_of_trn() {
        let p = LineParams { echo_db: -20.0, echo_delay_ms: 500.0, ..LineParams::preset("Urbaine").unwrap() };
        let mut path = EchoPath::new(p);
        let mut far = FarEchoCanceller::new();
        far.set_round_trip(4000 + 320); // délai mesuré : écho + 40 ms (détections)
        let mut tx = QamTx::new(3429.0, 1959.0);
        let mut m = TrainMapper::new(true);
        let syms: Vec<_> = (0..8000).map(|_| m.trn_symbol()).collect();
        tx.push(&syms);
        let mut prev_tx = [0.0f32; 16];
        let (mut ein, mut eout) = (0.0f64, 0.0f64);
        // 0,5 s d'émission « en apprentissage » (pas 1), puis on écoute l'écho
        for blk in 0..1000 {
            let mut rx = [0.0f32; 16];
            path.process(&prev_tx, &mut rx);
            let mut t = [0.0f32; 16];
            for i in 0..16 {
                let mu = far.follow_delayed_mu();
                let x = rx[i];
                let e = x - far.estimate();
                if mu > 0.0 {
                    far.adapt(e, far.power());
                    far.note(x, e);
                }
                t[i] = tx.next_sample();
                far.push_tx(t[i], if blk < 250 { 1.0 } else { 0.0 });
                if (600..800).contains(&blk) {
                    ein += (x as f64).powi(2);
                    eout += (e as f64).powi(2);
                }
            }
            prev_tx = t;
        }
        let erle = 10.0 * (ein / eout).log10();
        eprintln!("écho lointain : ERLE après apprentissage {erle:.1} dB (mesure interne {:.1} dB)", far.erle_db());
        assert!(erle > 20.0, "ERLE {erle:.1} dB");
    }
}

