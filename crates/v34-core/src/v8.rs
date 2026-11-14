//! ITU-T V.8 : procédure de démarrage (ANSam, CM, JM, CJ).
//! Référence : docs/specs/ITU-T_V.8_2000-11.pdf, clauses 5 à 8.

use std::f64::consts::{PI, TAU};

use crate::api::{NOMINAL_RMS, SAMPLE_RATE};
use crate::dsp::filters::Boxcar;
use crate::dsp::tone::{PowerMeter, ToneDetector};

/// Synchro CM/JM (Table 1/V.8), dans l'ordre d'émission.
const SYNC_CM_JM: [bool; 10] = [false, false, false, false, false, false, true, true, true, true];

/// Fonctions d'appel (Table 3/V.8), valeur = b5 + 2·b6 + 4·b7.
pub const CALLF_DATA: u8 = 0b110; // b5=0 b6=1 b7=1 : « Data (V-series modem) »

/// Modes de modulation (Table 4/V.8).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modes {
    pub v34: bool,
    pub v34hdx: bool,
    pub v32bis: bool,
    pub v22bis: bool,
    pub v17: bool,
    pub v29hdx: bool,
    pub v27ter: bool,
    pub v26ter: bool,
    pub v26bis: bool,
    pub v23: bool,
    pub v23hdx: bool,
    pub v21: bool,
}

impl Modes {
    pub fn intersect(&self, o: &Modes) -> Modes {
        Modes {
            v34: self.v34 && o.v34,
            v34hdx: self.v34hdx && o.v34hdx,
            v32bis: self.v32bis && o.v32bis,
            v22bis: self.v22bis && o.v22bis,
            v17: self.v17 && o.v17,
            v29hdx: self.v29hdx && o.v29hdx,
            v27ter: self.v27ter && o.v27ter,
            v26ter: self.v26ter && o.v26ter,
            v26bis: self.v26bis && o.v26bis,
            v23: self.v23 && o.v23,
            v23hdx: self.v23hdx && o.v23hdx,
            v21: self.v21 && o.v21,
        }
    }

    pub fn names(&self) -> Vec<String> {
        let mut v = Vec::new();
        let mut add = |c: bool, n: &str| {
            if c {
                v.push(n.to_string())
            }
        };
        add(self.v34, "V.34");
        add(self.v34hdx, "V.34 HDX");
        add(self.v32bis, "V.32bis");
        add(self.v22bis, "V.22bis");
        add(self.v17, "V.17");
        add(self.v29hdx, "V.29");
        add(self.v27ter, "V.27ter");
        add(self.v26ter, "V.26ter");
        add(self.v26bis, "V.26bis");
        add(self.v23, "V.23");
        add(self.v23hdx, "V.23 HDX");
        add(self.v21, "V.21");
        v
    }
}

/// Contenu d'un CM ou d'un JM.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Menu {
    pub call_function: u8,
    pub modes: Modes,
    /// Protocole LAPM (V.42) demandé
    pub lapm: bool,
    /// Octet « accès RTC » (bits b5..b7), si présent
    pub access: Option<u8>,
}

impl Menu {
    /// Menu annoncé par notre modem : V.34 duplex + modes classiques.
    pub fn ours() -> Self {
        Menu {
            call_function: CALLF_DATA,
            modes: Modes { v34: true, v32bis: true, v22bis: true, v21: true, ..Modes::default() },
            lapm: true,
            access: None,
        }
    }

    /// Octets d'information (sans préambule ni bits start/stop).
    pub fn to_octets(&self) -> Vec<u8> {
        let m = &self.modes;
        let b = |c: bool, v: u8| if c { v } else { 0 };
        let mut o = vec![0x01 | (self.call_function & 7) << 5];
        o.push(0x05 | b(m.v34, 0x40) | b(m.v34hdx, 0x80));
        let ext1 = b(m.v32bis, 0x01) | b(m.v22bis, 0x02) | b(m.v17, 0x04) | b(m.v29hdx, 0x40) | b(m.v27ter, 0x80);
        let ext2 = b(m.v26ter, 0x01) | b(m.v26bis, 0x02) | b(m.v23, 0x04) | b(m.v23hdx, 0x40) | b(m.v21, 0x80);
        if ext1 != 0 || ext2 != 0 {
            o.push(0x10 | ext1);
        }
        if ext2 != 0 {
            o.push(0x10 | ext2);
        }
        if self.lapm {
            o.push(0x0A | 0x20);
        }
        if let Some(a) = self.access {
            o.push(0x0D | (a & 7) << 5);
        }
        o
    }

