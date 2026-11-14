//! Trame de données V.90 (§5.4) : 6 symboles PCM portent D = K + S bits.
//!
//! - K bits passent par le codeur à modules : un entier R0 < 2^K est écrit
//!   en base mixte (M0, …, M5) ; le chiffre Ki choisit le niveau d'étiquette
//!   Ki dans l'ensemble Ci (étiquette 0 = le plus grand Ucode) ;
//! - S bits donnent les signes. Ici sans mise en forme spectrale (Sr = 0,
//!   S = 6) : codage différentiel $0 = s0 ⊕ $5(trame précédente),
//!   $i = si ⊕ $(i-1) (§5.4.5.1). Signe 1 = tension positive.
//!
//! L'émetteur est le modem numérique (le FAI) ; le décodeur est celui du
//! modem analogique (l'abonné). Les bits sont brouillés par le polynôme GPC
//! (§5.3) des deux côtés.

use super::codes::{self, Law};
use crate::train::Scrambler;

/// Nombre de symboles par trame.
pub const FRAME: usize = 6;
/// Symboles PCM par seconde.
pub const BAUD: u32 = 8000;

/// Paramètres de correspondance (§5.4.1), choisis par le modem analogique.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mapping {
    pub law: Law,
    /// Ensembles de niveaux de chaque intervalle de trame, en Ucodes
    /// rangés du plus grand au plus petit (étiquette 0 = le plus grand)
    pub sets: [Vec<u8>; FRAME],
    /// Bits du codeur à modules par trame
    pub k: u32,
    /// Bits de signe utiles par trame (6 sans mise en forme spectrale)
    pub s: u32,
}

impl Mapping {
    /// Même ensemble pour les six intervalles.
    pub fn uniform(law: Law, set: &[u8], k: u32) -> Mapping {
        let mut v = set.to_vec();
        v.sort_unstable_by(|a, b| b.cmp(a));
        v.dedup();
        Mapping { law, sets: std::array::from_fn(|_| v.clone()), k, s: 6 }
    }

    /// Bits par trame.
    pub fn bits_per_frame(&self) -> u32 {
        self.k + self.s
    }

    /// Débit (bit/s) : D × 8000 / 6.
    pub fn rate(&self) -> u32 {
        rate_for(self.k, self.s)
    }

    /// Les modules Mi permettent-ils de coder K bits (∏ Mi ≥ 2^K) ?
    pub fn is_valid(&self) -> bool {
        if self.k > 39 || self.sets.iter().any(|s| s.is_empty()) {
            return false;
        }
        let prod: u128 = self.sets.iter().map(|s| s.len() as u128).product();
        prod >= 1u128 << self.k
    }
}

/// Débit (bit/s) pour K et S, arrondi à l'entier comme dans les tableaux.
pub fn rate_for(k: u32, s: u32) -> u32 {
    ((k + s) * BAUD + 3) / 6
}

/// Symbole PCM : Ucode et signe (vrai = positif).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub ucode: u8,
    pub positive: bool,
}

impl Symbol {
    /// Valeur linéaire signée (16 bits).
    pub fn linear(&self, law: Law) -> i32 {
        let v = codes::linear(law, self.ucode);
        if self.positive {
            v
        } else {
            -v
        }
    }
}

/// Codeur du modem numérique : bits utilisateur → symboles PCM.
#[derive(Clone, Debug)]
pub struct Encoder {
    pub map: Mapping,
    scrambler: Scrambler,
    /// $5 de la trame précédente
    last_sign: bool,
}

impl Encoder {
    pub fn new(map: Mapping) -> Encoder {
        Encoder { map, scrambler: Scrambler::new(true), last_sign: false }
    }

    /// Code une trame. `bits` doit contenir D = K + S bits (non brouillés).
    pub fn frame(&mut self, bits: &[bool]) -> [Symbol; FRAME] {
        let d = self.map.bits_per_frame() as usize;
        assert_eq!(bits.len(), d, "trame V.90 : {d} bits attendus");
        let sc: Vec<bool> = bits.iter().map(|&b| self.scrambler.scramble(b)).collect();
        let s = self.map.s as usize;
        let (sign_bits, mod_bits) = sc.split_at(s);
        // codeur à modules (§5.4.3)
        let mut r: u64 = mod_bits.iter().enumerate().map(|(i, &b)| (b as u64) << i).sum();
        let mut out = [Symbol { ucode: 0, positive: true }; FRAME];
        for (i, o) in out.iter_mut().enumerate() {
            let set = &self.map.sets[i];
            let m = set.len() as u64;
            let ki = (r % m) as usize;
            r /= m;
            o.ucode = set[ki];
        }
        // signes (§5.4.5.1, Sr = 0)
        let mut prev = self.last_sign;
        for (i, o) in out.iter_mut().enumerate() {
            let si = sign_bits.get(i).copied().unwrap_or(false);
            let sign = si ^ prev;
            o.positive = sign;
            prev = sign;
        }
        self.last_sign = prev;
        out
    }
}

