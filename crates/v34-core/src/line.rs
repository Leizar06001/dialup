//! Simulateur de ligne téléphonique analogique.
//!
//! Appliqué au signal d'UN sens de transmission (8 kHz, f32).
//! L'écho local (signal émis qui revient dans son propre récepteur) est
//! géré par `EchoPath`, à additionner côté réception.
//!
//! Chaîne de traitement d'un sens (dans l'ordre) :
//! délai → décalage de fréquence → bande passante + pente HF + atténuation
//! → coupures → clics impulsifs → bruit blanc gaussien.
//!
//! Le SNR est défini au niveau du récepteur : un signal émis au niveau
//! nominal (`NOMINAL_RMS`) et atténué par la ligne arrive avec un rapport
//! `snr_db` au-dessus du bruit. Le bruit est ajouté après la ligne (il ne
//! dépend pas du signal), son niveau suit donc l'atténuation réglée.
//!
//! Tous les réglages peuvent changer en direct : les filtres et délais sont
//! fondus enchaînés sur 20 ms pour éviter les clics.

use std::f64::consts::PI;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::api::{NOMINAL_RMS, SAMPLE_RATE};

const FS: f64 = SAMPLE_RATE as f64;
/// Durée des fondus lors d'un changement de réglage (20 ms).
const FADE_LEN: usize = 160;
/// Délai maximal simulé (1 s).
const MAX_DELAY_SAMPLES: usize = 8000;
const HILBERT_TAPS: usize = 63;
const HILBERT_DELAY: usize = (HILBERT_TAPS - 1) / 2;
/// Fréquence où `hf_tilt_db` est atteint.
const TILT_REF_HZ: f64 = 3000.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LineParams {
    /// Bord bas de la bande passante (Hz), typ. 300
    pub band_low_hz: f32,
    /// Bord haut de la bande passante (Hz), typ. 3400
    pub band_high_hz: f32,
    /// Atténuation de la ligne (dB, ≥ 0)
    pub attenuation_db: f32,
    /// Pente d'atténuation supplémentaire dans les aigus (dB à 3 kHz) — ligne longue
    pub hf_tilt_db: f32,
    /// Rapport signal/bruit (dB) ; >= 80 = pas de bruit
    pub snr_db: f32,
    /// Parasites impulsifs (clics) par seconde, 0 = aucun
    pub impulses_per_s: f32,
    /// Décalage de fréquence (Hz), typ. 0..±7 (lignes FDM anciennes)
    pub freq_offset_hz: f32,
    /// Délai de propagation ajouté (ms)
    pub delay_ms: f32,
    /// Niveau de l'écho local (dB par rapport au signal émis), <= -80 = aucun
    pub echo_db: f32,
    /// Délai de l'écho local (ms)
    pub echo_delay_ms: f32,
    /// Coupures (probabilité par seconde d'une coupure de 20-100 ms)
    pub dropouts_per_s: f32,
}

impl LineParams {
    /// Ligne parfaite : canal numérique transparent.
    pub fn perfect() -> Self {
        Self {
            band_low_hz: 0.0,
            band_high_hz: 4000.0,
            attenuation_db: 0.0,
            hf_tilt_db: 0.0,
            snr_db: 100.0,
            impulses_per_s: 0.0,
            freq_offset_hz: 0.0,
            delay_ms: 0.0,
            echo_db: -100.0,
            echo_delay_ms: 0.0,
            dropouts_per_s: 0.0,
        }
    }

    /// Liste des préréglages (nom affiché, paramètres).
    pub fn presets() -> Vec<(&'static str, LineParams)> {
        let p = Self::perfect();
        vec![
            ("Parfaite", p.clone()),
            (
                "Urbaine",
                LineParams { band_low_hz: 200.0, band_high_hz: 3700.0, attenuation_db: 6.0, hf_tilt_db: 2.0, snr_db: 42.0, delay_ms: 5.0, echo_db: -30.0, echo_delay_ms: 2.0, ..p.clone() },
            ),
            (
                "Campagne",
                LineParams { band_low_hz: 300.0, band_high_hz: 3400.0, attenuation_db: 12.0, hf_tilt_db: 6.0, snr_db: 33.0, impulses_per_s: 0.2, delay_ms: 10.0, echo_db: -24.0, echo_delay_ms: 4.0, ..p.clone() },
            ),
            (
                "Longue distance",
                LineParams { band_low_hz: 300.0, band_high_hz: 3300.0, attenuation_db: 18.0, hf_tilt_db: 9.0, snr_db: 27.0, freq_offset_hz: 2.0, delay_ms: 60.0, echo_db: -20.0, echo_delay_ms: 6.0, ..p.clone() },
            ),
            (
                "Orage",
                LineParams { band_low_hz: 300.0, band_high_hz: 3200.0, attenuation_db: 15.0, hf_tilt_db: 8.0, snr_db: 20.0, impulses_per_s: 3.0, delay_ms: 10.0, echo_db: -18.0, echo_delay_ms: 4.0, dropouts_per_s: 0.05, ..p },
            ),
        ]
    }

