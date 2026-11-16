//! RTP (RFC 3550) : en-têtes, et passage aux trames de 20 ms du central.
//!
//! Le central découpe l'audio en trames de 160 échantillons numérotées ;
//! un ATA envoie des paquets de 10, 20 ou 30 ms. [`Reframer`] recoupe le
//! flux reçu en trames de 160 échantillons alignées sur l'horodatage RTP,
//! et numérote chaque trame d'après sa position dans le temps (et non
//! d'après le numéro de séquence) : un paquet perdu laisse un trou à sa
//! place, sans décaler la suite. [`Numbering`] fait l'inverse : numéro de
//! séquence et horodatage RTP déduits de ceux de la trame, pour qu'un trou
//! reste un trou chez l'ATA aussi.

use v34_net::g711;
use v34_net::{Codec, FRAME_SAMPLES};

pub const HEADER_LEN: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub pt: u8,
    pub marker: bool,
    pub seq: u16,
    pub ts: u32,
    pub ssrc: u32,
}

/// Version 2 dans les deux bits de poids fort.
pub fn is_rtp(buf: &[u8]) -> bool {
    buf.len() >= HEADER_LEN && buf[0] >> 6 == 2
}

/// En-tête et charge utile (sans CSRC, extension ni bourrage).
pub fn parse(buf: &[u8]) -> Option<(Header, &[u8])> {
    if !is_rtp(buf) {
        return None;
    }
    let cc = (buf[0] & 0x0F) as usize;
    let mut start = HEADER_LEN + 4 * cc;
    if buf[0] & 0x10 != 0 {
        let ext = buf.get(start + 2..start + 4)?;
        start += 4 + 4 * u16::from_be_bytes([ext[0], ext[1]]) as usize;
    }
    let mut end = buf.len();
    if buf[0] & 0x20 != 0 {
        end = end.checked_sub(*buf.last()? as usize)?;
    }
    let payload = buf.get(start..end)?;
    let h = Header {
        pt: buf[1] & 0x7F,
        marker: buf[1] & 0x80 != 0,
        seq: u16::from_be_bytes([buf[2], buf[3]]),
        ts: u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]),
        ssrc: u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]),
    };
    Some((h, payload))
}

