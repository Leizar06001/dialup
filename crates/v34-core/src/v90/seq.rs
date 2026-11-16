//! Séquences de bits de la poignée de main V.90 : Ja (descripteur du DIL),
//! Jd, CP et MP.
//!
//! Toutes ont la même forme : 17 uns (synchro), puis des blocs de 16 bits
//! précédés chacun d'un bit de start à 0, puis le CRC (V.34 §10.1.2.3.2,
//! calculé sur tout sauf la synchro et les bits de start), lui aussi
//! précédé d'un start, puis des bits de remplissage.

use crate::api::CrcMode;
use crate::info::{check_crc, crc16, push_crc, swap_crc};

/// Synchro de trame.
const SYNC: usize = 17;

fn push(b: &mut Vec<bool>, v: u32, n: usize) {
    for i in 0..n {
        b.push((v >> i) & 1 == 1);
    }
}

fn get(b: &[bool], start: usize, n: usize) -> u32 {
    (0..n).fold(0, |a, i| a | ((b[start + i] as u32) << i))
}

/// Encadre une charge utile (longueur multiple de 16) : synchro, blocs,
/// CRC. Sans remplissage final.
pub fn frame(payload: &[bool]) -> Vec<bool> {
    frame_crc(payload, false)
}

/// Comme [`frame`], CRC dans l'ancien ordre si `legacy` (voir [`CrcMode`]).
pub fn frame_crc(payload: &[bool], legacy: bool) -> Vec<bool> {
    assert_eq!(payload.len() % 16, 0, "charge utile par blocs de 16 bits");
    let mut b = vec![true; SYNC];
    for chunk in payload.chunks(16) {
        b.push(false);
        b.extend_from_slice(chunk);
    }
    b.push(false);
    let pos = b.len();
    push_crc(&mut b, crc16(payload));
    if legacy {
        swap_crc(&mut b, pos);
    }
    b
}

/// Relit une trame de `chunks` blocs (sans compter le CRC) au début de
/// `b` ; renvoie la charge utile si synchro, starts et CRC sont bons.
pub fn unframe(b: &[bool], chunks: usize) -> Option<Vec<bool>> {
    unframe_crc(b, chunks, CrcMode::Auto)
}

/// Comme [`unframe`], en n'acceptant que les ordres de CRC de `mode`.
pub fn unframe_crc(b: &[bool], chunks: usize, mode: CrcMode) -> Option<Vec<bool>> {
    let len = SYNC + 17 * (chunks + 1);
    if b.len() < len || b[..SYNC].iter().any(|&x| !x) {
        return None;
    }
    let mut payload = Vec::with_capacity(16 * chunks);
    for c in 0..chunks {
        let o = SYNC + 17 * c;
        if b[o] {
            return None;
        }
        payload.extend_from_slice(&b[o + 1..o + 17]);
    }
    let o = SYNC + 17 * chunks;
    if b[o] {
        return None;
    }
    check_crc(crc16(&payload), b, o + 1, mode)?;
    Some(payload)
}

/// Longueur d'une trame de `chunks` blocs (synchro et CRC compris).
pub fn framed_len(chunks: usize) -> usize {
    SYNC + 17 * (chunks + 1)
}

/// Nombre de bits D d'une trame PCM (≡ débit / 1333,33) → bit du masque
/// de débits de Jd (28 000 à 56 000 bit/s).
fn d_range() -> std::ops::RangeInclusive<u32> {
    21..=42
}

// ---------------------------------------------------------------------------
// Jd (tableau 13) : ce que le modem numérique sait émettre
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Jd {
    /// Débits descendants possibles, en bits par trame D (21 = 28 000 …
    /// 42 = 56 000) : bit `D - 21`
    pub rates: u32,
    /// CP, E et SCR en 16 points pendant l'apprentissage / la renégociation
    pub cp16_train: bool,
    pub cp16_reneg: bool,
    /// Anticipation maximale de la mise en forme spectrale (1 à 3)
    pub lookahead: u8,
}

impl Jd {
    /// Tous les débits jusqu'à `max` bit/s.
    pub fn upto(max_rate: u32) -> Jd {
        let mut rates = 0;
        for d in d_range() {
            if super::frame::rate_for(d - 6, 6) <= max_rate {
                rates |= 1 << (d - 21);
            }
        }
        Jd { rates, cp16_train: false, cp16_reneg: false, lookahead: 1 }
    }

    /// Plus grand D autorisé ≤ `d`.
    pub fn best_d(&self, d: u32) -> Option<u32> {
        d_range().rev().find(|&x| x <= d && self.rates & (1 << (x - 21)) != 0)
    }

