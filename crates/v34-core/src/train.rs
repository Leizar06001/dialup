//! Signaux des phases 3 et 4 (V.34 §10.1.3) : S, S̄, PP, TRN, J, J′, MP, E,
//! ainsi que le brouilleur (clause 7) et la superconstellation (§9.1).

use std::collections::VecDeque;

use crate::api::NOMINAL_RMS;
use crate::dsp::shaping::PassbandMod;
use crate::dsp::Cpx;

// ---------------------------------------------------------------------------
// Superconstellation (Figure 5/V.34)
// ---------------------------------------------------------------------------

/// Points du quart de superconstellation, étiquetés 0..415.
/// Le quart = points de coordonnées impaires (x, y) avec x ≡ y ≡ 1 (mod 4) ;
/// tri par module croissant, à module égal la plus grande partie imaginaire
/// d'abord (§9.1). Les trois rotations de 90° donnent les 1664 points.
pub fn quarter_constellation() -> Vec<(i32, i32)> {
    let mut pts = Vec::new();
    for x in (-47..=49).step_by(4) {
        for y in (-47..=49).step_by(4) {
            pts.push((x, y));
        }
    }
    pts.sort_by_key(|&(x, y)| (x * x + y * y, -y));
    pts.truncate(416);
    pts
}

/// Point d'indice `i` du quart, en coordonnées entières.
pub fn quarter_point(i: usize) -> Cpx {
    thread_local! {
        static Q: Vec<(i32, i32)> = quarter_constellation();
    }
    Q.with(|q| {
        let (x, y) = q[i];
        Cpx::new(x as f32, y as f32)
    })
}

/// Puissance moyenne d'une constellation à `l` points (quart de l/4 points).
pub fn constellation_power(l: usize) -> f32 {
    let n = l / 4;
    (0..n).map(|i| quarter_point(i).norm_sqr()).sum::<f32>() / n as f32
}

// ---------------------------------------------------------------------------
// Brouilleur (clause 7)
// ---------------------------------------------------------------------------

/// Brouilleur auto-synchronisant. Appelant : 1 + x⁻¹⁸ + x⁻²³ ;
/// appelé : 1 + x⁻⁵ + x⁻²³.
#[derive(Clone, Debug)]
pub struct Scrambler {
    reg: u32,
    tap: u32,
}

impl Scrambler {
    /// `call` : true pour le brouilleur de l'émetteur appelant (GPC).
    pub fn new(call: bool) -> Self {
        Self { reg: 0, tap: if call { 18 } else { 5 } }
    }

    pub fn reset(&mut self) {
        self.reg = 0;
    }

    #[inline]
    pub fn scramble(&mut self, b: bool) -> bool {
        let out = b as u32 ^ ((self.reg >> (self.tap - 1)) & 1) ^ ((self.reg >> 22) & 1);
        self.reg = ((self.reg << 1) | out) & 0x7F_FFFF;
        out == 1
    }

    #[inline]
    pub fn descramble(&mut self, b: bool) -> bool {
        let out = b as u32 ^ ((self.reg >> (self.tap - 1)) & 1) ^ ((self.reg >> 22) & 1);
        self.reg = ((self.reg << 1) | b as u32) & 0x7F_FFFF;
        out == 1
    }
}

// ---------------------------------------------------------------------------
// Séquences
// ---------------------------------------------------------------------------

/// Point 0 du quart de superconstellation.
pub fn point0() -> Cpx {
    quarter_point(0)
}

/// S (bar=false) ou S̄ (bar=true) sur `len` symboles (§10.1.3.7).
/// S alterne point0 et point0 tourné de 90° (anti-horaire) et finit sur ce
/// dernier ; S̄ commence par point0 tourné de 180°.
pub fn seq_s(len: usize, bar: bool) -> Vec<Cpx> {
    let p = point0();
    (0..len)
        .map(|i| {
            if !bar {
                // finit sur la rotation 90° : parité choisie selon len
                let rotated = (len - 1 - i) % 2 == 0;
                if rotated { p.rot_ccw(1) } else { p }
            } else if i % 2 == 0 {
                p.rot_ccw(2)
            } else {
                p.rot_ccw(3)
            }
        })
        .collect()
}

