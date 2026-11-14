//! Les 128 niveaux PCM de G.711, repérés par leur « Ucode » (V.90 tableau 1).
//!
//! Un Ucode (0 à 127) désigne la même amplitude positive en loi µ et en loi
//! A ; le signe est porté à part. Valeurs linéaires sur 16 bits, exactement
//! celles du tableau 1/V.90 (et du décodeur G.711 de référence).

/// Loi de compression du réseau : µ (Amérique du Nord, Japon) ou A (Europe,
/// dont la France).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Law {
    Mu,
    #[default]
    A,
}

/// Nombre de Ucodes.
pub const UCODES: usize = 128;
/// Échelle entre la valeur linéaire 16 bits et l'échantillon f32 du modem
/// (celle du transport G.711 : `f32 × 32767` → i16).
pub const SCALE: f32 = 32767.0;

/// Valeur linéaire (16 bits, positive) du Ucode `u`.
pub fn linear(law: Law, u: u8) -> i32 {
    let u = (u & 0x7F) as i32;
    let (seg, m) = (u >> 4, u & 15);
    match law {
        Law::Mu => (((m << 3) + 0x84) << seg) - 0x84,
        Law::A => {
            if seg == 0 {
                (2 * m + 1) * 8
            } else {
                ((2 * m + 33) << (seg - 1)) * 8
            }
        }
    }
}

/// Octet G.711 transmis pour le Ucode `u` avec le signe `positive`.
pub fn codeword(law: Law, u: u8, positive: bool) -> u8 {
    let u = u & 0x7F;
    let c = match law {
        Law::Mu => 0xFF - u,
        Law::A => (u ^ 0x55) | 0x80,
    };
    if positive {
        c
    } else {
        c ^ 0x80
    }
}

/// Échantillon émis par le modem numérique pour ce Ucode et ce signe : la
/// conversion G.711 du transport retrouve exactement le bon octet.
pub fn sample(law: Law, u: u8, positive: bool) -> f32 {
    let v = linear(law, u) as f32 / SCALE;
    if positive {
        v
    } else {
        -v
    }
}

/// Table des 128 valeurs linéaires d'une loi.
pub fn table(law: Law) -> [i32; UCODES] {
    let mut t = [0; UCODES];
    for (u, v) in t.iter_mut().enumerate() {
        *v = linear(law, u as u8);
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tableau_1_de_v90() {
        // quelques lignes du tableau 1/V.90
        for (u, mu, mu_lin, a, a_lin) in [
            (0u8, 0xFFu8, 0, 0xD5u8, 8),
            (15, 0xF0, 120, 0xDA, 248),
            (16, 0xEF, 132, 0xC5, 264),
            (32, 0xDF, 396, 0xF5, 528),
            (47, 0xD0, 876, 0xFA, 1008),
            (64, 0xBF, 1980, 0x95, 2112),
            (100, 0x9B, 10364, 0xB1, 10496),
            (127, 0x80, 32124, 0xAA, 32256),
        ] {
            assert_eq!((codeword(Law::Mu, u, true), linear(Law::Mu, u)), (mu, mu_lin), "µ Ucode {u}");
            assert_eq!((codeword(Law::A, u, true), linear(Law::A, u)), (a, a_lin), "A Ucode {u}");
        }
        // croissance stricte : les Ucodes sont rangés par amplitude
        for law in [Law::Mu, Law::A] {
            let t = table(law);
            assert!(t.windows(2).all(|w| w[0] < w[1]));
        }
    }
}