    pub fn to_bits(&self) -> Vec<bool> {
        self.to_bits_crc(false)
    }

    /// Comme `to_bits`, CRC dans l'ancien ordre si `legacy` (voir `CrcMode`).
    pub fn to_bits_crc(&self, legacy: bool) -> Vec<bool> {
        let mut p = Vec::new();
        push(&mut p, self.rates & 0xFFFF, 16); // bits 18:33 = 28 000 … 48 000
        push(&mut p, self.rates >> 16, 6); // bits 35:40 = 49 333 … 56 000
        push(&mut p, 0, 6); // 41:46 réservés
        p.push(self.cp16_train);
        p.push(self.cp16_reneg);
        push(&mut p, self.lookahead as u32, 2);
        let mut b = frame_crc(&p, legacy);
        b.extend_from_slice(&[false; 4]); // remplissage 68:71
        b
    }

    pub fn from_bits(b: &[bool]) -> Option<Jd> {
        Self::from_bits_crc(b, CrcMode::Auto)
    }

    /// Comme `from_bits`, en n'acceptant que les ordres de CRC de `mode`.
    pub fn from_bits_crc(b: &[bool], mode: CrcMode) -> Option<Jd> {
        let p = unframe_crc(b, 2, mode)?;
        Some(Jd { rates: get(&p, 0, 16) | get(&p, 16, 6) << 16, cp16_train: p[28], cp16_reneg: p[29], lookahead: get(&p, 30, 2) as u8 })
    }

    pub const LEN: usize = 72;
}

// ---------------------------------------------------------------------------
// Ja : descripteur du DIL (tableau 12)
// ---------------------------------------------------------------------------

/// Ce que le modem analogique demande au numérique d'émettre comme DIL
/// (§8.4.1) : N segments, chacun fait de symboles d'apprentissage (un Ucode
/// par segment) et de symboles de référence, selon les motifs SP (signes)
/// et TP (apprentissage / référence).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DilDescriptor {
    /// Motif de signes (bit 1 = positif), longueur 1 à 128
    pub sp: Vec<bool>,
    /// Motif apprentissage (1) / référence (0), longueur 1 à 128
    pub tp: Vec<bool>,
    /// Longueur des segments de chaque Uchord : (H + 1) × 6 symboles
    pub h: [u8; 8],
    /// Symbole de référence de chaque Uchord
    pub refs: [u8; 8],
    /// Ucode d'apprentissage de chaque segment (N = longueur, ≤ 255)
    pub train: Vec<u8>,
}

impl DilDescriptor {
    /// Longueur (symboles) du segment n.
    pub fn segment_len(&self, n: usize) -> usize {
        let c = (self.train[n] as usize >> 4).min(7);
        (self.h[c] as usize + 1) * 6
    }

    /// Symbole i du segment n : (Ucode, signe positif).
    pub fn symbol(&self, n: usize, i: usize) -> (u8, bool) {
        let c = (self.train[n] as usize >> 4).min(7);
        let positive = self.sp[i % self.sp.len()];
        let training = self.tp[i % self.tp.len()];
        (if training { self.train[n] } else { self.refs[c] }, positive)
    }

    /// Longueur totale d'un passage (symboles).
    pub fn total_len(&self) -> usize {
        (0..self.train.len()).map(|n| self.segment_len(n)).sum()
    }

    pub fn to_bits(&self) -> Vec<bool> {
        self.to_bits_crc(false)
    }

    /// Comme `to_bits`, CRC dans l'ancien ordre si `legacy` (voir `CrcMode`).
    pub fn to_bits_crc(&self, legacy: bool) -> Vec<bool> {
        let n = self.train.len();
        let mut p = Vec::new();
        push(&mut p, n as u32, 8);
        push(&mut p, 0, 8);
        push(&mut p, self.sp.len() as u32 - 1, 7);
        p.push(false);
        push(&mut p, self.tp.len() as u32 - 1, 7);
        p.push(false);
        for pat in [&self.sp, &self.tp] {
            let mut v = pat.clone();
            v.resize(pat.len().div_ceil(16) * 16, false);
            p.extend(v);
        }
        for v in self.h.iter().chain(self.refs.iter()) {
            push(&mut p, *v as u32 & 0x7F, 7);
            p.push(false);
        }
        for u in &self.train {
            push(&mut p, *u as u32 & 0x7F, 7);
            p.push(false);
        }
        if n % 2 == 1 {
            push(&mut p, 0, 8);
        }
        let mut b = frame_crc(&p, legacy);
        b.push(false);
        if b.len() % 2 == 1 {
            b.push(false);
        }
        b
    }

