//! Protocole applicatif « BBS » transporté par le flux fiable du modem :
//! chat et transferts de fichiers multiplexés.
//!
//! Trame : `A5 | type | longueur (u16 LE) | charge`. Les octets qui ne sont
//! pas dans une trame sont du texte brut (ex. l'invite « login: » d'un
//! serveur à l'ancienne) et sont affichés tels quels.
//!
//! Types : 1 chat, 2 à 5 fichier (offre, morceau, fin, abandon), 6 image
//! du visiophone ([`crate::visio`]). Les types 7 à 15 sont réservés aux
//! versions futures : une trame bien formée d'un type inconnu est ignorée
//! sans bruit.
//!
//! Compatibilité : les applis d'avant le visiophone ne connaissent que les
//! types 1 à 5 et afficheraient une trame de type 6 comme du texte (des
//! hiéroglyphes dans le chat). L'annonce du visiophone se déguise donc en
//! « abandon du fichier n° 0 », qui n'existe jamais (les numéros commencent
//! à 1) : une vieille appli l'ignore poliment, sans répondre. Les images ne
//! partent que vers un correspondant qui s'est annoncé à son tour.

use std::path::{Path, PathBuf};
use std::time::Instant;

const MAGIC: u8 = 0xA5;
const MAX_PAYLOAD: usize = 2048;
/// Dernier type connu de cette version
const LAST_KNOWN: u8 = 6;
/// Types acceptés comme trames (au-delà de `LAST_KNOWN` : ignorées)
const LAST_TYPE: u8 = 15;
/// Annonce du visiophone, après l'identifiant de fichier 0 d'un « abandon »
const VISIO_TAG: &[u8] = b"VISIO";
/// Version du visiophone annoncée
const VISIO_VERSION: u8 = 1;
/// Taille des morceaux de fichier
pub const CHUNK: usize = 512;

#[derive(Clone, Debug, PartialEq)]
pub enum Frame {
    Chat(String),
    FileOffer { id: u32, size: u64, name: String },
    FileData { id: u32, data: Vec<u8> },
    FileEnd { id: u32, crc: u32 },
    FileAbort { id: u32 },
    /// Visiophone : « je sais recevoir des images », caméra allumée ou non
    /// (annonce, puis à chaque changement)
    Visio { camera: bool },
    /// Visiophone : une image au format de [`crate::visio`]
    Video(Vec<u8>),
}

#[derive(Debug, PartialEq)]
pub enum Item {
    Frame(Frame),
    Raw(Vec<u8>),
}

pub fn encode(f: &Frame) -> Vec<u8> {
    let (t, payload): (u8, Vec<u8>) = match f {
        Frame::Chat(s) => (1, s.as_bytes().to_vec()),
        Frame::FileOffer { id, size, name } => {
            let mut p = id.to_le_bytes().to_vec();
            p.extend(size.to_le_bytes());
            p.extend(name.as_bytes());
            (2, p)
        }
        Frame::FileData { id, data } => {
            let mut p = id.to_le_bytes().to_vec();
            p.extend(data);
            (3, p)
        }
        Frame::FileEnd { id, crc } => {
            let mut p = id.to_le_bytes().to_vec();
            p.extend(crc.to_le_bytes());
            (4, p)
        }
        Frame::FileAbort { id } => (5, id.to_le_bytes().to_vec()),
        Frame::Visio { camera } => {
            let mut p = 0u32.to_le_bytes().to_vec();
            p.extend(VISIO_TAG);
            p.extend([VISIO_VERSION, *camera as u8]);
            (5, p)
        }
        Frame::Video(data) => (6, data.clone()),
    };
    let mut out = vec![MAGIC, t];
    out.extend((payload.len() as u16).to_le_bytes());
    out.extend(payload);
    out
}

