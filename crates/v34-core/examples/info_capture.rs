//! Analyse d'une capture : bits bruts des séquences INFO (DPSK 600 bit/s)
//! d'un canal d'un fichier WAV 8 kHz 16 bits (celui de `v34-cli … --wav`).
//!
//!     cargo run --release -p v34-core --example info_capture -- capture.wav [canal] [porteuse]
//!
//! canal : 0 = gauche (ce que `v34-cli` a reçu, défaut), 1 = droite (émis) ;
//! porteuse : 2400 (INFO de l'appelé, défaut) ou 1200 (INFO de l'appelant) ;
//! bits d'information : 17 (INFO0, défaut) ou 77 (INFO1c), 38 (INFO1a).
//! Pour chaque trame repérée (« 11 » + synchro 01110010), affiche les bits
//! qui suivent la synchro, et vérifie le CRC selon plusieurs conventions.

use v34_core::dsp::shaping::Downconverter;
use v34_core::dsp::Cpx;
use v34_core::info::crc16;

fn read_wav(path: &str, channel: usize) -> Vec<f32> {
    let b = std::fs::read(path).expect("lecture du WAV");
    let channels = u16::from_le_bytes([b[22], b[23]]) as usize;
    let data = &b[44..];
    data.chunks_exact(2 * channels).map(|f| i16::from_le_bytes([f[2 * channel], f[2 * channel + 1]]) as f32 / 32767.0).collect()
}

fn bits_str(b: &[bool]) -> String {
    b.iter().map(|&x| if x { '1' } else { '0' }).collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).expect("chemin du WAV");
    let channel: usize = args.get(2).map_or(0, |s| s.parse().unwrap());
    let fc: f64 = args.get(3).map_or(2400.0, |s| s.parse().unwrap());
    let ninfo: usize = args.get(4).map_or(17, |s| s.parse().unwrap());
    let x = read_wav(path, channel);
    let sps = 8000.0 / 600.0;
    let mut d = Downconverter::new(600.0, fc, 0.5, 6);
    // 4 phases d'échantillonnage, comme le récepteur du modem
    let mut streams: Vec<(f64, Cpx, Vec<(f64, bool)>)> = (0..4).map(|k| (k as f64 * sps / 4.0, Cpx::ZERO, Vec::new())).collect();
    for (n, &s) in x.iter().enumerate() {
        d.push(s);
        let t_now = d.latest();
        for st in streams.iter_mut() {
            if t_now < st.0 {
                continue;
            }
            let t = st.0;
            st.0 += sps;
            let Some(y) = d.at(t) else { continue };
            let p = y.norm_sqr();
            let valid = p > 1e-6 && st.1.norm_sqr() > 1e-6;
            let dot = y.re * st.1.re + y.im * st.1.im;
            st.1 = y;
            if valid {
                st.2.push((n as f64 / 8000.0, dot < 0.0));
            }
        }
    }
    let sync = [true, true, false, true, true, true, false, false, true, false];
    for (k, st) in streams.iter().enumerate() {
        let bits: Vec<bool> = st.2.iter().map(|b| b.1).collect();
        let mut shown = 0;
        let mut i = 0;
        while i + 10 + ninfo + 20 <= bits.len() && shown < 6 {
            if bits[i..i + 10] == sync {
                let after = &bits[i + 10..i + 10 + ninfo + 20];
                let info = &after[..ninfo];
                let crc_lsb: u16 = (0..16).fold(0, |a, j| a | ((after[ninfo + j] as u16) << j));
                let crc_msb: u16 = (0..16).fold(0, |a, j| a << 1 | after[ninfo + j] as u16);
                let calc = crc16(info);
                println!(
                    "phase {k} t={:.3} s : {} | {} | crc reçu lsb {:04x} msb {:04x}, calculé {:04x}{}",
                    st.2[i].0,
                    bits_str(info),
                    bits_str(&after[ninfo..ninfo + 16]),
                    crc_lsb,
                    crc_msb,
                    calc,
                    if calc == crc_lsb { "  ← OK (lsb)" } else if calc == crc_msb { "  ← OK (msb)" } else { "" }
                );
                shown += 1;
                i += ninfo + 32;
            } else {
                i += 1;
            }
        }
    }
}