    /// Relit un descripteur au début de `b`.
    pub fn from_bits(b: &[bool]) -> Option<DilDescriptor> {
        Self::from_bits_crc(b, CrcMode::Auto)
    }

    /// Comme `from_bits`, en n'acceptant que les ordres de CRC de `mode`.
    pub fn from_bits_crc(b: &[bool], mode: CrcMode) -> Option<DilDescriptor> {
        // l'en-tête (2 blocs) donne les longueurs, donc le nombre de blocs
        if b.len() < framed_len(2) || b[..SYNC].iter().any(|&x| !x) || b[SYNC] || b[SYNC + 17] {
            return None;
        }
        let h0 = &b[SYNC + 1..SYNC + 17];
        let h1 = &b[SYNC + 18..SYNC + 34];
        let n = get(h0, 0, 8) as usize;
        let lsp = get(h1, 0, 7) as usize + 1;
        let ltp = get(h1, 8, 7) as usize + 1;
        let chunks = 2 + lsp.div_ceil(16) + ltp.div_ceil(16) + 4 + 4 + n.div_ceil(2);
        let p = unframe_crc(b, chunks, mode)?;
        let mut o = 32;
        let sp = p[o..o + lsp].to_vec();
        o += lsp.div_ceil(16) * 16;
        let tp = p[o..o + ltp].to_vec();
        o += ltp.div_ceil(16) * 16;
        let mut h = [0u8; 8];
        let mut refs = [0u8; 8];
        for v in h.iter_mut().chain(refs.iter_mut()) {
            *v = get(&p, o, 7) as u8;
            o += 8;
        }
        let train = (0..n).map(|k| get(&p, o + 8 * k, 7) as u8).collect();
        Some(DilDescriptor { sp, tp, h, refs, train })
    }

    /// Longueur en bits de la séquence (pour la détection).
    pub fn len_bits(&self) -> usize {
        self.to_bits().len()
    }
}

// ---------------------------------------------------------------------------
// CP (tableau 14) : les constellations choisies par le modem analogique
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cp {
    /// Faux : CPt (paramètres de la phase 4) ; vrai : CP (mode données)
    pub data: bool,
    /// Bits par trame D (débit = D × 8000 / 6) ; 0 = raccrochage
    pub d: u32,
    pub silence: bool,
    /// Redondance de la mise en forme spectrale (0 : aucune)
    pub sr: u8,
    pub ack: bool,
    pub alaw: bool,
    /// Débits montants possibles : bit 0 = 4800 … bit 12 = 33 600
    pub up_rates: u16,
    pub lookahead: u8,
    /// Rapport des valeurs efficaces de TRN1d (Q3.13) ; 1,0 ici
    pub rms_ratio: u16,
    pub shaping: [i8; 4],
    /// Index de constellation de chaque intervalle de trame
    pub index: [u8; 6],
    /// Constellations : masque de 128 bits (bit u = Ucode u présent)
    pub consts: Vec<u128>,
}

impl Cp {
    /// Champ drn du débit (§8.5.2) : CP compte à partir de D = 20, CPt de 8.
    fn drn(&self) -> u32 {
        if self.d == 0 {
            0
        } else if self.data {
            self.d - 20
        } else {
            self.d - 8
        }
    }

    pub fn to_bits(&self) -> Vec<bool> {
        self.to_bits_crc(false)
    }

    /// Comme `to_bits`, CRC dans l'ancien ordre si `legacy` (voir `CrcMode`).
    pub fn to_bits_crc(&self, legacy: bool) -> Vec<bool> {
        let mut p = Vec::new();
        p.push(false);
        p.push(self.data);
        push(&mut p, self.drn(), 5);
        push(&mut p, 0, 5);
        p.push(self.silence);
        push(&mut p, self.sr as u32, 2);
        p.push(self.ack);
        p.push(self.alaw);
        push(&mut p, self.up_rates as u32, 13);
        push(&mut p, self.lookahead as u32, 2);
        push(&mut p, self.rms_ratio as u32, 16);
        for v in self.shaping {
            push(&mut p, v as u8 as u32, 8);
        }
        for &i in &self.index {
            push(&mut p, i as u32, 4);
        }
        p.push(false); // les constellations du codec sont les mêmes
        push(&mut p, 0, 7);
        for &c in &self.consts {
            for u in 0..128 {
                p.push((c >> u) & 1 == 1);
            }
        }
        let mut b = frame_crc(&p, legacy);
        b.extend_from_slice(&[false; 3]);
        b
    }

