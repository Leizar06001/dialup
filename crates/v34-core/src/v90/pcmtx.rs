//! Émetteur PCM du modem numérique (le FAI) : signaux des phases 3 et 4
//! (§8.4, §8.6/V.90) puis données (§5.4).
//!
//! Un symbole par échantillon (8 kHz), chacun exactement un niveau G.711.
//! La machine principale (phases 3/4) pilote l'enchaînement : Sd, S̄d,
//! TRN1d puis Jd en boucle ; J′d et DIL sur ordre ; Ri jusqu'au CPt de
//! l'autre ; R̄i, TRN2d, MP ; Ed, B1d et données sur ordre.

use std::collections::VecDeque;

use super::codes::{self, Law};
use super::frame::{Encoder, Mapping, Symbol, FRAME};
use super::seq::DilDescriptor;
use crate::train::Scrambler;

/// Longueur de TRN1d (≥ 2040T exigés, multiple de 6) : l'abonné y apprend
/// son égaliseur.
pub const TRN1D_LEN: usize = 4080;
/// Longueur de TRN2d (≥ 2040T, multiple de 6).
pub const TRN2D_LEN: usize = 3000;
/// Durée minimale de Ri.
const RI_MIN: usize = 192;

/// Ce qui vient de se passer à l'émission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TxEvent {
    /// Début d'un signal (nom affiché)
    Signal(&'static str),
    /// Fin d'une séquence MP (ou MP′)
    MpSent { ack: bool },
    /// B1d émis : les données commencent
    DataStarted,
}

#[derive(Clone, Debug, PartialEq)]
enum St {
    Silence,
    /// Sd (384T) puis S̄d (48T)
    Sd { i: usize },
    Trn1d { i: usize },
    Jd { pos: usize },
    JdPrime { i: usize },
    Dil { seg: usize, i: usize },
    Ri { i: usize },
    RiBar { i: usize },
    Trn2d { frames: usize },
    Mp { pos: usize },
    Ed { frames: usize },
    B1d { frames: usize },
    Data,
}

pub struct PcmTx {
    /// CRC de nos séquences (Jd) dans l'ancien ordre (voir `CrcMode`)
    pub crc_legacy: bool,
    law: Law,
    uinfo: u8,
    st: St,
    queue: VecDeque<f32>,
    events: Vec<TxEvent>,
    /// Brouilleur GPC de TRN1d / Jd / J′d, et dernier signe émis
    scr: Scrambler,
    last_sign: bool,
    jd_bits: Vec<bool>,
    /// Ordre de terminer Jd par J′d
    finish_jd: bool,
    dil: Option<DilDescriptor>,
    stop_dil: bool,
    /// Ordre de passer de Ri à R̄i + TRN2d, avec le jeu de la phase 4
    train_map: Option<Mapping>,
    enc: Option<Encoder>,
    mp_bits: Vec<bool>,
    mp_ack: bool,
    /// Ordre de terminer les MP par Ed, B1d, données
    finish_mp: bool,
    data_map: Option<Mapping>,
    /// Symboles émis depuis le début de TRN1d, jusqu'à ce que l'abonné
    /// reparle (son S) : fenêtre d'apprentissage de notre annuleur d'écho
    since_trn1d: Option<usize>,
    /// Symboles de DIL émis
    dil_sent: usize,
}

impl PcmTx {
    pub fn new(law: Law, uinfo: u8) -> PcmTx {
        PcmTx {
            law,
            uinfo,
            st: St::Silence,
            queue: VecDeque::new(),
            events: Vec::new(),
            scr: Scrambler::new(true),
            last_sign: true,
            jd_bits: Vec::new(),
            crc_legacy: false,
            finish_jd: false,
            dil: None,
            stop_dil: false,
            train_map: None,
            enc: None,
            mp_bits: Vec::new(),
            mp_ack: false,
            finish_mp: false,
            data_map: None,
            since_trn1d: None,
            dil_sent: 0,
        }
    }

    pub fn take_events(&mut self) -> Vec<TxEvent> {
        std::mem::take(&mut self.events)
    }

    fn signal(&mut self, name: &'static str) {
        self.events.push(TxEvent::Signal(name));
    }

    fn sym(&self, u: u8, positive: bool) -> f32 {
        codes::sample(self.law, u, positive)
    }

    /// Phase 3, après réception de Ja : Sd, S̄d, TRN1d, puis Jd en boucle.
    pub fn start_phase3(&mut self, jd: &super::seq::Jd, dil: Option<DilDescriptor>) {
        self.jd_bits = jd.to_bits_crc(self.crc_legacy);
        self.dil = dil.filter(|d| !d.train.is_empty());
        self.finish_jd = false;
        self.stop_dil = false;
        self.st = St::Sd { i: 0 };
        self.signal("Sd");
    }

    /// L'abonné a reçu Jd (son S est là) : finir le Jd en cours, J′d, puis DIL.
    pub fn finish_jd(&mut self) {
        self.finish_jd = true;
        self.since_trn1d = None;
    }

