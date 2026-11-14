//! Petite FFT radix-2 pour l'analyseur de spectre de l'IHM.

use std::f32::consts::PI;

/// Spectre en dBFS (n/2 valeurs) de `samples` (longueur puissance de 2), fenêtre de Hann.
pub fn spectrum_db(samples: &[f32]) -> Vec<f32> {
    let n = samples.len();
    debug_assert!(n.is_power_of_two());
    let mut re: Vec<f32> = samples
        .iter()
        .enumerate()
        .map(|(i, &s)| s * (0.5 - 0.5 * (2.0 * PI * i as f32 / n as f32).cos()))
        .collect();
    let mut im = vec![0.0f32; n];
    fft_in_place(&mut re, &mut im);
    // Gain cohérent de la fenêtre de Hann = 0.5 ; pleine échelle sinus → 0 dBFS.
    let norm = 2.0 / (n as f32 * 0.5);
    (0..n / 2)
        .map(|k| {
            let m = (re[k] * re[k] + im[k] * im[k]).sqrt() * norm;
            20.0 * m.max(1e-6).log10()
        })
        .collect()
}

fn fft_in_place(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    // Permutation bit-reverse
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * PI / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * cr - im[b] * ci;
                let ti = re[b] * ci + im[b] * cr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_peak_at_right_bin() {
        let n = 256;
        // 1000 Hz à 8 kHz → bin 32
        let s: Vec<f32> = (0..n).map(|i| (2.0 * PI * 1000.0 * i as f32 / 8000.0).sin()).collect();
        let db = spectrum_db(&s);
        let (kmax, vmax) = db.iter().enumerate().fold((0, f32::MIN), |a, (k, &v)| if v > a.1 { (k, v) } else { a });
        assert_eq!(kmax, 32);
        assert!(vmax.abs() < 1.0, "pic à {vmax} dBFS");
    }
}