fn decode_frame(t: u8, p: &[u8]) -> Option<Frame> {
    let id = |p: &[u8]| -> Option<u32> { Some(u32::from_le_bytes(p.get(..4)?.try_into().ok()?)) };
    Some(match t {
        1 => Frame::Chat(String::from_utf8_lossy(p).into_owned()),
        2 => Frame::FileOffer { id: id(p)?, size: u64::from_le_bytes(p.get(4..12)?.try_into().ok()?), name: String::from_utf8_lossy(p.get(12..)?).into_owned() },
        3 => Frame::FileData { id: id(p)?, data: p.get(4..)?.to_vec() },
        4 => Frame::FileEnd { id: id(p)?, crc: u32::from_le_bytes(p.get(4..8)?.try_into().ok()?) },
        5 => match (id(p)?, p.get(4..4 + VISIO_TAG.len())) {
            (0, Some(VISIO_TAG)) => Frame::Visio { camera: p.get(4 + VISIO_TAG.len() + 1).is_some_and(|f| f & 1 != 0) },
            (id, _) => Frame::FileAbort { id },
        },
        6 => Frame::Video(p.to_vec()),
        _ => return None,
    })
}

/// Découpe le flux reçu en trames et en texte brut.
#[derive(Default)]
pub struct Decoder {
    buf: Vec<u8>,
}

impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Item> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        let mut raw = Vec::new();
        loop {
            if self.buf.is_empty() {
                break;
            }
            if self.buf[0] != MAGIC {
                let n = self.buf.iter().position(|&b| b == MAGIC).unwrap_or(self.buf.len());
                raw.extend(self.buf.drain(..n));
                continue;
            }
            if self.buf.len() < 4 {
                break; // en-tête incomplet : on attend la suite
            }
            let t = self.buf[1];
            let len = u16::from_le_bytes([self.buf[2], self.buf[3]]) as usize;
            if !(1..=LAST_TYPE).contains(&t) || len > MAX_PAYLOAD {
                // Pas une trame : ce A5 est du texte brut
                raw.push(self.buf.remove(0));
                continue;
            }
            if self.buf.len() < 4 + len {
                break;
            }
            let frame: Vec<u8> = self.buf.drain(..4 + len).collect();
            if !raw.is_empty() {
                out.push(Item::Raw(std::mem::take(&mut raw)));
            }
            match decode_frame(t, &frame[4..]) {
                Some(f) => out.push(Item::Frame(f)),
                // type d'une version plus récente : ignoré sans bruit
                None if t > LAST_KNOWN => {}
                None => raw.extend(frame),
            }
        }
        if !raw.is_empty() {
            out.push(Item::Raw(raw));
        }
        out
    }
}

/// CRC-32 (IEEE 802.3), pour vérifier les fichiers reçus.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// Envoi de fichier en cours. La progression vient des octets réellement
/// acquittés par la couche de correction d'erreurs de l'autre modem.
pub struct Outgoing {
    pub name: String,
    pub size: u64,
    /// Position dans le flux émis : début et fin des trames de ce fichier
    pub stream_start: u64,
    pub stream_end: u64,
    pub started: Instant,
}

impl Outgoing {
    /// Fraction acquittée (0..1) d'après le nombre d'octets acquittés.
    pub fn progress(&self, acked: u64) -> f64 {
        let span = (self.stream_end - self.stream_start).max(1) as f64;
        ((acked.saturating_sub(self.stream_start)) as f64 / span).clamp(0.0, 1.0)
    }
}

/// Réception de fichier en cours.
pub struct Incoming {
    pub id: u32,
    pub name: String,
    pub size: u64,
    pub data: Vec<u8>,
    pub started: Instant,
}

/// Prépare les trames d'un fichier.
pub fn file_frames(id: u32, name: &str, data: &[u8]) -> Vec<u8> {
    let mut out = encode(&Frame::FileOffer { id, size: data.len() as u64, name: name.to_string() });
    for chunk in data.chunks(CHUNK) {
        out.extend(encode(&Frame::FileData { id, data: chunk.to_vec() }));
    }
    out.extend(encode(&Frame::FileEnd { id, crc: crc32(data) }));
    out
}