    /// Décode une suite d'octets d'information. Ignore ce qui est réservé.
    pub fn from_octets(oct: &[u8]) -> Option<Menu> {
        let mut menu = Menu::default();
        let mut cat: Option<u8> = None;
        let mut ext_idx = 0;
        let mut first = true;
        for &o in oct {
            let is_ext = (o & 0x38) == 0x10; // b3=0 b4=1 b5=0
            if is_ext {
                if cat == Some(0x05) {
                    ext_idx += 1;
                    let m = &mut menu.modes;
                    let bit = |v: u8| o & v != 0;
                    if ext_idx == 1 {
                        m.v32bis = bit(0x01);
                        m.v22bis = bit(0x02);
                        m.v17 = bit(0x04);
                        m.v29hdx = bit(0x40);
                        m.v27ter = bit(0x80);
                    } else if ext_idx == 2 {
                        m.v26ter = bit(0x01);
                        m.v26bis = bit(0x02);
                        m.v23 = bit(0x04);
                        m.v23hdx = bit(0x40);
                        m.v21 = bit(0x80);
                    }
                }
                continue;
            }
            if o & 0x10 != 0 {
                return None; // ni catégorie (b4=0) ni extension valide
            }
            let tag = o & 0x0F;
            let opts = o >> 5;
            if first && tag != 0x01 {
                return None; // la 1re catégorie doit être la fonction d'appel
            }
            first = false;
            cat = Some(tag);
            ext_idx = 0;
            match tag {
                0x01 => menu.call_function = opts,
                0x05 => {
                    menu.modes.v34 = o & 0x40 != 0;
                    menu.modes.v34hdx = o & 0x80 != 0;
                }
                0x0A => menu.lapm = opts == 0b001,
                0x0D => menu.access = Some(opts),
                _ => {}
            }
        }
        if first {
            None
        } else {
            Some(menu)
        }
    }

    /// Une séquence complète CM/JM en bits : 10 uns, synchro, octets cadrés.
    pub fn sequence_bits(&self) -> Vec<bool> {
        let mut bits = vec![true; 10];
        bits.extend_from_slice(&SYNC_CM_JM);
        for o in self.to_octets() {
            push_octet(&mut bits, o);
        }
        bits
    }
}

/// Ajoute un octet avec bit de start (0), b0..b7, bit de stop (1).
pub fn push_octet(bits: &mut Vec<bool>, o: u8) {
    bits.push(false);
    for i in 0..8 {
        bits.push((o >> i) & 1 == 1);
    }
    bits.push(true);
}

/// CJ : trois octets à zéro avec start/stop.
pub fn cj_bits() -> Vec<bool> {
    let mut b = Vec::new();
    for _ in 0..3 {
        push_octet(&mut b, 0);
    }
    b
}

/// Événement du décodeur V.8.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum V8Rx {
    /// Une séquence CM/JM a été reçue (pas encore confirmée)
    Sequence(Vec<u8>),
    /// Deux séquences identiques consécutives : menu validé
    Confirmed(Menu),
    /// Signal CJ reçu (3 octets nuls)
    Cj,
}

/// Décodeur de flux binaire V.8 (CM, JM, CJ).
#[derive(Clone, Debug, Default)]
pub struct V8Parser {
    /// 32 derniers bits (bit le plus récent en position 0)
    shift: u32,
    nbits: u32,
    in_seq: bool,
    octet_bits: u32,
    octet_val: u16,
    octets: Vec<u8>,
    last_seq: Option<Vec<u8>>,
    confirmed: bool,
}

impl V8Parser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bit: bool) -> Option<V8Rx> {
        self.shift = (self.shift << 1) | bit as u32;
        self.nbits += 1;
        let mut out = None;

        // CJ : (0 00000000 1) × 3 dans les 30 derniers bits
        const CJ_PAT: u32 = 0b0000000001_0000000001_0000000001;
        if self.nbits >= 30 && (self.shift & 0x3FFF_FFFF) == CJ_PAT {
            self.in_seq = false;
            return Some(V8Rx::Cj);
        }

