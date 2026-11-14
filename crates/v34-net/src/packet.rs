//! Format des paquets audio échangés sur UDP.
//!
//! En-tête (16 octets, gros-boutiste) :
//! ```text
//! 0      2     3      4          8          12         16
//! | "V3" | ver | codec | call_id  |   seq    | timestamp | charge utile…
//! ```
//! - `call_id` : identifiant d'appel (0 = liaison directe). Réservé pour le
//!   futur « central téléphonique » sur VPS qui relaiera plusieurs appels.
//! - `seq` : numéro de séquence (+1 par paquet).
//! - `timestamp` : position du premier échantillon, en échantillons à 8 kHz.
//! - charge utile : 160 échantillons (20 ms) codés selon `codec`, suivis
//!   éventuellement des une ou deux trames précédentes (redondance : un
//!   paquet perdu est rattrapé avec le suivant). Un récepteur ancien ne garde
//!   que les 160 premiers échantillons : il ignore cette queue sans erreur.

use crate::g711;

pub const MAGIC: [u8; 2] = *b"V3";
pub const VERSION: u8 = 1;
pub const HEADER_LEN: usize = 16;
/// Échantillons par paquet (20 ms à 8 kHz).
pub const FRAME_SAMPLES: usize = 160;
/// Trames précédentes répétées dans chaque paquet (redondance).
pub const BACKUP_FRAMES: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Codec {
    /// G.711 loi µ (8 bits/échantillon)
    Ulaw,
    /// G.711 loi A (8 bits/échantillon) — celle du réseau français
    #[default]
    Alaw,
    /// PCM linéaire 16 bits : ligne numérique « parfaite »
    L16,
}

impl Codec {
    pub fn id(self) -> u8 {
        match self {
            Codec::Ulaw => 0,
            Codec::Alaw => 8,
            Codec::L16 => 11,
        }
    }

    pub fn from_id(id: u8) -> Option<Codec> {
        match id {
            0 => Some(Codec::Ulaw),
            8 => Some(Codec::Alaw),
            11 => Some(Codec::L16),
            _ => None,
        }
    }

    pub fn from_name(s: &str) -> Option<Codec> {
        match s.to_ascii_lowercase().as_str() {
            "ulaw" | "mulaw" | "u" | "pcmu" => Some(Codec::Ulaw),
            "alaw" | "a" | "pcma" => Some(Codec::Alaw),
            "l16" | "pcm" | "linear" => Some(Codec::L16),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Codec::Ulaw => "G.711 µ-law",
            Codec::Alaw => "G.711 A-law",
            Codec::L16 => "L16",
        }
    }