/// PP : 288 symboles (6 périodes de 48), §10.1.3.6. Module unité.
pub fn seq_pp() -> Vec<Cpx> {
    (0..288)
        .map(|i| {
            let k = (i / 4) as f32;
            let ii = (i % 4) as f32;
            let ph = if (i / 4) % 3 == 1 { std::f32::consts::PI * (k * ii + 4.0) / 6.0 } else { std::f32::consts::PI * k * ii / 6.0 };
            Cpx::from_polar(1.0, ph)
        })
        .collect()
}

/// Générateur de TRN / J / MP / E : brouilleur + mapping 4 ou 16 points.
#[derive(Clone, Debug)]
pub struct TrainMapper {
    pub scr: Scrambler,
    pub sixteen: bool,
    /// Dernier Z (codage différentiel de J / MP / E)
    z: u32,
}

impl TrainMapper {
    pub fn new(call: bool) -> Self {
        Self { scr: Scrambler::new(call), sixteen: false, z: 0 }
    }

    /// TRN : uns brouillés, mapping NON différentiel (§10.1.3.8).
    /// Le brouilleur doit être remis à zéro au début de TRN.
    pub fn trn_symbol(&mut self) -> Cpx {
        let i1 = self.scr.scramble(true) as u32;
        let i2 = self.scr.scramble(true) as u32;
        let i = 2 * i2 + i1;
        let p = if self.sixteen {
            let q1 = self.scr.scramble(true) as usize;
            let q2 = self.scr.scramble(true) as usize;
            quarter_point(2 * q2 + q1)
        } else {
            point0()
        };
        // Initialise le codeur différentiel sur le dernier symbole de TRN
        self.z = i;
        p.rot_cw(i)
    }

    /// Symbole différentiel à partir de bits d'entrée (J, MP, E) :
    /// 2 bits (4 points) ou 4 bits (16 points) par symbole.
    pub fn diff_symbol(&mut self, bits: &[bool]) -> Cpx {
        let i1 = self.scr.scramble(bits[0]) as u32;
        let i2 = self.scr.scramble(bits[1]) as u32;
        let i = 2 * i2 + i1;
        self.z = (self.z + i) & 3;
        let p = if self.sixteen && bits.len() >= 4 {
            let q1 = self.scr.scramble(bits[2]) as usize;
            let q2 = self.scr.scramble(bits[3]) as usize;
            quarter_point(2 * q2 + q1)
        } else {
            point0()
        };
        p.rot_cw(self.z)
    }

    /// Remet le brouilleur et le codeur différentiel à zéro (CPt du V.90).
    pub fn reset_diff(&mut self) {
        self.scr.reset();
        self.z = 0;
    }

    /// Nombre de bits par symbole pour J/MP/E.
    pub fn bits_per_symbol(&self) -> usize {
        if self.sixteen { 4 } else { 2 }
    }
}

/// Motifs J (Table 18) et J′ (Table 19), bit de gauche émis en premier.
pub fn j_pattern(sixteen: bool) -> [bool; 16] {
    bits16(if sixteen { "0000110110010001" } else { "0000100110010001" })
}

pub fn j_prime_pattern() -> [bool; 16] {
    bits16("1111100110010001")
}

fn bits16(s: &str) -> [bool; 16] {
    let mut b = [false; 16];
    for (i, c) in s.chars().enumerate() {
        b[i] = c == '1';
    }
    b
}

/// J est toujours émis en 4 points (2 bits/symbole) : 8 symboles par motif.
pub fn j_symbols(mapper: &mut TrainMapper, pattern: &[bool; 16]) -> Vec<Cpx> {
    let saved = mapper.sixteen;
    mapper.sixteen = false;
    let v = pattern.chunks(2).map(|c| mapper.diff_symbol(c)).collect();
    mapper.sixteen = saved;
    v
}

// ---------------------------------------------------------------------------
// Séquences MP (Tables 20 / 21)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mp {
    pub type1: bool,
    /// Débit max appelant→appelé, multiples de 2400 (0 = cleardown)
    pub rate_c2a: u8,
    pub rate_a2c: u8,
    pub aux: bool,
    /// 0 = 16 états, 1 = 32, 2 = 64
    pub trellis: u8,
    pub nonlinear: bool,
    pub shaping_expanded: bool,
    pub ack: bool,
    /// Masque des débits supportés : bit 0 = 2400 … bit 13 = 33600
    pub rate_mask: u16,
    pub asym: bool,
    /// Coefficients de précodage (type 1), format 2.14
    pub precoder: [(i16, i16); 3],
}