        // Préambule + synchro CM/JM : 1111111111 0000001111 (20 bits)
        const PRE_PAT: u32 = 0b1111111111_0000001111;
        if self.nbits >= 20 && (self.shift & 0xF_FFFF) == PRE_PAT {
            out = self.finish_sequence();
            self.in_seq = true;
            self.octet_bits = 0;
            self.octet_val = 0;
            self.octets.clear();
            return out;
        }

        if self.in_seq {
            self.octet_val = (self.octet_val >> 1) | ((bit as u16) << 9);
            self.octet_bits += 1;
            if self.octet_bits == 10 {
                self.octet_bits = 0;
                let start = self.octet_val & 1;
                let stop = (self.octet_val >> 9) & 1;
                if self.octet_val == 0x3FF {
                    // dix uns : début du préambule suivant → fin de séquence
                    out = self.finish_sequence();
                    self.in_seq = false;
                } else if start == 0 && stop == 1 {
                    self.octets.push(((self.octet_val >> 1) & 0xFF) as u8);
                    if self.octets.len() > 64 {
                        self.in_seq = false;
                        self.octets.clear();
                    }
                } else {
                    // Erreur de cadrage
                    self.in_seq = false;
                    self.octets.clear();
                }
            }
        }
        out
    }

    fn finish_sequence(&mut self) -> Option<V8Rx> {
        if !self.in_seq || self.octets.is_empty() {
            return None;
        }
        let seq = std::mem::take(&mut self.octets);
        if self.last_seq.as_ref() == Some(&seq) && !self.confirmed {
            if let Some(menu) = Menu::from_octets(&seq) {
                self.confirmed = true;
                self.last_seq = Some(seq);
                return Some(V8Rx::Confirmed(menu));
            }
        }
        self.last_seq = Some(seq.clone());
        Some(V8Rx::Sequence(seq))
    }
}

/// Générateur ANSam : 2100 Hz, AM 15 Hz à ±20 %, inversion de phase toutes
/// les 450 ms (clause 7.2/V.8).
#[derive(Clone, Debug)]
pub struct AnsamGen {
    n: u64,
    phase: f64,
    amp: f32,
}

impl AnsamGen {
    pub fn new() -> Self {
        // Puissance moyenne = NOMINAL : A²/2·(1+0.2²/2) = rms²
        let amp = NOMINAL_RMS * (2.0f32 / 1.02).sqrt();
        Self { n: 0, phase: 0.0, amp }
    }

    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        let fs = SAMPLE_RATE as f64;
        let t = self.n as f64 / fs;
        // Inversion de phase toutes les 450 ms (3600 échantillons)
        if self.n > 0 && self.n % 3600 == 0 {
            self.phase += PI;
        }
        let env = 1.0 + 0.2 * (TAU * 15.0 * t).sin();
        let v = (self.phase + TAU * 2100.0 * t).cos() * env;
        self.n += 1;
        self.amp * v as f32
    }
}

impl Default for AnsamGen {
    fn default() -> Self {
        Self::new()
    }
}

/// Détecteur ANSam : ton 2100 Hz dominant + modulation d'amplitude à 15 Hz
/// (pour le distinguer de ANS simple, cf. note 1 de 7.2/V.8).
#[derive(Clone, Debug)]
pub struct AnsamDetector {
    tone: ToneDetector,
    total: PowerMeter,
    /// Enveloppe décimée à 200 Hz (une valeur / 40 échantillons)
    env: Vec<f32>,
    env_avg: Boxcar,
    count: u32,
    /// Durée de présence continue du ton (échantillons)
    present: u32,
    pub reversals: u32,
    detected: bool,
}

impl AnsamDetector {
    pub fn new() -> Self {
        Self {
            tone: ToneDetector::new(2100.0, 40),
            total: PowerMeter::new(40),
            env: Vec::with_capacity(64),
            env_avg: Boxcar::new(1),
            count: 0,
            present: 0,
            reversals: 0,
            detected: false,
        }
    }