/// Nom de fichier sûr (pas de chemin, pas de caractères spéciaux).
pub fn safe_name(name: &str) -> String {
    let base = Path::new(name).file_name().and_then(|s| s.to_str()).unwrap_or("fichier");
    let cleaned: String = base.chars().map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { '_' } else { c }).collect();
    let cleaned = cleaned.trim_start_matches('.').to_string();
    if cleaned.is_empty() { "fichier".into() } else { cleaned }
}

/// Enregistre un fichier reçu dans `dir` sans écraser un fichier existant.
pub fn save_unique(dir: &Path, name: &str, data: &[u8]) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let name = safe_name(name);
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (name[..i].to_string(), name[i..].to_string()),
        _ => (name.clone(), String::new()),
    };
    let mut path = dir.join(&name);
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    std::fs::write(&path, data)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_roundtrip_through_arbitrary_chunks() {
        let data: Vec<u8> = (0..3000u32).map(|i| (i * 7 + 0xA5) as u8).collect();
        let mut stream = b"login: ".to_vec();
        stream.extend(encode(&Frame::Chat("salut ✓".into())));
        stream.extend(file_frames(7, "photo.jpg", &data));
        stream.extend(b"\xA5 texte brut");
        let mut d = Decoder::default();
        let mut items = Vec::new();
        for c in stream.chunks(13) {
            items.extend(d.push(c));
        }
        assert_eq!(items[0], Item::Raw(b"login: ".to_vec()));
        assert_eq!(items[1], Item::Frame(Frame::Chat("salut ✓".into())));
        let mut got = Vec::new();
        let mut crc_ok = false;
        for it in &items {
            match it {
                Item::Frame(Frame::FileData { data, .. }) => got.extend(data),
                Item::Frame(Frame::FileEnd { crc, .. }) => crc_ok = *crc == crc32(&got),
                _ => {}
            }
        }
        assert_eq!(got, data);
        assert!(crc_ok);
        let raw: Vec<u8> = items.iter().filter_map(|i| if let Item::Raw(r) = i { Some(r.clone()) } else { None }).flatten().collect();
        assert!(raw.ends_with(b"texte brut"));
    }

    #[test]
    fn visiophone_et_types_inconnus() {
        let img: Vec<u8> = (0..300u32).map(|i| (i * 13) as u8).collect();
        let mut stream = encode(&Frame::Visio { camera: true });
        stream.extend(encode(&Frame::Video(img.clone())));
        // trame d'une version future (type 9) : ignorée sans bruit
        stream.extend([MAGIC, 9, 3, 0, 1, 2, 3]);
        stream.extend(encode(&Frame::Visio { camera: false }));
        stream.extend(encode(&Frame::FileAbort { id: 0 }));
        stream.extend(encode(&Frame::Chat("toujours là".into())));
        let mut d = Decoder::default();
        let items: Vec<Item> = stream.chunks(7).flat_map(|c| d.push(c)).collect();
        assert_eq!(
            items,
            vec![
                Item::Frame(Frame::Visio { camera: true }),
                Item::Frame(Frame::Video(img)),
                Item::Frame(Frame::Visio { camera: false }),
                Item::Frame(Frame::FileAbort { id: 0 }),
                Item::Frame(Frame::Chat("toujours là".into())),
            ]
        );
        // Une appli d'avant le visiophone lit l'annonce comme un abandon du
        // fichier n° 0 (type 5, identifiant nul), qu'elle ignore.
        let a = encode(&Frame::Visio { camera: true });
        assert_eq!((a[1], u16::from_le_bytes([a[2], a[3]]) as usize, &a[4..8]), (5, a.len() - 4, &[0u8; 4][..]));
    }

    #[test]
    fn crc32_reference() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn names_are_sanitized() {
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name("a:b?.txt"), "a_b_.txt");
        assert_eq!(safe_name(".bashrc"), "bashrc");
    }
}