    fn bytes_per_sample(self) -> usize {
        match self {
            Codec::Ulaw | Codec::Alaw => 1,
            Codec::L16 => 2,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Packet {
    pub codec: Codec,
    pub call_id: u32,
    pub seq: u32,
    pub timestamp: u32,
    /// Échantillons décodés (FRAME_SAMPLES), ±1.0
    pub samples: Vec<f32>,
    /// Trames précédentes répétées (redondance) : `backup[0]` est la trame
    /// `seq − 1`, `backup[1]` la trame `seq − 2`.
    pub backup: Vec<Vec<f32>>,
}

impl Packet {
    /// Sérialise le paquet (encode les échantillons).
    pub fn encode(&self) -> Vec<u8> {
        let n = self.samples.len() + self.backup.iter().map(Vec::len).sum::<usize>();
        let mut out = Vec::with_capacity(HEADER_LEN + n * self.codec.bytes_per_sample());
        out.extend_from_slice(&MAGIC);
        out.push(VERSION);
        out.push(self.codec.id());
        out.extend_from_slice(&self.call_id.to_be_bytes());
        out.extend_from_slice(&self.seq.to_be_bytes());
        out.extend_from_slice(&self.timestamp.to_be_bytes());
        for &s in self.samples.iter().chain(self.backup.iter().flatten()) {
            let v = g711::f32_to_i16(s);
            match self.codec {
                Codec::Ulaw => out.push(g711::linear_to_ulaw(v)),
                Codec::Alaw => out.push(g711::linear_to_alaw(v)),
                Codec::L16 => out.extend_from_slice(&v.to_be_bytes()),
            }
        }
        out
    }

    /// Analyse un datagramme ; `None` s'il n'est pas valide.
    pub fn decode(buf: &[u8]) -> Option<Packet> {
        if buf.len() < HEADER_LEN || buf[0..2] != MAGIC || buf[2] != VERSION {
            return None;
        }
        let codec = Codec::from_id(buf[3])?;
        let u32_at = |i: usize| u32::from_be_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]);
        let payload = &buf[HEADER_LEN..];
        let samples: Vec<f32> = match codec {
            Codec::Ulaw => payload.iter().map(|&b| g711::i16_to_f32(g711::ulaw_to_linear(b))).collect(),
            Codec::Alaw => payload.iter().map(|&b| g711::i16_to_f32(g711::alaw_to_linear(b))).collect(),
            Codec::L16 => {
                if payload.len() % 2 != 0 {
                    return None;
                }
                payload.chunks_exact(2).map(|c| g711::i16_to_f32(i16::from_be_bytes([c[0], c[1]]))).collect()
            }
        };
        if samples.is_empty() || samples.len() > 4 * FRAME_SAMPLES {
            return None;
        }
        let (mut samples, mut backup) = (samples, Vec::new());
        let k = samples.len() / FRAME_SAMPLES;
        if samples.len() % FRAME_SAMPLES == 0 && (2..=1 + BACKUP_FRAMES).contains(&k) {
            backup = samples[FRAME_SAMPLES..].chunks(FRAME_SAMPLES).map(<[f32]>::to_vec).collect();
            samples.truncate(FRAME_SAMPLES);
        }
        Some(Packet { codec, call_id: u32_at(4), seq: u32_at(8), timestamp: u32_at(12), samples, backup })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_all_codecs() {
        let samples: Vec<f32> = (0..FRAME_SAMPLES).map(|i| ((i as f32) * 0.07).sin() * 0.5).collect();
        for codec in [Codec::Ulaw, Codec::Alaw, Codec::L16] {
            let p = Packet { codec, call_id: 0xDEAD_BEEF, seq: 42, timestamp: 6720, samples: samples.clone(), backup: Vec::new() };
            let bytes = p.encode();
            let q = Packet::decode(&bytes).unwrap();
            assert_eq!((q.codec, q.call_id, q.seq, q.timestamp), (codec, 0xDEAD_BEEF, 42, 6720));
            assert_eq!(q.samples.len(), FRAME_SAMPLES);
            let tol = if codec == Codec::L16 { 1.0 / 32000.0 } else { 0.02 };
            for (a, b) in samples.iter().zip(&q.samples) {
                assert!((a - b).abs() <= tol, "{codec:?} {a} {b}");
            }
        }
    }

    #[test]
    fn backup_frames_round_trip_and_old_receivers_ignore_them() {
        let f = |a: f32| -> Vec<f32> { (0..FRAME_SAMPLES).map(|i| ((i as f32) * 0.05 + a).sin() * 0.4).collect() };
        let p = Packet { codec: Codec::Alaw, call_id: 7, seq: 10, timestamp: 1600, samples: f(0.0), backup: vec![f(1.0), f(2.0)] };
        let bytes = p.encode();
        assert_eq!(bytes.len(), HEADER_LEN + 3 * FRAME_SAMPLES);
        let q = Packet::decode(&bytes).unwrap();
        assert_eq!(q.samples.len(), FRAME_SAMPLES);
        assert_eq!(q.backup.len(), 2);
        assert!((q.backup[1][5] - f(2.0)[5]).abs() < 0.02);
        // un récepteur ancien (sans `backup`) prend les 160 premiers : la trame courante
        assert!((q.samples[5] - f(0.0)[5]).abs() < 0.02);
    }

    #[test]
    fn rejects_garbage() {
        assert!(Packet::decode(b"hello").is_none());
        assert!(Packet::decode(&[0u8; 200]).is_none());
    }
}