impl Mp {
    pub fn to_bits(&self) -> Vec<bool> {
        let mut b = vec![true; 17];
        b.push(false); // start
        b.push(self.type1);
        b.push(false);
        push(&mut b, self.rate_c2a as u32, 4);
        push(&mut b, self.rate_a2c as u32, 4);
        b.push(self.aux);
        push(&mut b, self.trellis as u32, 2);
        b.push(self.nonlinear);
        b.push(self.shaping_expanded);
        b.push(self.ack);
        b.push(false); // start (34)
        push(&mut b, self.rate_mask as u32 & 0x3FFF, 15);
        b.push(self.asym);
        if self.type1 {
            for &(re, im) in &self.precoder {
                b.push(false);
                push(&mut b, re as u16 as u32, 16);
                b.push(false);
                push(&mut b, im as u16 as u32, 16);
            }
        }
        b.push(false); // start
        push(&mut b, 0, 16); // réservé
        b.push(false); // start
        // CRC sur tous les bits sauf synchro de trame et bits de start
        let crc_in: Vec<bool> = mp_crc_bits(&b);
        crate::info::push_crc(&mut b, crate::info::crc16(&crc_in));
        if self.type1 {
            b.push(false);
        } else {
            b.extend_from_slice(&[false; 3]);
        }
        b
    }

    /// Position du CRC dans la trame.
    pub fn crc_pos(type1: bool) -> usize {
        if type1 { 171 } else { 69 }
    }

    /// Décode une trame complète (à partir du premier bit de synchro), CRC
    /// dans l'un ou l'autre ordre.
    pub fn from_bits(b: &[bool]) -> Option<Mp> {
        Self::from_bits_crc(b, crate::api::CrcMode::Auto)
    }

    /// Comme [`Mp::from_bits`], en n'acceptant que les ordres de CRC de `mode`.
    pub fn from_bits_crc(b: &[bool], mode: crate::api::CrcMode) -> Option<Mp> {
        if b.len() < 88 || b[..17].iter().any(|&x| !x) || b[17] {
            return None;
        }
        let type1 = b[18];
        let len = if type1 { 188 } else { 88 };
        if b.len() < len {
            return None;
        }
        let crc_pos = Self::crc_pos(type1);
        let crc_in = mp_crc_bits(&b[..crc_pos]);
        crate::info::check_crc(crate::info::crc16(&crc_in), b, crc_pos, mode)?;
        let mut precoder = [(0i16, 0i16); 3];
        if type1 {
            for (k, c) in precoder.iter_mut().enumerate() {
                let o = 52 + 34 * k;
                *c = (get(b, o, 16) as u16 as i16, get(b, o + 17, 16) as u16 as i16);
            }
        }
        Some(Mp {
            type1,
            rate_c2a: get(b, 20, 4) as u8,
            rate_a2c: get(b, 24, 4) as u8,
            aux: b[28],
            trellis: get(b, 29, 2) as u8,
            nonlinear: b[31],
            shaping_expanded: b[32],
            ack: b[33],
            rate_mask: get(b, 35, 15) as u16 & 0x3FFF,
            asym: b[50],
            precoder,
        })
    }

    pub fn len_bits(type1: bool) -> usize {
        if type1 { 188 } else { 88 }
    }
}

/// Bits couverts par le CRC d'une MP : tout sauf la synchro (0..16) et les
/// bits de start (17, 34, 51, 68, …, tous les 17 bits).
fn mp_crc_bits(b: &[bool]) -> Vec<bool> {
    b.iter().enumerate().filter(|(i, _)| *i >= 17 && (*i - 17) % 17 != 0).map(|(_, &v)| v).collect()
}

fn push(b: &mut Vec<bool>, v: u32, n: usize) {
    for i in 0..n {
        b.push((v >> i) & 1 == 1);
    }
}

fn get(b: &[bool], start: usize, n: usize) -> u32 {
    (0..n).fold(0, |a, i| a | ((b[start + i] as u32) << i))
}

/// Débit (bit/s) → bit du masque MP.
pub fn rate_mask_upto(max_rate: u32) -> u16 {
    let n = (max_rate / 2400).min(14) as u16;
    if n == 0 { 0 } else { (1u16 << n) - 1 }
}

// ---------------------------------------------------------------------------
// Émetteur QAM des phases 3/4 (et plus tard données)
// ---------------------------------------------------------------------------

/// Émetteur QAM alimenté par une file de symboles. Quand la file est vide,
/// il appelle la source « au fil de l'eau » (`refill`) ou émet du silence.
pub struct QamTx {
    m: PassbandMod,
    queue: VecDeque<Cpx>,
    /// Facteur d'échelle des points entiers (puissance nominale)
    scale: f32,
    /// Symboles émis depuis la création
    pub sent: u64,
    pub symbol_rate: f64,
}

