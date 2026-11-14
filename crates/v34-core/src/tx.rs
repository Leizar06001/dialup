//! Émetteur : produit le signal de ligne selon le « mode » courant.
//! Chaque mode génère ses échantillons ; la machine d'états change de
//! mode au bon échantillon.

use std::collections::VecDeque;

use crate::api::NOMINAL_RMS;
use crate::dsp::fsk::{FskMod, V21Channel};
use crate::dsp::osc::Osc;
use crate::info::InfoTx;
use crate::probe::ProbeGen;
use crate::v8::AnsamGen;

/// Émission FSK V.21 : une file de bits, puis éventuellement une séquence
/// répétée indéfiniment (CM ou JM).
pub struct FskTx {
    m: FskMod,
    queue: VecDeque<bool>,
    repeat: Vec<bool>,
    pos: usize,
    /// Nombre de bits émis depuis le début de la séquence répétée
    pub bits_sent: u64,
}

impl FskTx {
    pub fn new(ch: V21Channel) -> Self {
        Self { m: FskMod::new(ch), queue: VecDeque::new(), repeat: Vec::new(), pos: 0, bits_sent: 0 }
    }

    pub fn set_repeat(&mut self, seq: Vec<bool>) {
        self.repeat = seq;
        self.pos = 0;
    }

    /// Arrête la répétition à la fin de l'octet en cours (bloc de 10 bits
    /// après le préambule de 20 bits), puis émet `then`.
    pub fn finish_octet_then(&mut self, then: &[bool]) {
        // Position dans la séquence : préambule 20 bits puis octets de 10.
        let remaining = if self.pos == 0 {
            0
        } else if self.pos < 20 {
            20 - self.pos
        } else {
            (10 - (self.pos - 20) % 10) % 10
        };
        let seq = std::mem::take(&mut self.repeat);
        for i in 0..remaining {
            self.queue.push_back(seq[(self.pos + i) % seq.len()]);
        }
        self.queue.extend(then.iter().copied());
        self.pos = 0;
    }

    /// Plus rien à émettre.
    pub fn idle(&self) -> bool {
        self.queue.is_empty() && self.repeat.is_empty() && self.m.needs_bit()
    }

    fn next_bit(&mut self) -> Option<bool> {
        if let Some(b) = self.queue.pop_front() {
            return Some(b);
        }
        if self.repeat.is_empty() {
            return None;
        }
        let b = self.repeat[self.pos];
        self.pos = (self.pos + 1) % self.repeat.len();
        Some(b)
    }

    pub fn next_sample(&mut self) -> f32 {
        if self.m.needs_bit() {
            match self.next_bit() {
                Some(b) => {
                    self.bits_sent += 1;
                    self.m.set_bit(b)
                }
                None => return 0.0,
            }
        }
        NOMINAL_RMS * std::f32::consts::SQRT_2 * self.m.next_sample()
    }
}

/// Ton A (2400 Hz + garde 1800 Hz) ou B (1200 Hz), avec inversions de
/// phase programmées à l'échantillon près.
pub struct ToneTx {
    osc: Osc,
    guard: Option<Osc>,
    amp: f32,
    guard_amp: f32,
    reversals: VecDeque<u64>,
}

impl ToneTx {
    /// Ton A : 2400 Hz à −1 dB + garde 1800 Hz à −7 dB (§10.1.2.1 ; le
    /// niveau de garde est pris identique à celui des INFO de l'appelé).
    pub fn tone_a() -> Self {
        let a = NOMINAL_RMS * std::f32::consts::SQRT_2;
        Self { osc: Osc::new(2400.0), guard: Some(Osc::new(1800.0)), amp: a * 10f32.powf(-1.0 / 20.0), guard_amp: a * 10f32.powf(-7.0 / 20.0), reversals: VecDeque::new() }
    }

    /// Ton B : 1200 Hz au niveau nominal.
    pub fn tone_b() -> Self {
        Self { osc: Osc::new(1200.0), guard: None, amp: NOMINAL_RMS * std::f32::consts::SQRT_2, guard_amp: 0.0, reversals: VecDeque::new() }
    }

    /// Programme une inversion de phase à l'échantillon `at`.
    pub fn schedule_reversal(&mut self, at: u64) {
        self.reversals.push_back(at);
    }

    pub fn next_sample(&mut self, n: u64) -> f32 {
        if let Some(&r) = self.reversals.front() {
            if n >= r {
                self.osc.reverse();
                self.reversals.pop_front();
            }
        }
        let mut x = self.amp * self.osc.next_cos();
        if let Some(g) = self.guard.as_mut() {
            x += self.guard_amp * g.next_cos();
        }
        x
    }
}

pub enum TxMode {
    Silence,
    Ansam(AnsamGen),
    Fsk(FskTx),
    Tone(ToneTx),
    Info(InfoTx),
    Probe(ProbeGen),
}

pub struct Transmitter {
    pub mode: TxMode,
}

impl Default for Transmitter {
    fn default() -> Self {
        Self { mode: TxMode::Silence }
    }
}

impl Transmitter {
    pub fn next_sample(&mut self, n: u64) -> f32 {
        match &mut self.mode {
            TxMode::Silence => 0.0,
            TxMode::Ansam(g) => g.next_sample(),
            TxMode::Fsk(f) => f.next_sample(),
            TxMode::Tone(t) => t.next_sample(n),
            TxMode::Info(i) => i.next_sample(),
            TxMode::Probe(p) => p.next_sample(),
        }
    }
}