    /// Cherche un préréglage par nom (insensible à la casse et aux accents
    /// usuels, espaces/tirets ignorés).
    pub fn preset(name: &str) -> Option<LineParams> {
        fn norm(s: &str) -> String {
            s.chars()
                .filter(|c| !c.is_whitespace() && *c != '-' && *c != '_')
                .map(|c| match c {
                    'é' | 'è' | 'ê' | 'É' | 'È' => 'e',
                    c => c.to_ascii_lowercase(),
                })
                .collect()
        }
        let n = norm(name);
        Self::presets().into_iter().find(|(k, _)| norm(k) == n).map(|(_, p)| p)
    }

    /// Copie des paramètres ramenés dans des plages valides.
    pub fn sanitized(&self) -> LineParams {
        fn c(v: f32, lo: f32, hi: f32, default: f32) -> f32 {
            if v.is_finite() { v.clamp(lo, hi) } else { default }
        }
        let mut p = LineParams {
            band_low_hz: c(self.band_low_hz, 0.0, 2000.0, 0.0),
            band_high_hz: c(self.band_high_hz, 1000.0, 4000.0, 4000.0),
            attenuation_db: c(self.attenuation_db, 0.0, 60.0, 0.0),
            hf_tilt_db: c(self.hf_tilt_db, 0.0, 30.0, 0.0),
            snr_db: c(self.snr_db, -10.0, 100.0, 100.0),
            impulses_per_s: c(self.impulses_per_s, 0.0, 50.0, 0.0),
            freq_offset_hz: c(self.freq_offset_hz, -50.0, 50.0, 0.0),
            delay_ms: c(self.delay_ms, 0.0, 1000.0, 0.0),
            echo_db: c(self.echo_db, -100.0, 0.0, -100.0),
            echo_delay_ms: c(self.echo_delay_ms, 0.0, 200.0, 0.0),
            dropouts_per_s: c(self.dropouts_per_s, 0.0, 10.0, 0.0),
        };
        if p.band_low_hz > p.band_high_hz - 200.0 {
            p.band_low_hz = (p.band_high_hz - 200.0).max(0.0);
        }
        p
    }

    fn noise_off(&self) -> bool {
        self.snr_db >= 80.0
    }

    fn echo_off(&self) -> bool {
        self.echo_db <= -80.0
    }

    fn same_filters(&self, o: &LineParams) -> bool {
        self.band_low_hz == o.band_low_hz
            && self.band_high_hz == o.band_high_hz
            && self.attenuation_db == o.attenuation_db
            && self.hf_tilt_db == o.hf_tilt_db
    }
}

impl Default for LineParams {
    fn default() -> Self {
        Self::perfect()
    }
}

// ───────────────────────────── briques DSP ─────────────────────────────

/// Générateur pseudo-aléatoire xorshift64* + gaussienne Box-Muller.
#[derive(Clone, Debug)]
struct Rng {
    s: u64,
    spare: Option<f64>,
}

impl Rng {
    fn new(seed: u64) -> Self {
        // Mélange (splitmix64) pour éviter les graines faibles ; jamais 0.
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        Self { s: if z == 0 { 0x1234_5678 } else { z }, spare: None }
    }

