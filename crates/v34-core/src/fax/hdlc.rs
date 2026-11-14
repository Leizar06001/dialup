//! Tramage HDLC des trames T.30 (en V.21 à 300 bit/s) : fanions 0x7E,
//! transparence (un 0 après cinq 1), octets émis bit de poids faible en
//! premier, FCS 16 bits CRC-CCITT (le même que [`crate::lapm`]).

use std::collections::VecDeque;

use crate::lapm::fcs16;

const FANION: u8 = 0x7E;
/// Trame T.30 la plus longue : adresse, contrôle, FCF, FIF (≤ 64), FCS
const TRAME_MAX: usize = 72;

/// Émetteur : une file de bits (préambule, trames, fanion de fin).
#[derive(Default)]
pub struct Emetteur {
    bits: VecDeque<bool>,
}

impl Emetteur {
    fn fanion(&mut self) {
        for i in 0..8 {
            self.bits.push_back((FANION >> i) & 1 == 1);
        }
    }

    /// Préambule : `n` fanions (1 s ≈ 38 fanions à 300 bit/s).
    pub fn preambule(&mut self, n: usize) {
        for _ in 0..n {
            self.fanion();
        }
    }

    /// Une trame, suivie de son fanion de fermeture.
    pub fn trame(&mut self, octets: &[u8]) {
        let fcs = fcs16(octets);
        let mut uns = 0;
        for &o in octets.iter().chain([fcs as u8, (fcs >> 8) as u8].iter()) {
            for i in 0..8 {
                let b = (o >> i) & 1 == 1;
                self.bits.push_back(b);
                if b {
                    uns += 1;
                    if uns == 5 {
                        self.bits.push_back(false);
                        uns = 0;
                    }
                } else {
                    uns = 0;
                }
            }
        }
        self.fanion();
    }

    pub fn bit(&mut self) -> Option<bool> {
        self.bits.pop_front()
    }

    pub fn vide(&self) -> bool {
        self.bits.is_empty()
    }
}

/// Récepteur : rend les trames dont le FCS est bon (sans le FCS).
#[derive(Default)]
pub struct Recepteur {
    uns: u32,
    bits: Vec<bool>,
    chasse: bool,
    /// Des fanions ont été vus récemment (préambule en cours)
    pub fanions: u32,
}

impl Recepteur {
    pub fn new() -> Recepteur {
        Recepteur { chasse: true, ..Default::default() }
    }

    pub fn bit(&mut self, b: bool) -> Option<Vec<u8>> {
        if b {
            self.uns += 1;
            if self.uns >= 7 {
                // abandon
                self.chasse = true;
                self.bits.clear();
            } else if !self.chasse {
                self.bits.push(true);
            }
            return None;
        }
        let uns = self.uns;
        self.uns = 0;
        if uns == 6 {
            // fanion : ferme la trame en cours
            self.fanions += 1;
            let bits = std::mem::take(&mut self.bits);
            let fin = bits.len().saturating_sub(7); // retire « 0111111 », le début du fanion
            self.chasse = false;
            return trame(&bits[..fin]);
        }
        if uns == 5 {
            // bit de transparence : ignoré
            return None;
        }
        if !self.chasse {
            self.bits.push(false);
            if self.bits.len() > TRAME_MAX * 8 + 8 {
                self.chasse = true;
                self.bits.clear();
            }
        }
        None
    }
}

fn trame(bits: &[bool]) -> Option<Vec<u8>> {
    if bits.len() < 4 * 8 || bits.len() % 8 != 0 {
        return None;
    }
    let octets: Vec<u8> = bits.chunks(8).map(|c| c.iter().enumerate().fold(0u8, |o, (i, &b)| o | ((b as u8) << i))).collect();
    let (corps, fcs) = octets.split_at(octets.len() - 2);
    let attendu = fcs16(corps);
    (fcs == [attendu as u8, (attendu >> 8) as u8]).then(|| corps.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aller_retour() {
        let mut e = Emetteur::default();
        e.preambule(5);
        let t1 = vec![0xFF, 0x03, 0x40, b' ', b'1', b'2'];
        let t2 = vec![0xFF, 0x13, 0x80, 0x00, 0xFF, 0x1F, 0x3E]; // beaucoup de 1 : transparence
        e.trame(&t1);
        e.trame(&t2);
        let mut r = Recepteur::new();
        let mut recues = Vec::new();
        // un peu de bruit avant
        for b in [true, false, false, true, true, true, true, true, true, true, true, false] {
            r.bit(b);
        }
        while let Some(b) = e.bit() {
            if let Some(t) = r.bit(b) {
                recues.push(t);
            }
        }
        assert_eq!(recues, vec![t1, t2]);
    }

    #[test]
    fn fcs_faux_rejete() {
        let mut e = Emetteur::default();
        e.preambule(2);
        e.trame(&[0xFF, 0x13, 0x84]);
        let mut bits: Vec<bool> = std::iter::from_fn(|| e.bit()).collect();
        bits[16 + 12] ^= true;
        let mut r = Recepteur::new();
        assert!(bits.into_iter().all(|b| r.bit(b).is_none()));
    }
}
