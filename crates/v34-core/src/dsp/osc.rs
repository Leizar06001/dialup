//! Oscillateur numérique (phase en f64 pour éviter la dérive).

use std::f64::consts::TAU;

use crate::api::SAMPLE_RATE;

#[derive(Clone, Debug)]
pub struct Osc {
    phase: f64,
    step: f64,
}

impl Osc {
    pub fn new(freq_hz: f64) -> Self {
        Self { phase: 0.0, step: TAU * freq_hz / SAMPLE_RATE as f64 }
    }

    pub fn set_freq(&mut self, freq_hz: f64) {
        self.step = TAU * freq_hz / SAMPLE_RATE as f64;
    }

    /// Inverse la phase (saut de 180°).
    pub fn reverse(&mut self) {
        self.phase = (self.phase + std::f64::consts::PI) % TAU;
    }

    pub fn reset(&mut self) {
        self.phase = 0.0;
    }

    /// Échantillon suivant de cos(phase).
    #[inline]
    pub fn next_cos(&mut self) -> f32 {
        let v = self.phase.cos() as f32;
        self.phase = (self.phase + self.step) % TAU;
        v
    }

    /// Échantillon suivant complexe (cos, sin).
    #[inline]
    pub fn next_iq(&mut self) -> (f32, f32) {
        let (s, c) = self.phase.sin_cos();
        self.phase = (self.phase + self.step) % TAU;
        (c as f32, s as f32)
    }
}