    #[inline]
    fn next_u64(&mut self) -> u64 {
        self.s ^= self.s >> 12;
        self.s ^= self.s << 25;
        self.s ^= self.s >> 27;
        self.s.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniforme dans [0, 1).
    #[inline]
    fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Gaussienne centrée réduite.
    #[inline]
    fn gauss(&mut self) -> f64 {
        if let Some(v) = self.spare.take() {
            return v;
        }
        let u1 = self.uniform().max(1e-300);
        let u2 = self.uniform();
        let r = (-2.0 * u1.ln()).sqrt();
        let (s, c) = (2.0 * PI * u2).sin_cos();
        self.spare = Some(r * s);
        r * c
    }
}

static SEED_COUNTER: AtomicU64 = AtomicU64::new(1);

fn next_seed() -> u64 {
    SEED_COUNTER.fetch_add(0x1F3D_5B79, Ordering::Relaxed)
}

/// Cellule biquadratique (forme directe II transposée, f64).
#[derive(Clone, Copy, Debug)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    fn from_coefs(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> Self {
        Self { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0, z1: 0.0, z2: 0.0 }
    }

    /// Passe-bas RBJ.
    fn lowpass(f0: f64, q: f64) -> Self {
        let w0 = 2.0 * PI * f0 / FS;
        let (s, c) = w0.sin_cos();
        let alpha = s / (2.0 * q);
        Self::from_coefs((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha)
    }

    /// Passe-haut RBJ.
    fn highpass(f0: f64, q: f64) -> Self {
        let w0 = 2.0 * PI * f0 / FS;
        let (s, c) = w0.sin_cos();
        let alpha = s / (2.0 * q);
        Self::from_coefs((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha)
    }

    /// Étagère haute RBJ (gain en dB, pente S).
    fn high_shelf(f0: f64, gain_db: f64, slope: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * f0 / FS;
        let (s, c) = w0.sin_cos();
        let alpha = s / 2.0 * ((a + 1.0 / a) * (1.0 / slope - 1.0) + 2.0).sqrt();
        let sa = 2.0 * a.sqrt() * alpha;
        Self::from_coefs(
            a * ((a + 1.0) + (a - 1.0) * c + sa),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
            a * ((a + 1.0) + (a - 1.0) * c - sa),
            (a + 1.0) - (a - 1.0) * c + sa,
            2.0 * ((a - 1.0) - (a + 1.0) * c),
            (a + 1.0) - (a - 1.0) * c - sa,
        )
    }

    #[inline]
    fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    /// |H(e^{jω})| à la fréquence `f` (Hz).
    fn magnitude(&self, f: f64) -> f64 {
        let w = 2.0 * PI * f / FS;
        // z^-1 = e^{-jω}
        let (s1, c1) = (-w).sin_cos();
        let (s2, c2) = (-2.0 * w).sin_cos();
        let nr = self.b0 + self.b1 * c1 + self.b2 * c2;
        let ni = self.b1 * s1 + self.b2 * s2;
        let dr = 1.0 + self.a1 * c1 + self.a2 * c2;
        let di = self.a1 * s1 + self.a2 * s2;
        ((nr * nr + ni * ni) / (dr * dr + di * di).max(1e-300)).sqrt()
    }

    fn take_state(&mut self, o: &Biquad) {
        self.z1 = o.z1;
        self.z2 = o.z2;
    }
}

/// Facteurs Q d'un Butterworth d'ordre 4 en deux cellules.
const BUTTER4_Q: [f64; 2] = [0.541_196_100_146_197, 1.306_562_964_876_376_7];

/// Chaîne de filtrage linéaire : passe-haut, passe-bas, pente HF, gain.
#[derive(Clone, Debug)]
struct Chain {
    hp: Option<[Biquad; 2]>,
    lp: Option<[Biquad; 2]>,
    tilt: Option<Biquad>,
    gain: f64,
}

impl Chain {
    fn bandpass(p: &LineParams) -> Self {
        let hp = (p.band_low_hz > 1.0).then(|| BUTTER4_Q.map(|q| Biquad::highpass(p.band_low_hz as f64, q)));
        let lp = (p.band_high_hz < 3990.0).then(|| BUTTER4_Q.map(|q| Biquad::lowpass(p.band_high_hz as f64, q)));
        Self { hp, lp, tilt: None, gain: 1.0 }
    }

    fn line(p: &LineParams) -> Self {
        let mut ch = Self::bandpass(p);
        if p.hf_tilt_db > 0.01 {
            ch.tilt = Some(tilt_filter(p.hf_tilt_db as f64));
        }
        ch.gain = 10f64.powf(-(p.attenuation_db as f64) / 20.0);
        ch
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let mut v = x as f64;
        if let Some(hp) = &mut self.hp {
            let t = hp[0].process(v);
            v = hp[1].process(t);
        }
        if let Some(lp) = &mut self.lp {
            let t = lp[0].process(v);
            v = lp[1].process(t);
        }
        if let Some(t) = &mut self.tilt {
            v = t.process(v);
        }
        (v * self.gain) as f32
    }

    fn magnitude(&self, f: f64) -> f64 {
        let mut m = self.gain;
        for bq in self.hp.iter().flatten().chain(self.lp.iter().flatten()).chain(self.tilt.iter()) {
            m *= bq.magnitude(f);
        }
        m
    }

    /// Reprend l'état interne d'une ancienne chaîne (cellules homologues).
    fn inherit(&mut self, old: &Chain) {
        if let (Some(n), Some(o)) = (&mut self.hp, &old.hp) {
            n[0].take_state(&o[0]);
            n[1].take_state(&o[1]);
        }
        if let (Some(n), Some(o)) = (&mut self.lp, &old.lp) {
            n[0].take_state(&o[0]);
            n[1].take_state(&o[1]);
        }
        if let (Some(n), Some(o)) = (&mut self.tilt, &old.tilt) {
            n.take_state(o);
        }
    }
}

/// Étagère haute dont l'atténuation à 3 kHz vaut exactement `tilt_db`.
fn tilt_filter(tilt_db: f64) -> Biquad {
    const F0: f64 = 1500.0;
    const SLOPE: f64 = 0.5;
    let target = -tilt_db;
    let at_ref = |g: f64| 20.0 * Biquad::high_shelf(F0, g, SLOPE).magnitude(TILT_REF_HZ).log10();
    // L'atténuation à 3 kHz croît de façon monotone avec |g| : dichotomie.
    let (mut lo, mut hi) = (3.0 * target - 6.0, 0.0);
    for _ in 0..50 {
        let mid = 0.5 * (lo + hi);
        if at_ref(mid) > target {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    Biquad::high_shelf(F0, 0.5 * (lo + hi), SLOPE)
}

/// Chaîne avec fondu enchaîné lors des changements de réglage.
#[derive(Clone, Debug)]
struct FadingChain {
    cur: Chain,
    old: Option<Chain>,
    pos: usize,
}

impl FadingChain {
    fn new(ch: Chain) -> Self {
        Self { cur: ch, old: None, pos: 0 }
    }

    fn replace(&mut self, mut ch: Chain) {
        ch.inherit(&self.cur);
        let old = std::mem::replace(&mut self.cur, ch);
        self.old = Some(old);
        self.pos = 0;
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.cur.process(x);
        match &mut self.old {
            None => y,
            Some(old) => {
                let yo = old.process(x);
                self.pos += 1;
                let a = self.pos as f32 / FADE_LEN as f32;
                if self.pos >= FADE_LEN {
                    self.old = None;
                }
                yo + (y - yo) * a.min(1.0)
            }
        }
    }
}

/// Ligne à retard avec changement de délai en fondu enchaîné.
#[derive(Clone, Debug)]
struct DelayLine {
    buf: Vec<f32>,
    w: usize,
    cur: usize,
    old: usize,
    fade: usize,
}

impl DelayLine {
    fn new(delay: usize) -> Self {
        Self { buf: vec![0.0; MAX_DELAY_SAMPLES + 1], w: 0, cur: delay.min(MAX_DELAY_SAMPLES), old: 0, fade: 0 }
    }

    fn set_delay(&mut self, delay: usize) {
        let d = delay.min(MAX_DELAY_SAMPLES);
        if d != self.cur {
            self.old = self.cur;
            self.cur = d;
            self.fade = FADE_LEN;
        }
    }

    #[inline]
    fn read(&self, d: usize) -> f32 {
        let n = self.buf.len();
        self.buf[(self.w + n - d) % n]
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        self.buf[self.w] = x;
        let y = if self.fade > 0 {
            let a = 1.0 - self.fade as f32 / FADE_LEN as f32;
            self.fade -= 1;
            let (yo, yn) = (self.read(self.old), self.read(self.cur));
            yo + (yn - yo) * a
        } else {
            self.read(self.cur)
        };
        self.w = (self.w + 1) % self.buf.len();
        y
    }
}

/// Décalage de fréquence : signal analytique (Hilbert FIR 63 coefs) × e^{jφ}.
#[derive(Clone, Debug)]
struct FreqShifter {
    taps: [f64; HILBERT_TAPS],
    hist: [f32; HILBERT_TAPS],
    pos: usize,
    phase: f64,
    step: f64,
    mix: f32,
    target: f32,
}

impl FreqShifter {
    fn new(offset_hz: f32) -> Self {
        let mut taps = [0.0; HILBERT_TAPS];
        for (k, t) in taps.iter_mut().enumerate() {
            let n = k as i64 - HILBERT_DELAY as i64;
            if n % 2 != 0 {
                let x = k as f64 / (HILBERT_TAPS - 1) as f64;
                let blackman = 0.42 - 0.5 * (2.0 * PI * x).cos() + 0.08 * (4.0 * PI * x).cos();
                *t = 2.0 / (PI * n as f64) * blackman;
            }
        }
        let mut s = Self { taps, hist: [0.0; HILBERT_TAPS], pos: 0, phase: 0.0, step: 0.0, mix: 0.0, target: 0.0 };
        s.set_offset(offset_hz);
        s.mix = s.target;
        s
    }

    fn set_offset(&mut self, offset_hz: f32) {
        self.step = 2.0 * PI * offset_hz as f64 / FS;
        self.target = if offset_hz.abs() > 1e-3 { 1.0 } else { 0.0 };
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        self.hist[self.pos] = x;
        let newest = self.pos;
        self.pos = (self.pos + 1) % HILBERT_TAPS;
        if self.mix == 0.0 && self.target == 0.0 {
            return x;
        }
        // hist[newest - k] = x(n - k)
        let at = |k: usize| self.hist[(newest + HILBERT_TAPS - k) % HILBERT_TAPS] as f64;
        let xd = at(HILBERT_DELAY);
        let mut xh = 0.0;
        // coefficients non nuls : n = k - 31 impair ⇔ k pair
        for k in (0..HILBERT_TAPS).step_by(2) {
            xh += self.taps[k] * at(k);
        }
        let (s, c) = self.phase.sin_cos();
        self.phase = (self.phase + self.step) % (2.0 * PI);
        let shifted = (xd * c - xh * s) as f32;
        let mix = self.mix;
        let step = 1.0 / FADE_LEN as f32;
        self.mix = if self.target > mix { (mix + step).min(1.0) } else { (mix - step).max(0.0) };
        if mix >= 1.0 {
            shifted
        } else {
            x + (shifted - x) * mix
        }
    }
}

/// Coupures franches de 20 à 100 ms (rampes de 1 ms).
#[derive(Clone, Debug)]
struct Dropouts {
    remaining: usize,
    env: f32,
}

impl Dropouts {
    #[inline]
    fn process(&mut self, x: f32, prob: f64, rng: &mut Rng) -> f32 {
        if self.remaining == 0 && prob > 0.0 && rng.uniform() < prob {
            self.remaining = 160 + (rng.uniform() * 640.0) as usize;
        }
        let target = if self.remaining > 0 {
            self.remaining -= 1;
            0.0
        } else {
            1.0
        };
        if self.env == 1.0 && target == 1.0 {
            return x;
        }
        self.env = if target > self.env { (self.env + 0.125).min(1.0) } else { (self.env - 0.125).max(0.0) };
        x * self.env
    }
}

/// Un clic : sinusoïde amortie.
#[derive(Clone, Debug)]
struct Impulse {
    amp: f64,
    decay: f64,
    phase: f64,
    step: f64,
}

// ───────────────────────────── simulateur ─────────────────────────────

/// Simulateur d'un sens de transmission.
pub struct LineSim {
    params: LineParams,
    delay: DelayLine,
    shifter: FreqShifter,
    chain: FadingChain,
    drops: Dropouts,
    impulses: Vec<Impulse>,
    noise_sigma: f64,
    noise_target: f64,
    rng: Rng,
}

impl LineSim {
    pub fn new(params: LineParams) -> Self {
        Self::with_seed(params, next_seed())
    }

    /// Comme `new`, avec une graine aléatoire fixée (résultats reproductibles).
    pub fn with_seed(params: LineParams, seed: u64) -> Self {
        let p = params.sanitized();
        let noise = noise_sigma_for(&p);
        Self {
            delay: DelayLine::new(ms_to_samples(p.delay_ms)),
            shifter: FreqShifter::new(p.freq_offset_hz),
            chain: FadingChain::new(Chain::line(&p)),
            drops: Dropouts { remaining: 0, env: 1.0 },
            impulses: Vec::new(),
            noise_sigma: noise,
            noise_target: noise,
            rng: Rng::new(seed),
            params: p,
        }
    }

    pub fn params(&self) -> &LineParams {
        &self.params
    }

    /// Change les paramètres en direct (sans coupure audible).
    pub fn set_params(&mut self, params: LineParams) {
        let p = params.sanitized();
        if !p.same_filters(&self.params) {
            self.chain.replace(Chain::line(&p));
        }
        self.delay.set_delay(ms_to_samples(p.delay_ms));
        self.shifter.set_offset(p.freq_offset_hz);
        self.noise_target = noise_sigma_for(&p);
        self.params = p;
    }

    /// Valeur efficace du bruit actuellement ajouté.
    pub fn noise_rms(&self) -> f32 {
        self.noise_sigma as f32
    }

    /// Traite un bloc : `output.len() == input.len()`.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        assert_eq!(input.len(), output.len(), "LineSim::process : longueurs différentes");
        let imp_prob = self.params.impulses_per_s as f64 / FS;
        let drop_prob = self.params.dropouts_per_s as f64 / FS;
        let rx_level = NOMINAL_RMS as f64 * 10f64.powf(-(self.params.attenuation_db as f64) / 20.0);
        for (x, out) in input.iter().zip(output.iter_mut()) {
            let mut y = self.delay.process(*x);
            y = self.shifter.process(y);
            y = self.chain.process(y);
            y = self.drops.process(y, drop_prob, &mut self.rng);

            // Clics : naissance poissonienne, sinusoïdes amorties.
            if imp_prob > 0.0 && self.rng.uniform() < imp_prob {
                let sign = if self.rng.uniform() < 0.5 { -1.0 } else { 1.0 };
                let tau = 0.0005 + 0.0015 * self.rng.uniform();
                self.impulses.push(Impulse {
                    amp: sign * rx_level * (2.0 + 4.0 * self.rng.uniform()),
                    decay: (-1.0 / (tau * FS)).exp(),
                    phase: 0.0,
                    step: 2.0 * PI * (300.0 + 2700.0 * self.rng.uniform()) / FS,
                });
            }
            if !self.impulses.is_empty() {
                let mut acc = 0.0;
                for imp in &mut self.impulses {
                    acc += imp.amp * imp.phase.sin();
                    imp.phase += imp.step;
                    imp.amp *= imp.decay;
                }
                self.impulses.retain(|i| i.amp.abs() > 1e-6);
                y += acc as f32;
            }

            // Bruit blanc gaussien (niveau lissé ~20 ms pour les changements).
            if self.noise_sigma != self.noise_target {
                self.noise_sigma += (self.noise_target - self.noise_sigma) * 0.006;
                if (self.noise_sigma - self.noise_target).abs() < 1e-9 {
                    self.noise_sigma = self.noise_target;
                }
            }
            if self.noise_sigma > 0.0 {
                y += (self.rng.gauss() * self.noise_sigma) as f32;
            }
            *out = y;
        }
    }

    /// Réponse en fréquence théorique (Hz, dB) pour tracer la courbe dans l'IHM.
    /// Inclut bande passante, pente HF et atténuation ; plancher à -100 dB.
    pub fn frequency_response(&self, points: usize) -> Vec<(f32, f32)> {
        let n = points.max(2);
        (0..n)
            .map(|i| {
                let f = 4000.0 * i as f64 / (n - 1) as f64;
                let db = 20.0 * self.chain.cur.magnitude(f).max(1e-5).log10();
                (f as f32, db as f32)
            })
            .collect()
    }
}

fn ms_to_samples(ms: f32) -> usize {
    (ms as f64 * FS / 1000.0).round().max(0.0) as usize
}

fn noise_sigma_for(p: &LineParams) -> f64 {
    if p.noise_off() {
        0.0
    } else {
        NOMINAL_RMS as f64 * 10f64.powf(-(p.attenuation_db as f64 + p.snr_db as f64) / 20.0)
    }
}

fn echo_gain_for(p: &LineParams) -> f64 {
    if p.echo_off() {
        0.0
    } else {
        10f64.powf(p.echo_db as f64 / 20.0)
    }
}

/// Chemin d'écho local : renvoie une copie retardée/atténuée du signal émis.
pub struct EchoPath {
    params: LineParams,
    delay: DelayLine,
    chain: FadingChain,
    gain: f64,
    gain_target: f64,
}

impl EchoPath {
    pub fn new(params: LineParams) -> Self {
        let p = params.sanitized();
        let g = echo_gain_for(&p);
        Self {
            delay: DelayLine::new(ms_to_samples(p.echo_delay_ms)),
            chain: FadingChain::new(Chain::bandpass(&p)),
            gain: g,
            gain_target: g,
            params: p,
        }
    }

    pub fn set_params(&mut self, params: LineParams) {
        let p = params.sanitized();
        if p.band_low_hz != self.params.band_low_hz || p.band_high_hz != self.params.band_high_hz {
            self.chain.replace(Chain::bandpass(&p));
        }
        self.delay.set_delay(ms_to_samples(p.echo_delay_ms));
        self.gain_target = echo_gain_for(&p);
        self.params = p;
    }

    /// Ajoute l'écho de `tx` dans `rx` (même longueur).
    pub fn process(&mut self, tx: &[f32], rx: &mut [f32]) {
        assert_eq!(tx.len(), rx.len(), "EchoPath::process : longueurs différentes");
        let step = 1.0 / FADE_LEN as f64;
        for (t, r) in tx.iter().zip(rx.iter_mut()) {
            let e = self.delay.process(*t);
            if self.gain != self.gain_target {
                let d = self.gain_target - self.gain;
                self.gain += d.clamp(-step, step);
            }
            if self.gain == 0.0 && self.gain_target == 0.0 {
                continue;
            }
            let e = self.chain.process(e) as f64 * self.gain;
            *r += e as f32;
        }
    }
}

// ───────────────────────────── tests ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, rms: f64, n: usize) -> Vec<f32> {
        let a = rms * 2f64.sqrt();
        (0..n).map(|i| (a * (2.0 * PI * freq * i as f64 / FS).sin()) as f32).collect()
    }

    fn power(x: &[f32]) -> f64 {
        x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len() as f64
    }

    fn run(sim: &mut LineSim, x: &[f32]) -> Vec<f32> {
        let mut y = vec![0.0; x.len()];
        // blocs de taille variable pour vérifier la continuité
        let mut i = 0;
        let mut b = 7;
        while i < x.len() {
            let n = b.min(x.len() - i);
            sim.process(&x[i..i + n], &mut y[i..i + n]);
            i += n;
            b = b * 3 % 211 + 1;
        }
        y
    }

    #[test]
    fn perfect_line_is_bit_exact() {
        let mut rng = Rng::new(42);
        let x: Vec<f32> = (0..5000).map(|i| if i == 10 { -0.0 } else { (rng.gauss() * 0.3) as f32 }).collect();
        let mut sim = LineSim::new(LineParams::perfect());
        let y = run(&mut sim, &x);
        for (a, b) in x.iter().zip(&y) {
            assert_eq!(a.to_bits(), b.to_bits());
        }
        let mut echo = EchoPath::new(LineParams::perfect());
        let mut rx = y.clone();
        echo.process(&x, &mut rx);
        for (a, b) in y.iter().zip(&rx) {
            assert_eq!(a.to_bits(), b.to_bits());
        }
    }

    fn measured_snr(params: LineParams) -> f64 {
        let x = sine(1000.0, NOMINAL_RMS as f64, 32000);
        let clean = run(&mut LineSim::with_seed(LineParams { snr_db: 100.0, ..params.clone() }, 1), &x);
        let noisy = run(&mut LineSim::with_seed(params, 1), &x);
        let noise: Vec<f32> = noisy.iter().zip(&clean).map(|(a, b)| a - b).collect();
        // on ignore le régime transitoire des filtres
        10.0 * (power(&clean[800..]) / power(&noise[800..])).log10()
    }

    #[test]
    fn snr_matches_setting() {
        for snr in [20.0f32, 30.0, 40.0] {
            let s = measured_snr(LineParams { snr_db: snr, ..LineParams::perfect() });
            assert!((s - snr as f64).abs() < 1.5, "consigne {snr} dB, mesuré {s:.2} dB");
        }
        // avec atténuation + bande téléphonique : le SNR reste celui réglé
        let p = LineParams { band_low_hz: 300.0, band_high_hz: 3400.0, attenuation_db: 12.0, snr_db: 30.0, ..LineParams::perfect() };
        let s = measured_snr(p);
        assert!((s - 30.0).abs() < 1.5, "mesuré {s:.2} dB");
    }

    #[test]
    fn out_of_band_tones_are_attenuated() {
        let p = LineParams { band_low_hz: 300.0, band_high_hz: 3400.0, ..LineParams::perfect() };
        let gain_db = |f: f64| {
            let x = sine(f, 0.25, 16000);
            let y = run(&mut LineSim::new(p.clone()), &x);
            10.0 * (power(&y[2000..]) / power(&x[2000..])).log10()
        };
        assert!(gain_db(1000.0).abs() < 1.0);
        assert!(gain_db(100.0) < -20.0, "100 Hz : {:.1} dB", gain_db(100.0));
        assert!(gain_db(3900.0) < -20.0, "3900 Hz : {:.1} dB", gain_db(3900.0));
        // la courbe théorique concorde
        let sim = LineSim::new(LineParams { attenuation_db: 10.0, ..p });
        let resp = sim.frequency_response(81);
        let at = |f: f32| resp.iter().find(|(x, _)| (*x - f).abs() < 1.0).unwrap().1;
        assert!((at(1000.0) + 10.0).abs() < 0.5);
        assert!(at(100.0) < -30.0);
        assert!(at(0.0) <= -99.0);
    }

    #[test]
    fn hf_tilt_reaches_target_at_3khz() {
        let sim = LineSim::new(LineParams { hf_tilt_db: 6.0, ..LineParams::perfect() });
        let resp = sim.frequency_response(81);
        let at = |f: f32| resp.iter().find(|(x, _)| (*x - f).abs() < 1.0).unwrap().1;
        assert!((at(3000.0) + 6.0).abs() < 0.05, "{}", at(3000.0));
        assert!(at(300.0) > -1.0);
    }

    #[test]
    fn delay_is_exact() {
        let mut x = vec![0.0f32; 1000];
        x[0] = 1.0;
        let y = run(&mut LineSim::new(LineParams { delay_ms: 25.0, ..LineParams::perfect() }), &x);
        let peak = y.iter().position(|v| *v == 1.0).unwrap();
        assert_eq!(peak, 200);
        assert_eq!(y.iter().filter(|v| **v != 0.0).count(), 1);
    }

    #[test]
    fn echo_path_delay_and_level() {
        let p = LineParams { echo_db: -20.0, echo_delay_ms: 4.0, ..LineParams::perfect() };
        let mut tx = vec![0.0f32; 100];
        tx[0] = 1.0;
        let mut rx = vec![0.0f32; 100];
        EchoPath::new(p).process(&tx, &mut rx);
        assert!((rx[32] - 0.1).abs() < 1e-6);
        assert_eq!(rx.iter().filter(|v| **v != 0.0).count(), 1);
    }

    fn goertzel_power(x: &[f32], f: f64) -> f64 {
        let k = 2.0 * (2.0 * PI * f / FS).cos();
        let (mut s1, mut s2) = (0.0f64, 0.0f64);
        for v in x {
            let s = *v as f64 + k * s1 - s2;
            s2 = s1;
            s1 = s;
        }
        s1 * s1 + s2 * s2 - k * s1 * s2
    }

    #[test]
    fn frequency_offset_shifts_up() {
        let x = sine(1000.0, 0.25, 16000);
        let y = run(&mut LineSim::new(LineParams { freq_offset_hz: 5.0, ..LineParams::perfect() }), &x);
        let seg = &y[8000..];
        let up = goertzel_power(seg, 1005.0);
        let orig = goertzel_power(seg, 1000.0);
        let down = goertzel_power(seg, 995.0);
        assert!(up > 100.0 * orig && up > 100.0 * down, "up {up:e} orig {orig:e} down {down:e}");
    }

    #[test]
    fn live_changes_do_not_click() {
        // Un ton en bande : changer bande/délai/atténuation ne doit pas créer de saut
        // supérieur à la pente max du signal.
        let x = sine(1000.0, 0.25, 24000);
        let mut sim = LineSim::with_seed(LineParams::perfect(), 3);
        let mut y = vec![0.0f32; x.len()];
        for (bi, chunk) in x.chunks(160).enumerate() {
            let p = match bi {
                30 => LineParams { band_low_hz: 300.0, band_high_hz: 3400.0, ..LineParams::perfect() },
                60 => LineParams { band_low_hz: 300.0, band_high_hz: 3400.0, delay_ms: 30.0, ..LineParams::perfect() },
                90 => LineParams { band_low_hz: 300.0, band_high_hz: 3400.0, delay_ms: 30.0, attenuation_db: 6.0, ..LineParams::perfect() },
                120 => LineParams { freq_offset_hz: 3.0, ..LineParams::perfect() },
                _ => sim.params().clone(),
            };
            sim.set_params(p);
            let s = bi * 160;
            sim.process(chunk, &mut y[s..s + chunk.len()]);
        }
        let max_slope = 0.25 * 2f32.sqrt() * 2.0 * std::f32::consts::PI * 1000.0 / 8000.0;
        for w in y.windows(2) {
            assert!((w[1] - w[0]).abs() < max_slope * 1.6, "saut {}", (w[1] - w[0]).abs());
        }
    }

    #[test]
    fn impulses_and_dropouts_happen() {
        let x = sine(1000.0, 0.25, 80000);
        let p = LineParams { impulses_per_s: 5.0, dropouts_per_s: 1.0, ..LineParams::perfect() };
        let y = run(&mut LineSim::with_seed(p, 9), &x);
        let diff = x.iter().zip(&y).filter(|(a, b)| (**a - **b).abs() > 1e-4).count();
        assert!(diff > 1000, "{diff}");
        let silent = y.windows(160).filter(|w| w.iter().all(|v| v.abs() < 1e-3)).count();
        assert!(silent > 0, "aucune coupure observée");
    }

    #[test]
    fn presets_lookup() {
        assert!(LineParams::preset("campagne").is_some());
        assert!(LineParams::preset("Longue-distance").is_some());
        assert!(LineParams::preset("parfaite").is_some());
        assert!(LineParams::preset("inconnu").is_none());
    }
}
