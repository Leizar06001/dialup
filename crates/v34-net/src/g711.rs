//! Codec G.711 (ITU-T) : loi A (Europe, dont la France) et loi µ (USA, Japon).
//!
//! Implémentation de référence classique (dérivée du g711.c de Sun
//! Microsystems, domaine public). Entrée/sortie linéaire sur 16 bits.

const SIGN_BIT: u8 = 0x80;
const QUANT_MASK: u8 = 0x0F;
const SEG_SHIFT: u8 = 4;
const SEG_MASK: u8 = 0x70;

const SEG_AEND: [i16; 8] = [0x1F, 0x3F, 0x7F, 0xFF, 0x1FF, 0x3FF, 0x7FF, 0xFFF];
const SEG_UEND: [i16; 8] = [0x3F, 0x7F, 0xFF, 0x1FF, 0x3FF, 0x7FF, 0xFFF, 0x1FFF];

const ULAW_BIAS: i16 = 0x84;
const ULAW_CLIP: i16 = 8159;

fn search(val: i16, table: &[i16; 8]) -> usize {
    table.iter().position(|&end| val <= end).unwrap_or(8)
}

/// Linéaire 16 bits → loi A.
pub fn linear_to_alaw(pcm: i16) -> u8 {
    let mut v = pcm >> 3;
    let mask: u8 = if v >= 0 {
        0xD5
    } else {
        v = -v - 1;
        0x55
    };
    let seg = search(v, &SEG_AEND);
    if seg >= 8 {
        return 0x7F ^ mask;
    }
    let mut aval = (seg as u8) << SEG_SHIFT;
    aval |= if seg < 2 { ((v >> 1) as u8) & QUANT_MASK } else { ((v >> seg) as u8) & QUANT_MASK };
    aval ^ mask
}

/// Loi A → linéaire 16 bits.
pub fn alaw_to_linear(a: u8) -> i16 {
    let a = a ^ 0x55;
    let mut t = ((a & QUANT_MASK) as i16) << 4;
    let seg = (a & SEG_MASK) >> SEG_SHIFT;
    match seg {
        0 => t += 8,
        1 => t += 0x108,
        _ => {
            t += 0x108;
            t <<= seg - 1;
        }
    }
    if a & SIGN_BIT != 0 { t } else { -t }
}

/// Linéaire 16 bits → loi µ.
pub fn linear_to_ulaw(pcm: i16) -> u8 {
    let mut v = pcm >> 2;
    let mask: u8 = if v < 0 {
        v = -v;
        0x7F
    } else {
        0xFF
    };
    if v > ULAW_CLIP {
        v = ULAW_CLIP;
    }
    v += ULAW_BIAS >> 2;
    let seg = search(v, &SEG_UEND);
    if seg >= 8 {
        return 0x7F ^ mask;
    }
    let uval = ((seg as u8) << 4) | (((v >> (seg + 1)) as u8) & 0x0F);
    uval ^ mask
}

/// Loi µ → linéaire 16 bits.
pub fn ulaw_to_linear(u: u8) -> i16 {
    let u = !u;
    let mut t = (((u & QUANT_MASK) as i16) << 3) + ULAW_BIAS;
    t <<= (u & SEG_MASK) >> SEG_SHIFT;
    if u & SIGN_BIT != 0 { ULAW_BIAS - t } else { t - ULAW_BIAS }
}

/// Conversion f32 (±1.0) → i16 avec écrêtage.
#[inline]
pub fn f32_to_i16(x: f32) -> i16 {
    (x * 32767.0).round().clamp(-32768.0, 32767.0) as i16
}

/// Conversion i16 → f32 (±1.0).
#[inline]
pub fn i16_to_f32(x: i16) -> f32 {
    x as f32 / 32767.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alaw_codes_round_trip() {
        for c in 0..=255u8 {
            assert_eq!(linear_to_alaw(alaw_to_linear(c)), c, "code {c:#04x}");
        }
    }

    #[test]
    fn ulaw_codes_round_trip() {
        for c in 0..=255u8 {
            if c == 0x7F {
                continue; // « -0 » se ré-encode en « +0 » (0xFF)
            }
            assert_eq!(linear_to_ulaw(ulaw_to_linear(c)), c, "code {c:#04x}");
        }
    }

    #[test]
    fn known_values() {
        // Valeurs de référence G.711
        assert_eq!(linear_to_alaw(0), 0xD5);
        assert_eq!(linear_to_ulaw(0), 0xFF);
        assert_eq!(ulaw_to_linear(0xFF), 0);
        assert_eq!(alaw_to_linear(0xD5), 8);
        assert_eq!(ulaw_to_linear(0x00), -32124);
        assert_eq!(ulaw_to_linear(0x80), 32124);
        assert_eq!(alaw_to_linear(0xAA), 32256);
    }

    /// V.90 : le modem numérique émet des échantillons f32 ; le transport
    /// doit en tirer exactement l'octet G.711 voulu, et l'abonné retrouver
    /// exactement la valeur du tableau 1/V.90.
    #[test]
    fn niveaux_v90_transportes_exactement() {
        use v34_core::v90::codes::{codeword, linear, sample, Law};
        for u in 0..128u8 {
            for pos in [true, false] {
                let x = f32_to_i16(sample(Law::A, u, pos));
                assert_eq!(linear_to_alaw(x), codeword(Law::A, u, pos), "A Ucode {u}");
                let y = alaw_to_linear(codeword(Law::A, u, pos)) as i32;
                assert_eq!(y, if pos { linear(Law::A, u) } else { -linear(Law::A, u) });
                let x = f32_to_i16(sample(Law::Mu, u, pos));
                if !(u == 0 && !pos) {
                    assert_eq!(linear_to_ulaw(x), codeword(Law::Mu, u, pos), "µ Ucode {u}");
                }
                let y = ulaw_to_linear(codeword(Law::Mu, u, pos)) as i32;
                assert_eq!(y, if pos { linear(Law::Mu, u) } else { -linear(Law::Mu, u) });
            }
        }
    }

    #[test]
    fn quantization_error_is_logarithmic() {
        for &x in &[100i16, 1000, 5000, 20000, -300, -12000, 32000] {
            for (enc, dec) in [(linear_to_alaw as fn(i16) -> u8, alaw_to_linear as fn(u8) -> i16), (linear_to_ulaw, ulaw_to_linear)] {
                let y = dec(enc(x));
                let err = (y as i32 - x as i32).abs();
                assert!(err <= (x as i32).abs() / 16 + 32, "x={x} y={y}");
                assert_eq!(x.signum(), y.signum());
            }
        }
    }
}