    /// Relit un CP au début de `b`, d'après le nombre de constellations
    /// annoncé.
    pub fn from_bits(b: &[bool]) -> Option<Cp> {
        Self::from_bits_crc(b, CrcMode::Auto)
    }

    /// Comme `from_bits`, en n'acceptant que les ordres de CRC de `mode`.
    pub fn from_bits_crc(b: &[bool], mode: CrcMode) -> Option<Cp> {
        if b.len() < framed_len(7) || b[..SYNC].iter().any(|&x| !x) {
            return None;
        }
        // index des intervalles (bits 103:127) : charge utile 80..103
        let at = |q: usize| SYNC + 17 * (q / 16) + 1 + q % 16;
        let max_index = (0..6).map(|k| get(b, at(80 + 4 * k), 4)).max()?;
        if max_index > 5 {
            return None;
        }
        let chunks = 7 + 8 * (max_index as usize + 1);
        let p = unframe_crc(b, chunks, mode)?;
        let data = p[1];
        let drn = get(&p, 2, 5);
        let d = if drn == 0 { 0 } else if data { drn + 20 } else { drn + 8 };
        let mut index = [0u8; 6];
        for (k, i) in index.iter_mut().enumerate() {
            *i = get(&p, 80 + 4 * k, 4) as u8;
        }
        let consts = (0..=max_index as usize)
            .map(|c| {
                let o = 112 + 128 * c;
                (0..128).fold(0u128, |m, u| m | (p[o + u] as u128) << u)
            })
            .collect();
        Some(Cp {
            data,
            d,
            silence: p[12],
            sr: get(&p, 13, 2) as u8,
            ack: p[15],
            alaw: p[16],
            up_rates: get(&p, 17, 13) as u16,
            lookahead: get(&p, 30, 2) as u8,
            rms_ratio: get(&p, 32, 16) as u16,
            shaping: [get(&p, 48, 8) as u8 as i8, get(&p, 56, 8) as u8 as i8, get(&p, 64, 8) as u8 as i8, get(&p, 72, 8) as u8 as i8],
            index,
            consts,
        })
    }

    /// Construit le CP d'un jeu de constellations (sans mise en forme).
    pub fn for_mapping(map: &super::frame::Mapping, data: bool, up_rates: u16) -> Cp {
        let mut consts: Vec<u128> = Vec::new();
        let mut index = [0u8; 6];
        for (i, set) in map.sets.iter().enumerate() {
            let mask = set.iter().fold(0u128, |m, &u| m | 1u128 << u);
            let k = consts.iter().position(|&c| c == mask).unwrap_or_else(|| {
                consts.push(mask);
                consts.len() - 1
            });
            index[i] = k as u8;
        }
        Cp {
            data,
            d: map.bits_per_frame(),
            silence: false,
            sr: 0,
            ack: false,
            alaw: map.law == super::Law::A,
            up_rates,
            lookahead: 0,
            rms_ratio: 1 << 13,
            shaping: [0; 4],
            index,
            consts,
        }
    }

    /// Jeu de constellations décrit (S = 6, sans mise en forme).
    pub fn mapping(&self, law: super::Law) -> super::frame::Mapping {
        let sets = std::array::from_fn(|i| {
            let mask = self.consts.get(self.index[i] as usize).copied().unwrap_or(0);
            (0..128u8).rev().filter(|&u| (mask >> u) & 1 == 1).collect()
        });
        let s = 6 - self.sr as u32;
        super::frame::Mapping { law, sets, k: self.d.saturating_sub(s), s }
    }

    /// Longueur en bits (pour la détection).
    pub fn len_bits(&self) -> usize {
        self.to_bits().len()
    }
}

// ---------------------------------------------------------------------------
// MP du V.90 (tableau 16), type 0 : envoyée par le modem numérique
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mp90 {
    /// Débit montant max, en multiples de 2400 (0 = raccrochage)
    pub up_drn: u8,
    /// Treillis exigé de l'émetteur montant (0 : 16 états)
    pub trellis: u8,
    pub nonlinear: bool,
    pub shaping_expanded: bool,
    pub ack: bool,
    /// Débits montants possibles : bit 0 = 4800 … bit 12 = 33 600
    pub up_rates: u16,
}

impl Mp90 {
    pub fn to_bits(&self) -> Vec<bool> {
        self.to_bits_crc(false)
    }

