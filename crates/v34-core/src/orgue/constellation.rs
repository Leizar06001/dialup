//! Constellations : pas de phase dans un arc (mode « joli ») et MAQ
//! rectangulaire (mode « rapide »), codage de Gray, seuils de chargement.
//!
//! Pourquoi un arc : si chaque harmonique prenait une phase au hasard à
//! chaque symbole (MDP sur tout le cercle), son énergie s'étalerait sur
//! ±1/T autour d'elle — à peu près l'écart entre harmoniques — et le
//! spectre deviendrait presque plat : un souffle coloré, plus de hauteur.
//! Avec des pas limités à ±45°, la phase ne fait que diffuser lentement :
//! ~80 % de la puissance reste sur les raies (une « voix céleste »), au
//! prix d'environ 12 dB de SNR à nombre de bits égal.

use std::f32::consts::PI;

use super::Mode;
use crate::dsp::Cpx;

/// Bits maximum par harmonique.
pub fn bits_max(mode: Mode) -> u8 {
    match mode {
        Mode::Joli => 4,
        Mode::Rapide => 10,
    }
}

/// Rapport signal/bruit (dB, par harmonique) qu'il faut pour porter `bits`
/// bits avec un taux d'erreur brut de l'ordre de 1e-5, sans marge.
/// Joli : 2^b pas espacés de 90°/2^b, décidés de façon cohérente par
/// rapport à la phase décidée précédente (une erreur en coûte deux) :
/// autant qu'une MDP-2^(b+2) sur tout le cercle, avec un peu de marge pour
/// le bruit de phase résiduel (mesuré). Rapide : MAQ carrée ou
/// rectangulaire (8, 32, 128, 512 points), décision cohérente.
pub fn snr_requis_db(mode: Mode, bits: u8) -> f32 {
    match mode {
        Mode::Joli => [0.0, 18.5, 24.5, 32.0, 38.0][bits as usize],
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

/// Ouverture de l'arc des pas de phase du mode joli (90°).
pub const ARC_JOLI: f32 = PI / 2.0;

/// Pas de phase (rad) portant la valeur `v` (code de Gray, cyclique) sur
/// `bits` bits. La valeur 0 tombe sur le plus petit pas (juste sous le
/// centre de l'arc) : une ligne au repos reste une note tenue.
pub fn pas_arc(bits: u8, v: u32) -> f32 {
    let m = 1u32 << bits;
    let j = (gray_inverse(v) + m / 2 - 1) % m;
    (j as f32 - (m as f32 - 1.0) / 2.0) * ARC_JOLI / m as f32
}

/// Décision d'un pas de phase observé `d` (rad) : (valeur, pas décidé).
pub fn arc_decider(bits: u8, d: f32) -> (u32, f32) {
    let m = 1u32 << bits;
    let mf = m as f32;
    let j = (d * mf / ARC_JOLI + (mf - 1.0) / 2.0).round().clamp(0.0, mf - 1.0) as u32;
    (gray((j + m - (m / 2 - 1)) % m), (j as f32 - (mf - 1.0) / 2.0) * ARC_JOLI / mf)
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
    fn arc_aller_retour_dans_plus_ou_moins_45_degres() {
        for bits in 1..=4u8 {
            for v in 0..(1u32 << bits) {
                let p = pas_arc(bits, v);
                assert!(p.abs() < ARC_JOLI / 2.0);
                if v == 0 {
                    assert!(p.abs() < ARC_JOLI / (1 << bits) as f32, "{bits} bits : zéro sur le plus petit pas");
                }
                assert_eq!(arc_decider(bits, p + 0.2 * ARC_JOLI / (1 << bits) as f32).0, v);
            }
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