    /// L'abonné a assez écouté le DIL : finir le segment en cours, puis Ri.
    pub fn stop_dil(&mut self) {
        self.stop_dil = true;
        if matches!(self.st, St::Silence) {
            self.st = St::Ri { i: 0 };
            self.signal("Ri");
        }
    }

    /// CPt reçu : finir la période de Ri, R̄i, puis TRN2d et MP avec ce jeu.
    pub fn start_trn2d(&mut self, train: Mapping, mp_bits: Vec<bool>) {
        self.train_map = Some(train);
        self.set_mp(mp_bits, false);
    }

    /// Change la MP émise (acquittement, débit montant).
    pub fn set_mp(&mut self, bits: Vec<bool>, ack: bool) {
        self.mp_bits = bits;
        self.mp_ack = ack;
    }

    /// CP′ ou E reçu après notre MP′ : Ed, puis B1d et données avec ce jeu.
    pub fn finish_mp(&mut self, data: Mapping) {
        self.data_map = Some(data);
        self.finish_mp = true;
    }

    /// Passages complets du DIL déjà émis (le demandeur en écoute au moins un).
    pub fn dil_passes_sent(&self) -> f64 {
        match &self.dil {
            Some(d) if d.total_len() > 0 => self.dil_sent as f64 / d.total_len() as f64,
            _ => f64::MAX,
        }
    }

    pub fn in_data(&self) -> bool {
        self.st == St::Data
    }

    /// Symboles émis depuis le début de TRN1d, tant que l'abonné n'a pas
    /// repris la parole : il se tait alors (après un aller-retour), notre
    /// annuleur d'écho peut apprendre sur notre PCM.
    pub fn trn1d_index(&self) -> Option<usize> {
        self.since_trn1d
    }

    /// Prochain échantillon émis ; `bit` fournit les bits de données.
    pub fn next_sample(&mut self, bit: &mut dyn FnMut() -> bool) -> f32 {
        while self.queue.is_empty() {
            self.produce(bit);
        }
        if let Some(i) = self.since_trn1d.as_mut() {
            *i += 1;
        }
        self.queue.pop_front().unwrap()
    }

    fn produce(&mut self, bit: &mut dyn FnMut() -> bool) {
        let w = self.uinfo.saturating_add(16).min(127);
        match self.st.clone() {
            St::Silence => self.queue.push_back(0.0),
            St::Sd { i } => {
                // {+W, +0, +W, −W, −0, −W} ×64, puis l'inverse ×8 (§8.4.4)
                const PAT: [(bool, bool); 6] = [(true, true), (false, true), (true, true), (true, false), (false, false), (true, false)];
                let (is_w, pos) = PAT[i % 6];
                let pos = if i >= 384 { !pos } else { pos };
                self.queue.push_back(self.sym(if is_w { w } else { 0 }, pos));
                if i == 383 {
                    self.signal("S̄d");
                }
                self.st = if i + 1 >= 432 {
                    self.scr.reset();
                    self.since_trn1d = Some(0);
                    self.signal("TRN1d");
                    St::Trn1d { i: 0 }
                } else {
                    St::Sd { i: i + 1 }
                };
            }
            St::Trn1d { i } => {
                // ±UINFO, signes = uns brouillés (§8.4.5)
                let s = self.scr.scramble(true);
                self.last_sign = s;
                self.queue.push_back(self.sym(self.uinfo, s));
                self.st = if i + 1 >= TRN1D_LEN {
                    self.signal("Jd");
                    St::Jd { pos: 0 }
                } else {
                    St::Trn1d { i: i + 1 }
                };
            }
            St::Jd { pos } => {
                let b = self.jd_bits[pos];
                self.diff_sign(b);
                let next = pos + 1;
                self.st = if next < self.jd_bits.len() {
                    St::Jd { pos: next }
                } else if self.finish_jd {
                    self.signal("J′d");
                    St::JdPrime { i: 0 }
                } else {
                    St::Jd { pos: 0 }
                };
            }
            St::JdPrime { i } => {
                self.diff_sign(false);
                self.st = if i + 1 < 12 {
                    St::JdPrime { i: i + 1 }
                } else if self.dil.is_some() && !self.stop_dil {
                    self.signal("DIL");
                    St::Dil { seg: 0, i: 0 }
                } else if self.stop_dil {
                    self.signal("Ri");
                    St::Ri { i: 0 }
                } else {
                    // pas de DIL demandé : on attend le S̄ de l'abonné
                    St::Silence
                };
            }
            St::Dil { seg, i } => {
                let dil = self.dil.as_ref().unwrap();
                let (u, pos) = dil.symbol(seg, i);
                let len = dil.segment_len(seg);
                let nseg = dil.train.len();
                self.queue.push_back(self.sym(u, pos));
                self.dil_sent += 1;
                self.st = if i + 1 < len {
                    St::Dil { seg, i: i + 1 }
                } else if self.stop_dil {
                    self.signal("Ri");
                    St::Ri { i: 0 }
                } else {
                    St::Dil { seg: (seg + 1) % nseg, i: 0 }
                };
            }
            St::Ri { i } => {
                // UINFO, signes + + + − − − (§8.6.4)
                for k in 0..FRAME {
                    self.queue.push_back(self.sym(self.uinfo, k < 3));
                }
                let i = i + FRAME;
                self.st = if i >= RI_MIN && self.train_map.is_some() {
                    self.signal("R̄i");
                    St::RiBar { i: 0 }
                } else {
                    St::Ri { i }
                };
            }
            St::RiBar { i } => {
                for k in 0..FRAME {
                    self.queue.push_back(self.sym(self.uinfo, k >= 3));
                }
                self.st = if i + FRAME >= 24 {
                    // TRN2d : uns brouillés à travers le codeur (§8.6.5)
                    self.enc = Some(Encoder::new(self.train_map.clone().unwrap()));
                    self.signal("TRN2d");
                    St::Trn2d { frames: 0 }
                } else {
                    St::RiBar { i: i + FRAME }
                };
            }
            St::Trn2d { frames } => {
                self.encode_frame(&mut || true);
                self.st = if (frames + 1) * FRAME >= TRN2D_LEN {
                    self.signal(if self.mp_ack { "MP′" } else { "MP" });
                    St::Mp { pos: 0 }
                } else {
                    St::Trn2d { frames: frames + 1 }
                };
            }
            St::Mp { pos } => {
                // une trame de bits MP (complétée de zéros en fin de MP)
                let d = self.enc.as_ref().unwrap().map.bits_per_frame() as usize;
                let n = self.mp_bits.len();
                let mut p = pos;
                let mut frame_bits = Vec::with_capacity(d);
                for _ in 0..d {
                    frame_bits.push(if p < n { self.mp_bits[p] } else { false });
                    p += 1;
                }
                let mut it = frame_bits.into_iter();
                self.encode_frame(&mut || it.next().unwrap_or(false));
                self.st = if p >= n {
                    let ack = self.mp_ack;
                    self.events.push(TxEvent::MpSent { ack });
                    if self.finish_mp && ack {
                        self.signal("Ed");
                        St::Ed { frames: 0 }
                    } else {
                        St::Mp { pos: 0 }
                    }
                } else {
                    St::Mp { pos: p }
                };
            }
            St::Ed { frames } => {
                self.encode_frame(&mut || false);
                self.st = if frames + 1 >= 2 {
                    // B1d : nouveau codeur (brouilleur à zéro), jeu des données
                    self.enc = Some(Encoder::new(self.data_map.clone().unwrap()));
                    self.signal("B1d");
                    St::B1d { frames: 0 }
                } else {
                    St::Ed { frames: frames + 1 }
                };
            }
            St::B1d { frames } => {
                self.encode_frame(&mut || true);
                self.st = if frames + 1 >= 48 {
                    self.events.push(TxEvent::DataStarted);
                    St::Data
                } else {
                    St::B1d { frames: frames + 1 }
                };
            }
            St::Data => self.encode_frame(bit),
        }
    }