/// Décodeur du modem analogique : symboles décidés → bits utilisateur.
#[derive(Clone, Debug)]
pub struct Decoder {
    pub map: Mapping,
    descrambler: Scrambler,
    last_sign: bool,
    /// Trames dont l'entier reconstitué dépassait 2^K (erreur certaine)
    pub invalid: u64,
}

impl Decoder {
    pub fn new(map: Mapping) -> Decoder {
        Decoder { map, descrambler: Scrambler::new(true), last_sign: false, invalid: 0 }
    }

    /// Étiquette (position dans l'ensemble) d'un Ucode décidé.
    fn label(&self, i: usize, ucode: u8) -> usize {
        self.map.sets[i].iter().position(|&u| u == ucode).unwrap_or(0)
    }

    /// Décode une trame de 6 symboles décidés en D bits.
    pub fn frame(&mut self, syms: &[Symbol; FRAME]) -> Vec<bool> {
        let s = self.map.s as usize;
        let k = self.map.k;
        let mut bits = Vec::with_capacity((k as usize) + s);
        // signes
        let mut prev = self.last_sign;
        for sym in syms.iter().take(s) {
            bits.push(sym.positive ^ prev);
            prev = sym.positive;
        }
        for sym in syms.iter().skip(s) {
            prev = sym.positive;
        }
        self.last_sign = prev;
        // modules
        let mut r: u128 = 0;
        let mut weight: u128 = 1;
        for (i, sym) in syms.iter().enumerate() {
            r += self.label(i, sym.ucode) as u128 * weight;
            weight *= self.map.sets[i].len() as u128;
        }
        if r >> k != 0 {
            self.invalid += 1;
        }
        for i in 0..k {
            bits.push((r >> i) & 1 == 1);
        }
        bits.into_iter().map(|b| self.descrambler.descramble(b)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(n: usize, seed: &mut u64) -> Vec<bool> {
        (0..n)
            .map(|_| {
                *seed ^= *seed << 13;
                *seed ^= *seed >> 7;
                *seed ^= *seed << 17;
                *seed & 1 == 1
            })
            .collect()
    }

    #[test]
    fn debits_du_tableau_2() {
        assert_eq!(rate_for(15, 6), 28000);
        assert_eq!(rate_for(16, 6), 29333);
        assert_eq!(rate_for(32, 6), 50667);
        assert_eq!(rate_for(36, 6), 56000);
        assert_eq!(rate_for(39, 3), 56000);
    }

    #[test]
    fn aller_retour_sans_canal() {
        // ensembles différents par intervalle, K au plus juste
        let mut map = Mapping::uniform(Law::A, &(20..90).step_by(2).collect::<Vec<u8>>(), 29);
        map.sets[2].truncate(20);
        map.sets[5].truncate(33);
        assert!(map.is_valid());
        let mut enc = Encoder::new(map.clone());
        let mut dec = Decoder::new(map.clone());
        let mut seed = 0x1234_5678_9abc_def0;
        for _ in 0..500 {
            let b = bits(map.bits_per_frame() as usize, &mut seed);
            let syms = enc.frame(&b);
            for (i, s) in syms.iter().enumerate() {
                assert!(map.sets[i].contains(&s.ucode));
            }
            assert_eq!(dec.frame(&syms), b);
        }
        assert_eq!(dec.invalid, 0);
    }

    #[test]
    fn plafond_56k() {
        // 64 niveaux partout : K = 36 → 56 000 bit/s
        let map = Mapping::uniform(Law::Mu, &(0..64).map(|u| u + 64).collect::<Vec<u8>>(), 36);
        assert!(map.is_valid() && map.rate() == 56000);
        let trop = Mapping { k: 37, ..map };
        assert!(!trop.is_valid());
    }
}