    /// Comme `to_bits`, CRC dans l'ancien ordre si `legacy` (voir `CrcMode`).
    pub fn to_bits_crc(&self, legacy: bool) -> Vec<bool> {
        let mut p = Vec::new();
        p.push(false); // type 0
        push(&mut p, 0, 5);
        push(&mut p, self.up_drn as u32, 4);
        p.push(false);
        push(&mut p, self.trellis as u32, 2);
        p.push(self.nonlinear);
        p.push(self.shaping_expanded);
        p.push(self.ack);
        p.push(false);
        push(&mut p, self.up_rates as u32, 13);
        push(&mut p, 0, 2);
        push(&mut p, 0, 16); // réservés 52:67
        let mut b = frame_crc(&p, legacy);
        b.push(false); // remplissage 85
        b
    }

    pub fn from_bits(b: &[bool]) -> Option<Mp90> {
        Self::from_bits_crc(b, CrcMode::Auto)
    }

    /// Comme `from_bits`, en n'acceptant que les ordres de CRC de `mode`.
    pub fn from_bits_crc(b: &[bool], mode: CrcMode) -> Option<Mp90> {
        let p = unframe_crc(b, 3, mode)?;
        if p[0] {
            return None; // type 1 : non géré
        }
        Some(Mp90 {
            up_drn: get(&p, 6, 4) as u8,
            trellis: get(&p, 11, 2) as u8,
            nonlinear: p[13],
            shaping_expanded: p[14],
            ack: p[15],
            up_rates: get(&p, 17, 13) as u16,
        })
    }

    pub const LEN: usize = 86;
}

/// Masque des débits montants (bit 0 = 4800 … bit 12 = 33 600) jusqu'à `max`.
pub fn up_mask_upto(max_rate: u32) -> u16 {
    let mut m = 0;
    for k in 0..13u32 {
        if 4800 + 2400 * k <= max_rate {
            m |= 1 << k;
        }
    }
    m
}

/// Cherche, à la fin de `bits`, une séquence complète décodée par `parse`
/// sur `len` bits.
pub fn find_tail<T>(bits: &std::collections::VecDeque<bool>, len: usize, parse: impl Fn(&[bool]) -> Option<T>) -> Option<T> {
    if bits.len() < len {
        return None;
    }
    let w: Vec<bool> = bits.range(bits.len() - len..).copied().collect();
    parse(&w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v90::frame::Mapping;
    use crate::v90::Law;

    #[test]
    fn jd_aller_retour() {
        let jd = Jd::upto(50000);
        let b = jd.to_bits();
        assert_eq!(b.len(), Jd::LEN, "tableau 13 : bits 0..71");
        assert_eq!(Jd::from_bits(&b), Some(jd.clone()));
        assert_eq!(jd.best_d(42), Some(37)); // 49 333 bit/s
        assert!(b[18] && b[33] && !b[34] && b[35] && !b[36], "28 000 à 49 333 oui, 50 667 non");
    }

    #[test]
    fn mp90_aller_retour() {
        let mp = Mp90 { up_drn: 13, trellis: 0, nonlinear: false, shaping_expanded: false, ack: true, up_rates: up_mask_upto(31200) };
        let b = mp.to_bits();
        assert_eq!(b.len(), Mp90::LEN);
        assert_eq!(Mp90::from_bits(&b), Some(mp));
        assert_eq!(get(&b, 24, 4), 13, "bits 24:27 du tableau 16");
    }

    #[test]
    fn cp_aller_retour() {
        let mut map = Mapping::uniform(Law::A, &(40..100).step_by(3).collect::<Vec<u8>>(), 20);
        map.sets[3] = (50..90).step_by(4).rev().collect();
        let mut cp = Cp::for_mapping(&map, true, up_mask_upto(33600));
        cp.ack = true;
        let b = cp.to_bits();
        let got = Cp::from_bits(&b).expect("CP relu");
        assert_eq!(got, cp);
        assert_eq!(got.consts.len(), 2);
        assert_eq!(got.mapping(Law::A).sets, map.sets);
        // CPt : drn compté à partir de 8
        let cpt = Cp { data: false, ..cp };
        assert_eq!(Cp::from_bits(&cpt.to_bits()).unwrap().d, cpt.d);
    }

    #[test]
    fn descripteur_dil_aller_retour() {
        let dil = DilDescriptor {
            sp: vec![true, false, true, true, false],
            tp: (0..20).map(|i| i % 3 != 0).collect(),
            h: [1, 1, 2, 2, 3, 3, 3, 3],
            refs: [100; 8],
            train: (8..128).step_by(8).map(|u| u as u8).collect(),
        };
        let b = dil.to_bits();
        assert_eq!(b.len() % 2, 0);
        assert_eq!(DilDescriptor::from_bits(&b), Some(dil.clone()));
        assert_eq!(dil.segment_len(0), 12);
        assert_eq!(dil.segment_len(14), 24);
    }
}
