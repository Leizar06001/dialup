//! Filtres élémentaires.

use std::collections::VecDeque;

use super::Cpx;

/// Moyenne glissante complexe (boxcar) sur N échantillons.
/// Utilisée après un mélangeur : rejette les fréquences à k·fs/N de la
/// fréquence d'analyse.
#[derive(Clone, Debug)]
pub struct BoxcarCpx {
    buf: VecDeque<Cpx>,
    sum: Cpx,
    n: usize,
}

impl BoxcarCpx {
    pub fn new(n: usize) -> Self {
        Self { buf: VecDeque::with_capacity(n + 1), sum: Cpx::ZERO, n }
    }

    #[inline]
    pub fn push(&mut self, x: Cpx) -> Cpx {
        self.buf.push_back(x);
        self.sum += x;
        if self.buf.len() > self.n {
            let old = self.buf.pop_front().unwrap();
            self.sum -= old;
        }
        self.sum * (1.0 / self.n as f32)
    }

    pub fn len(&self) -> usize {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    pub fn reset(&mut self) {
        self.buf.clear();
        self.sum = Cpx::ZERO;
    }
}

/// Moyenne glissante réelle.
#[derive(Clone, Debug)]
pub struct Boxcar {
    buf: VecDeque<f32>,
    sum: f64,
    n: usize,
}

impl Boxcar {
    pub fn new(n: usize) -> Self {
        Self { buf: VecDeque::with_capacity(n + 1), sum: 0.0, n }
    }

    #[inline]
    pub fn push(&mut self, x: f32) -> f32 {
        self.buf.push_back(x);
        self.sum += x as f64;
        if self.buf.len() > self.n {
            self.sum -= self.buf.pop_front().unwrap() as f64;
        }
        (self.sum / self.n as f64) as f32
    }
}

/// Filtre passe-bas du 1er ordre (lissage exponentiel).
#[derive(Clone, Copy, Debug)]
pub struct OnePole {
    pub y: f32,
    a: f32,
}

impl OnePole {
    /// `tau_samples` : constante de temps en échantillons.
    pub fn new(tau_samples: f32) -> Self {
        Self { y: 0.0, a: 1.0 / tau_samples.max(1.0) }
    }

    #[inline]
    pub fn push(&mut self, x: f32) -> f32 {
        self.y += self.a * (x - self.y);
        self.y
    }
}