    /// Renvoie `true` au moment où ANSam est reconnu.
    pub fn push(&mut self, x: f32) -> bool {
        if self.tone.push(x).is_some() {
            self.reversals += 1;
        }
        let p_tot = self.total.push(x);
        let a = self.tone.amplitude();
        // Le ton porte l'essentiel de la puissance (a²/2 ≈ p_tot) et dépasse -43 dBm0 env.
        let dominant = a * a * 0.5 > 0.6 * p_tot && a > 0.01;
        if dominant {
            self.present += 1;
        } else {
            self.present = 0;
            self.env.clear();
        }
        self.count += 1;
        if self.count % 40 == 0 && dominant {
            self.env.push(self.env_avg.push(a));
            if self.env.len() > 60 {
                self.env.remove(0);
            }
        }
        // Après ≥ 300 ms de ton : on vérifie la modulation à 15 Hz sur 200 ms
        if !self.detected && self.present > 2400 && self.env.len() >= 40 {
            let depth = am_depth(&self.env[self.env.len() - 40..], 15.0, 200.0);
            if depth > 0.1 {
                self.detected = true;
                return true;
            }
        }
        false
    }

    pub fn detected(&self) -> bool {
        self.detected
    }

    /// Le ton 2100 Hz est-il toujours présent ?
    pub fn tone_present(&self) -> bool {
        self.present > 0
    }
}

impl Default for AnsamDetector {
    fn default() -> Self {
        Self::new()
    }
}

/// Profondeur de modulation d'une enveloppe échantillonnée à `fs` Hz,
/// à la fréquence `f` (DFT ponctuelle). Les inversions de phase créent des
/// creux brefs : on les écrête en remplaçant les valeurs < 50 % de la médiane.
fn am_depth(env: &[f32], f: f32, fs: f32) -> f32 {
    let mut sorted: Vec<f32> = env.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = sorted[sorted.len() / 2];
    if med <= 0.0 {
        return 0.0;
    }
    let (mut re, mut im) = (0.0f32, 0.0f32);
    for (i, &e) in env.iter().enumerate() {
        let e = if e < 0.5 * med { med } else { e };
        let w = std::f32::consts::TAU * f * i as f32 / fs;
        re += (e - med) * w.cos();
        im += (e - med) * w.sin();
    }
    let amp = 2.0 * (re * re + im * im).sqrt() / env.len() as f32;
    amp / med
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_roundtrip_and_known_octets() {
        let m = Menu::ours();
        let o = m.to_octets();
        assert_eq!(o[0], 0xC1, "callf0 data");
        assert_eq!(o[1], 0x45, "modn0 V.34 duplex");
        assert_eq!(Menu::from_octets(&o), Some(m));
    }

    #[test]
    fn no_hdlc_flag_in_bitstream() {
        let bits = Menu::ours().sequence_bits();
        let s: String = bits.iter().map(|&b| if b { '1' } else { '0' }).collect();
        assert!(!s.contains("01111110"));
    }

    #[test]
    fn parser_confirms_two_identical_sequences_and_cj() {
        let m = Menu::ours();
        let mut p = V8Parser::new();
        let mut bits = Vec::new();
        for _ in 0..3 {
            bits.extend(m.sequence_bits());
        }
        bits.extend(cj_bits());
        let mut confirmed = false;
        let mut cj = false;
        for b in bits {
            match p.push(b) {
                Some(V8Rx::Confirmed(got)) => {
                    assert_eq!(got, m);
                    confirmed = true;
                }
                Some(V8Rx::Cj) => cj = true,
                _ => {}
            }
        }
        assert!(confirmed && cj);
    }

    #[test]
    fn ansam_is_detected() {
        let mut g = AnsamGen::new();
        let mut d = AnsamDetector::new();
        let mut at = None;
        for n in 0..16000 {
            if d.push(g.next_sample()) && at.is_none() {
                at = Some(n);
            }
        }
        let at = at.expect("ANSam non détecté");
        assert!(at < 8000, "détection trop lente : {at}");
        assert!(d.reversals >= 1);
    }

    #[test]
    fn pure_2100_is_not_ansam() {
        let mut o = crate::dsp::osc::Osc::new(2100.0);
        let mut d = AnsamDetector::new();
        for _ in 0..16000 {
            assert!(!d.push(0.3 * o.next_cos()));
        }
    }
}