pub const QAM_ALPHA: f64 = 0.12;
pub const QAM_SPAN: usize = 16;

impl QamTx {
    pub fn new(symbol_rate: f64, carrier_hz: f64) -> Self {
        let mut m = PassbandMod::new(symbol_rate, carrier_hz, QAM_ALPHA, QAM_SPAN);
        m.gain = NOMINAL_RMS * std::f32::consts::SQRT_2;
        // Puissance de référence = constellation 4 points (|point0|² = 2)
        Self { m, queue: VecDeque::new(), scale: 1.0 / 2f32.sqrt(), sent: 0, symbol_rate }
    }

    /// Facteur appliqué aux points entiers ; `avg_power` = puissance moyenne
    /// de la constellation utilisée (pour garder la puissance constante).
    pub fn set_constellation_power(&mut self, avg_power: f32) {
        self.scale = 1.0 / avg_power.sqrt();
    }

    /// Met des symboles en file (coordonnées entières de la constellation
    /// courante). La mise à l'échelle est appliquée ICI, avec la puissance
    /// de constellation en vigueur au moment de la mise en file.
    pub fn push(&mut self, pts: &[Cpx]) {
        let k = self.scale;
        self.queue.extend(pts.iter().map(|p| *p * k));
    }

    /// Symboles en attente d'émission (file).
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// Le filtre a besoin d'un symbole et la file est vide.
    pub fn starving(&self) -> bool {
        self.queue.is_empty() && self.m.needs_symbol()
    }

    pub fn next_sample(&mut self) -> f32 {
        while self.m.needs_symbol() {
            match self.queue.pop_front() {
                Some(p) => {
                    self.m.push_symbol(p);
                    self.sent += 1;
                }
                None => {
                    self.m.push_symbol(Cpx::ZERO);
                }
            }
        }
        self.m.next_sample()
    }

    /// Symboles poussés mais pas encore sortis du filtre.
    pub fn in_flight(&self) -> f64 {
        self.m.pending_symbols()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constellation_matches_figure5() {
        let q = quarter_constellation();
        assert_eq!(q[0], (1, 1));
        assert_eq!(q[1], (-3, 1));
        assert_eq!(q[2], (1, -3));
        assert_eq!(q[3], (-3, -3));
        assert_eq!(q[4], (1, 5));
        assert_eq!(q[5], (5, 1));
        assert_eq!(q[8], (5, 5));
        // Quelques points lisibles sur la figure 5 : ligne y = 1
        let lbl = |p: (i32, i32)| q.iter().position(|&x| x == p).unwrap();
        assert_eq!(lbl((-7, 1)), 9);
        assert_eq!(lbl((-11, 1)), 22);
        assert_eq!(lbl((-11, -3)), 25);
        assert_eq!(lbl((9, -3)), 18);
        assert_eq!(lbl((9, 1)), 16);
        assert_eq!(lbl((13, 1)), 32);
    }

    #[test]
    fn scrambler_roundtrip() {
        for call in [true, false] {
            let mut s = Scrambler::new(call);
            let mut d = Scrambler::new(call);
            let data: Vec<bool> = (0..500).map(|i| (i * 7 + i / 3) % 5 < 2).collect();
            let out: Vec<bool> = data.iter().map(|&b| d.descramble(s.scramble(b))).collect();
            assert_eq!(data, out);
        }
    }

    #[test]
    fn mp_roundtrip() {
        for type1 in [false, true] {
            let mp = Mp { type1, rate_c2a: 14, rate_a2c: 12, aux: false, trellis: 0, nonlinear: false, shaping_expanded: true, ack: true, rate_mask: rate_mask_upto(33600), asym: true, precoder: [(100, -200), (0, 5), (-1, 1)] };
            let b = mp.to_bits();
            assert_eq!(b.len(), Mp::len_bits(type1));
            let mut got = Mp::from_bits(&b).unwrap();
            if !type1 {
                got.precoder = mp.precoder;
            }
            assert_eq!(got, mp);
        }
    }

    #[test]
    fn pp_is_unit_and_periodic() {
        let pp = seq_pp();
        assert_eq!(pp.len(), 288);
        for i in 0..240 {
            assert!((pp[i] - pp[i + 48]).abs() < 1e-4);
        }
    }
}