pub fn encode(h: &Header, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.push(0x80);
    out.push(h.pt & 0x7F | if h.marker { 0x80 } else { 0 });
    out.extend_from_slice(&h.seq.to_be_bytes());
    out.extend_from_slice(&h.ts.to_be_bytes());
    out.extend_from_slice(&h.ssrc.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// Échantillons (±1.0) → octets G.711.
pub fn encode_payload(law: Codec, samples: &[f32]) -> Vec<u8> {
    samples
        .iter()
        .map(|&s| {
            let v = g711::f32_to_i16(s);
            match law {
                Codec::Ulaw => g711::linear_to_ulaw(v),
                _ => g711::linear_to_alaw(v),
            }
        })
        .collect()
}

/// Octets G.711 → échantillons (±1.0).
pub fn decode_payload(law: Codec, bytes: &[u8]) -> Vec<f32> {
    bytes
        .iter()
        .map(|&b| {
            g711::i16_to_f32(match law {
                Codec::Ulaw => g711::ulaw_to_linear(b),
                _ => g711::alaw_to_linear(b),
            })
        })
        .collect()
}

const F: u32 = FRAME_SAMPLES as u32;

/// Flux RTP reçu → trames de 160 échantillons numérotées.
#[derive(Clone, Debug, Default)]
pub struct Reframer {
    ssrc: Option<u32>,
    /// Horodatage RTP du début de la trame 0
    base: u32,
    /// Trame en cours de remplissage et ses premiers octets
    pending: Option<(u32, Vec<u8>)>,
    last: Option<u32>,
}

impl Reframer {
    /// Ajoute un paquet ; renvoie les trames complètes (numéro, 160 octets).
    pub fn push(&mut self, ssrc: u32, ts: u32, payload: &[u8]) -> Vec<(u32, Vec<u8>)> {
        let mut out = Vec::new();
        if self.ssrc != Some(ssrc) {
            // Nouveau flux (début, ou l'ATA a changé de source) : il reprend
            // à la trame qui suit la dernière, sans trou ni recouvrement.
            let next = self.last.map_or(0, |l| l.wrapping_add(1));
            self.base = ts.wrapping_sub(next.wrapping_mul(F));
            self.ssrc = Some(ssrc);
            self.pending = None;
        }
        let pos = ts.wrapping_sub(self.base);
        if pos > u32::MAX / 2 {
            return out; // antérieur au début du flux
        }
        let (mut pos, mut data) = (pos, payload);
        if let Some((f, buf)) = &self.pending {
            if f.wrapping_mul(F).wrapping_add(buf.len() as u32) != pos {
                self.pending = None; // un morceau manque : trame incomplète abandonnée
            }
        }
        while !data.is_empty() {
            let (f, buf) = match self.pending.as_mut() {
                Some(p) => p,
                None => {
                    // on se cale sur la prochaine frontière de trame
                    let f = pos.div_ceil(F);
                    let skip = (f.wrapping_mul(F).wrapping_sub(pos)) as usize;
                    if skip >= data.len() {
                        break;
                    }
                    data = &data[skip..];
                    pos = f.wrapping_mul(F);
                    self.pending.insert((f, Vec::with_capacity(FRAME_SAMPLES)))
                }
            };
            let n = (FRAME_SAMPLES - buf.len()).min(data.len());
            buf.extend_from_slice(&data[..n]);
            data = &data[n..];
            pos = pos.wrapping_add(n as u32);
            if buf.len() == FRAME_SAMPLES {
                let f = *f;
                out.push((f, std::mem::replace(buf, Vec::with_capacity(FRAME_SAMPLES))));
                self.pending = Some((f.wrapping_add(1), Vec::with_capacity(FRAME_SAMPLES)));
                self.last = Some(f);
            }
        }
        out
    }
}

/// Trames numérotées → en-têtes RTP (source et décalages tirés au hasard).
#[derive(Clone, Debug)]
pub struct Numbering {
    pub ssrc: u32,
    pub pt: u8,
    seq_off: u16,
    ts_off: u32,
    started: bool,
}

impl Numbering {
    pub fn new(pt: u8) -> Numbering {
        let r = v34_net::crypto::random_bytes::<10>();
        Numbering {
            ssrc: u32::from_be_bytes([r[0], r[1], r[2], r[3]]),
            pt,
            seq_off: u16::from_be_bytes([r[4], r[5]]),
            ts_off: u32::from_be_bytes([r[6], r[7], r[8], r[9]]),
            started: false,
        }
    }

    /// En-tête du paquet portant la trame `seq` (horodatage `ts`, en
    /// échantillons) du central.
    pub fn header(&mut self, seq: u32, ts: u32) -> Header {
        let marker = !std::mem::replace(&mut self.started, true);
        Header { pt: self.pt, marker, seq: (seq as u16).wrapping_add(self.seq_off), ts: ts.wrapping_add(self.ts_off), ssrc: self.ssrc }
    }

    /// Paquet RTP complet pour ces échantillons.
    pub fn packet(&mut self, law: Codec, seq: u32, ts: u32, samples: &[f32]) -> Vec<u8> {
        let h = self.header(seq, ts);
        encode(&h, &encode_payload(law, samples))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(start: u8, n: usize) -> Vec<u8> {
        (0..n).map(|i| start.wrapping_add(i as u8)).collect()
    }

    #[test]
    fn en_tete_aller_retour_avec_csrc_extension_bourrage() {
        let h = Header { pt: 8, marker: true, seq: 65535, ts: 0xDEAD_BEEF, ssrc: 7 };
        let p = encode(&h, &[1, 2, 3]);
        assert_eq!(parse(&p), Some((h, &[1u8, 2, 3][..])));
        // un CSRC, une extension d'un mot, deux octets de bourrage
        let mut q = vec![0x80 | 0x20 | 0x10 | 1, 0];
        q.extend_from_slice(&[0, 1, 0, 0, 0, 160, 0, 0, 0, 9]);
        q.extend_from_slice(&[0, 0, 0, 1]); // CSRC
        q.extend_from_slice(&[0xBE, 0xDE, 0, 1, 9, 9, 9, 9]); // extension
        q.extend_from_slice(&[5, 6, 7, 0, 2]); // charge + bourrage (2)
        let (h2, pl) = parse(&q).unwrap();
        assert_eq!((h2.seq, h2.ts, h2.ssrc), (1, 160, 9));
        assert_eq!(pl, &[5, 6, 7]);
        assert!(parse(b"V3\x01\x08\0\0\0\0\0\0\0\0").is_none());
    }

    #[test]
    fn paquets_de_20_ms_avec_une_perte() {
        let mut r = Reframer::default();
        assert_eq!(r.push(1, 1000, &bytes(0, 160)), vec![(0, bytes(0, 160))]);
        assert_eq!(r.push(1, 1160, &bytes(1, 160))[0].0, 1);
        // le paquet de 1320 est perdu : la trame suivante garde sa place
        assert_eq!(r.push(1, 1480, &bytes(3, 160))[0].0, 3);
        // un paquet en retard retrouve aussi la sienne
        assert_eq!(r.push(1, 1320, &bytes(2, 160))[0].0, 2);
    }

    #[test]
    fn paquets_de_30_ms_recoupes() {
        let mut r = Reframer::default();
        let a = r.push(5, 0, &bytes(0, 240));
        assert_eq!(a.len(), 1);
        assert_eq!(a[0], (0, bytes(0, 160)));
        let b = r.push(5, 240, &bytes(240, 240));
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].0, 1);
        assert_eq!(b[0].1[..80], bytes(160, 80)[..]);
        assert_eq!(b[0].1[80..], bytes(240, 80)[..]);
        assert_eq!(b[1], (2, bytes(64, 160)));
        // perte du paquet suivant (480) : on se recale sur la frontière 800
        let c = r.push(5, 720, &bytes(0, 240));
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].0, 5);
        assert_eq!(c[0].1, bytes(80, 160));
    }

    #[test]
    fn changement_de_source_sans_trou() {
        let mut r = Reframer::default();
        r.push(1, 0, &bytes(0, 160));
        r.push(1, 160, &bytes(0, 160));
        // nouvelle source, horodatage sans rapport : on enchaîne à la trame 2
        assert_eq!(r.push(2, 777_777, &bytes(0, 160))[0].0, 2);
        assert_eq!(r.push(2, 777_937, &bytes(0, 160))[0].0, 3);
    }

    #[test]
    fn numerotation_garde_les_trous() {
        let mut n = Numbering::new(8);
        let a = n.header(10, 1600);
        let b = n.header(12, 1920);
        assert!(a.marker && !b.marker);
        assert_eq!(b.seq.wrapping_sub(a.seq), 2);
        assert_eq!(b.ts.wrapping_sub(a.ts), 320);
        assert_eq!(a.ssrc, b.ssrc);
    }

    #[test]
    fn g711_bit_a_bit() {
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(encode_payload(Codec::Alaw, &decode_payload(Codec::Alaw, &all)), all);
        // en loi µ, 0x7F (« −0 ») redevient 0xFF (« +0 ») : seul doublon
        let mu: Vec<u8> = (0..=255).filter(|&b| b != 0x7F).collect();
        assert_eq!(encode_payload(Codec::Ulaw, &decode_payload(Codec::Ulaw, &mu)), mu);
    }
}