    /// Bit codé différentiellement dans le signe de UINFO (Jd, J′d).
    fn diff_sign(&mut self, b: bool) {
        let s = self.scr.scramble(b) ^ self.last_sign;
        self.last_sign = s;
        self.queue.push_back(self.sym(self.uinfo, s));
    }

    fn encode_frame(&mut self, bit: &mut dyn FnMut() -> bool) {
        let enc = self.enc.as_mut().expect("codeur PCM");
        let d = enc.map.bits_per_frame() as usize;
        let bits: Vec<bool> = (0..d).map(|_| bit()).collect();
        let syms = enc.frame(&bits);
        let law = self.law;
        self.queue.extend(syms.iter().map(|s: &Symbol| codes::sample(law, s.ucode, s.positive)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v90::seq::Jd;

    #[test]
    fn enchainement_de_la_phase_3() {
        let mut tx = PcmTx::new(Law::A, 96);
        tx.start_phase3(&Jd::upto(56000), None);
        let mut no = || false;
        let s: Vec<f32> = (0..432 + TRN1D_LEN + 2 * Jd::LEN).map(|_| tx.next_sample(&mut no)).collect();
        // Sd périodique de période 6, S̄d inversé
        for i in 6..384 {
            assert_eq!(s[i], s[i - 6]);
        }
        for i in 384..390 {
            assert_eq!(s[i], -s[i - 6], "S→S̄d");
        }
        for i in 390..432 {
            assert_eq!(s[i], s[i - 6]);
        }
        // TRN1d puis Jd : toujours ±UINFO
        let a = codes::sample(Law::A, 96, true);
        assert!(s[432..].iter().all(|x| x.abs() == a));
        let ev = tx.take_events();
        assert_eq!(ev, ["Sd", "S̄d", "TRN1d", "Jd"].map(TxEvent::Signal).to_vec());
        // J′d puis (sans DIL) silence
        tx.finish_jd();
        let s: Vec<f32> = (0..200).map(|_| tx.next_sample(&mut no)).collect();
        assert_eq!(*s.last().unwrap(), 0.0);
        assert!(tx.take_events().contains(&TxEvent::Signal("J′d")));
    }
}
