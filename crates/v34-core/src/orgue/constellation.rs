//! Constellations : MDP différentielle (mode « joli ») et MAQ rectangulaire
//! (mode « rapide »), codage de Gray, seuils de chargement.

use std::f32::consts::PI;

use super::Mode;
use crate::dsp::Cpx;

/// Bits maximum par harmonique.
pub fn bits_max(mode: Mode) -> u8 {
    match mode {
        Mode::Joli => 5,
        Mode::Rapide => 10,
    }
}

/// Rapport signal/bruit (dB, par harmonique) qu'il faut pour porter `bits`
/// bits avec un taux d'erreur brut de l'ordre de 1e-5, sans marge.
/// Joli : MDP-2^b décidée de façon cohérente puis décodée en différentiel
/// (une erreur de symbole en coûte deux) ; la MDP-32 (±5,6°) a besoin de
/// plus que la théorie : bruit de phase résiduel et interférence la
/// gênent davantage (mesuré). Rapide : MAQ carrée ou
/// rectangulaire (8, 32, 128, 512 points), décision cohérente.
pub fn snr_requis_db(mode: Mode, bits: u8) -> f32 {
    match mode {
        Mode::Joli => [0.0, 10.0, 13.0, 18.5, 24.5, 33.0][bits as usize],
        Mode::Rapide => [0.0, 10.0, 13.0, 17.0, 20.0, 23.5, 26.5, 30.0, 33.0, 36.5, 39.5][bits as usize],
    }
}

/// Code de Gray.
#[inline]
pub fn gray(i: u32) -> u32 {
    i ^ (i >> 1)
}

/// Inverse du code de Gray.
#[inline]
pub fn gray_inverse(g: u32) -> u32 {
    let mut i = g;
    let mut s = g >> 1;
    while s != 0 {
        i ^= s;
        s >>= 1;
    }
    i
}

/// Point MDP d'indice `i` (sur 2^bits).
#[inline]
pub fn mdp(bits: u8, i: u32) -> Cpx {
    Cpx::from_polar(1.0, 2.0 * PI * i as f32 / (1u32 << bits) as f32)
}

/// Indice MDP le plus proche de `z`.
#[inline]
pub fn mdp_decider(bits: u8, z: Cpx) -> u32 {
    let m = 1u32 << bits;
    let a = z.arg().rem_euclid(2.0 * PI);
    ((a * m as f32 / (2.0 * PI)).round() as u32) % m
}

fn axes(bits: u8) -> (u8, u8) {
    (bits.div_ceil(2), bits / 2)
}

fn echelle(bits: u8) -> f32 {
    let (bi, bq) = axes(bits);
    let li = (1u32 << bi) as f32;
    let lq = (1u32 << bq) as f32;
    let p = (li * li - 1.0) / 3.0 + (lq * lq - 1.0) / 3.0;
    1.0 / p.sqrt()
}

#[inline]
fn niveau(n: u8, v: u32) -> f32 {
    let l = 1u32 << n;
    (2 * gray_inverse(v) as i32 - (l as i32 - 1)) as f32
}

#[inline]
fn decider_axe(n: u8, x: f32) -> u32 {
    let l = (1u32 << n) as f32;
    let j = ((x + l - 1.0) / 2.0).round().clamp(0.0, l - 1.0) as u32;
    gray(j)
}

/// Point MAQ (puissance moyenne 1) portant la valeur `v` sur `bits` bits.
pub fn maq(bits: u8, v: u32) -> Cpx {
    let (bi, bq) = axes(bits);
    let vi = v >> bq;
    let vq = v & ((1 << bq) - 1);
    let s = echelle(bits);
    let q = if bq == 0 { 0.0 } else { niveau(bq, vq) };
    Cpx::new(niveau(bi, vi) * s, q * s)
}

/// Décision MAQ : (valeur, point décidé).
pub fn maq_decider(bits: u8, z: Cpx) -> (u32, Cpx) {
    let (bi, bq) = axes(bits);
    let s = echelle(bits);
    let vi = decider_axe(bi, z.re / s);
    let vq = if bq == 0 { 0 } else { decider_axe(bq, z.im / s) };
    let v = (vi << bq) | vq;
    (v, maq(bits, v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gray_aller_retour() {
        for i in 0..256 {
            assert_eq!(gray_inverse(gray(i)), i);
            assert_eq!((gray(i) ^ gray(i + 1)).count_ones(), 1);
        }
    }

    #[test]
    fn maq_aller_retour_et_puissance_unite() {
        for bits in 1..=10u8 {
            let m = 1u32 << bits;
            let mut p = 0.0;
            for v in 0..m {
                let z = maq(bits, v);
                p += z.norm_sqr();
                assert_eq!(maq_decider(bits, z).0, v, "{bits} bits, v = {v}");
                // un léger bruit ne change pas la décision
                assert_eq!(maq_decider(bits, z + Cpx::new(0.02, -0.02)).0, v);
            }
            assert!((p / m as f32 - 1.0).abs() < 1e-4, "{bits} bits : puissance {}", p / m as f32);
        }
    }

    #[test]
    fn mdp_aller_retour() {
        for bits in 1..=5u8 {
            for i in 0..(1u32 << bits) {
                let z = mdp(bits, i) * 0.7;
                assert_eq!(mdp_decider(bits, z), i);
            }
        }
    }
}
