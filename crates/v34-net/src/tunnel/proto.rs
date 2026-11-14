//! Protocole du tunnel multiplexé transporté par le flux d'octets du modem.
//!
//! ## Trame
//! ```text
//! | 0xA5 | type u8 | seq u8 | flux u16 | long. u16 | charge (≤ 1024 o) | CRC-16 u16 |
//! ```
//! - `0xA5` : octet de synchronisation (permet de se recaler après une erreur).
//! - `seq` : compteur de trames par sens (+1 à chaque trame, modulo 256).
//!   Un trou signale une trame perdue : tous les flux sont alors fermés
//!   (mieux vaut une connexion coupée qu'un flux TCP silencieusement corrompu).
//! - CRC-16/CCITT-FALSE (poly 0x1021, init 0xFFFF) sur type…charge.
//!
//! Le flux du modem V.34 est normalement fiabilisé par la couche LAPM de
//! `v34-core` ; la synchro + CRC + seq ne sont qu'un filet de sécurité.
//!
//! ## Types
//! | type | nom       | sens | flux | charge                                   |
//! |------|-----------|------|------|------------------------------------------|
//! | 0x01 | HELLO     | ↔    | 0    | version u8, nb max de flux u16           |
//! | 0x02 | OPEN      | C→S  | id   | port u16, hôte (UTF-8, ≤ 255 o)          |
//! | 0x03 | OPEN_OK   | S→C  | id   | —                                        |
//! | 0x04 | OPEN_FAIL | S→C  | id   | code u8 (codes SOCKS5), message UTF-8    |
//! | 0x05 | DATA      | ↔    | id   | octets                                   |
//! | 0x06 | CLOSE     | ↔    | id   | — (fermeture complète, pas de demi-fermeture) |
//!
//! ## Avant le tunnel : phase texte
//! Après CONNECT, le serveur envoie du texte (bannière, bienvenue) puis
//! [`TUNNEL_MARKER`] ; tout ce qui suit est en trames. `v34-server` ne
//! demande pas de login (l'abonné est déjà authentifié par le central) ;
//! le client sait tout de même répondre à des invites `login: ` /
//! `password: ` (lignes terminées par `\r`) pour un autre serveur, et
//! abandonne sur `Login incorrect`.

pub const SYNC: u8 = 0xA5;
pub const VERSION: u8 = 1;
pub const HEADER_LEN: usize = 7;
pub const CRC_LEN: usize = 2;
/// Charge maximale acceptée.
pub const MAX_PAYLOAD: usize = 1024;
/// Taille des morceaux DATA émis : petite, pour que les flux s'entrelacent
/// correctement sur une ligne à 4 ko/s.
pub const DATA_CHUNK: usize = 256;

pub const LOGIN_PROMPT: &str = "login: ";
pub const PASSWORD_PROMPT: &str = "password: ";
pub const LOGIN_FAILED: &str = "Login incorrect";
/// Fin de la phase texte : la suite du flux est en trames.
pub const TUNNEL_MARKER: &str = "\r\nTUNNEL\r\n";

const T_HELLO: u8 = 0x01;
const T_OPEN: u8 = 0x02;
const T_OPEN_OK: u8 = 0x03;
const T_OPEN_FAIL: u8 = 0x04;
const T_DATA: u8 = 0x05;
const T_CLOSE: u8 = 0x06;

/// Cause d'un échec d'ouverture (valeurs = codes de réponse SOCKS5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailCode {
    General = 1,
    NotAllowed = 2,
    NetUnreachable = 3,
    HostUnreachable = 4,
    Refused = 5,
    Timeout = 6,
}

impl FailCode {
    pub fn from_u8(c: u8) -> FailCode {
        match c {
            2 => FailCode::NotAllowed,
            3 => FailCode::NetUnreachable,
            4 => FailCode::HostUnreachable,
            5 => FailCode::Refused,
            6 => FailCode::Timeout,
            _ => FailCode::General,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frame {
    Hello { version: u8, max_streams: u16 },
    Open { id: u16, host: String, port: u16 },
    OpenOk { id: u16 },
    OpenFail { id: u16, code: FailCode, msg: String },
    Data { id: u16, data: Vec<u8> },
    Close { id: u16 },
}

/// CRC-16/CCITT-FALSE.
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

/// Encodeur (tient le compteur de séquence de notre sens).
#[derive(Default)]
pub struct Encoder {
    seq: u8,
}

impl Encoder {
    pub fn encode(&mut self, frame: &Frame) -> Vec<u8> {
        let (t, id, payload): (u8, u16, Vec<u8>) = match frame {
            Frame::Hello { version, max_streams } => {
                let mut p = vec![*version];
                p.extend_from_slice(&max_streams.to_be_bytes());
                (T_HELLO, 0, p)
            }
            Frame::Open { id, host, port } => {
                let mut p = port.to_be_bytes().to_vec();
                let h = host.as_bytes();
                p.extend_from_slice(&h[..h.len().min(255)]);
                (T_OPEN, *id, p)
            }
            Frame::OpenOk { id } => (T_OPEN_OK, *id, Vec::new()),
            Frame::OpenFail { id, code, msg } => {
                let mut p = vec![*code as u8];
                let m = msg.as_bytes();
                p.extend_from_slice(&m[..m.len().min(200)]);
                (T_OPEN_FAIL, *id, p)
            }
            Frame::Data { id, data } => {
                debug_assert!(data.len() <= MAX_PAYLOAD);
                (T_DATA, *id, data[..data.len().min(MAX_PAYLOAD)].to_vec())
            }
            Frame::Close { id } => (T_CLOSE, *id, Vec::new()),
        };
        if t == T_HELLO {
            self.seq = 0; // HELLO (re)démarre la numérotation
        }
        let mut out = Vec::with_capacity(HEADER_LEN + payload.len() + CRC_LEN);
        out.push(SYNC);
        out.push(t);
        out.push(self.seq);
        out.extend_from_slice(&id.to_be_bytes());
        out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        out.extend_from_slice(&payload);
        let crc = crc16(&out[1..]);
        out.extend_from_slice(&crc.to_be_bytes());
        self.seq = self.seq.wrapping_add(1);
        out
    }
}

/// Résultat du décodeur.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decoded {
    Frame(Frame),
    /// Données corrompues ou trame perdue : les flux doivent être réinitialisés.
    Loss,
}

/// Décodeur en flux, avec resynchronisation.
#[derive(Default)]
pub struct Decoder {
    buf: Vec<u8>,
    expect: Option<u8>,
    pending: Option<Frame>,
    /// Octets ignorés en cherchant la synchro
    pub skipped: u64,
    /// Trames rejetées (CRC) ou manquantes (trou de séquence)
    pub losses: u64,
}

impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Prochaine trame décodée, `None` s'il faut plus d'octets.
    pub fn pop(&mut self) -> Option<Decoded> {
        if let Some(f) = self.pending.take() {
            return Some(Decoded::Frame(f));
        }
        let mut lost = false;
        loop {
            // recherche de la synchro
            match self.buf.iter().position(|&b| b == SYNC) {
                Some(0) => {}
                Some(i) => {
                    self.skipped += i as u64;
                    self.buf.drain(..i);
                    lost = true;
                }
                None => {
                    if !self.buf.is_empty() {
                        self.skipped += self.buf.len() as u64;
                        self.buf.clear();
                        lost = true;
                    }
                    return self.loss_or_none(lost);
                }
            }
            if self.buf.len() < HEADER_LEN {
                return self.loss_or_none(lost);
            }
            let len = u16::from_be_bytes([self.buf[5], self.buf[6]]) as usize;
            if len > MAX_PAYLOAD {
                self.buf.drain(..1);
                self.skipped += 1;
                lost = true;
                continue;
            }
            let total = HEADER_LEN + len + CRC_LEN;
            if self.buf.len() < total {
                return self.loss_or_none(lost);
            }
            let crc = u16::from_be_bytes([self.buf[total - 2], self.buf[total - 1]]);
            if crc16(&self.buf[1..total - 2]) != crc {
                self.buf.drain(..1);
                self.skipped += 1;
                lost = true;
                continue;
            }
            let t = self.buf[1];
            let seq = self.buf[2];
            let id = u16::from_be_bytes([self.buf[3], self.buf[4]]);
            let frame = parse(t, id, &self.buf[HEADER_LEN..HEADER_LEN + len]);
            self.buf.drain(..total);
            if t == T_HELLO {
                self.expect = Some(seq.wrapping_add(1));
            } else {
                if let Some(e) = self.expect {
                    if e != seq {
                        lost = true;
                    }
                }
                self.expect = Some(seq.wrapping_add(1));
            }
            let Some(frame) = frame else {
                // type inconnu (version future) : ignoré
                continue;
            };
            if lost {
                self.losses += 1;
                self.pending = Some(frame);
                return Some(Decoded::Loss);
            }
            return Some(Decoded::Frame(frame));
        }
    }

    fn loss_or_none(&mut self, lost: bool) -> Option<Decoded> {
        if lost {
            self.losses += 1;
            Some(Decoded::Loss)
        } else {
            None
        }
    }
}

fn parse(t: u8, id: u16, p: &[u8]) -> Option<Frame> {
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    Some(match t {
        T_HELLO if p.len() >= 3 => Frame::Hello { version: p[0], max_streams: u16::from_be_bytes([p[1], p[2]]) },
        T_OPEN if p.len() >= 3 => Frame::Open { id, port: u16::from_be_bytes([p[0], p[1]]), host: text(&p[2..]) },
        T_OPEN_OK => Frame::OpenOk { id },
        T_OPEN_FAIL if !p.is_empty() => Frame::OpenFail { id, code: FailCode::from_u8(p[0]), msg: text(&p[1..]) },
        T_DATA => Frame::Data { id, data: p.to_vec() },
        T_CLOSE => Frame::Close { id },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames() -> Vec<Frame> {
        vec![
            Frame::Hello { version: VERSION, max_streams: 16 },
            Frame::Open { id: 1, host: "example.org".into(), port: 443 },
            Frame::OpenOk { id: 1 },
            Frame::OpenFail { id: 3, code: FailCode::NotAllowed, msg: "port interdit".into() },
            Frame::Data { id: 1, data: (0..=255u8).collect() },
            Frame::Data { id: 2, data: vec![SYNC; 40] },
            Frame::Close { id: 1 },
        ]
    }

    #[test]
    fn crc_reference() {
        assert_eq!(crc16(b"123456789"), 0x29B1);
    }

    #[test]
    fn aller_retour_octet_par_octet() {
        let mut enc = Encoder::default();
        let bytes: Vec<u8> = frames().iter().flat_map(|f| enc.encode(f)).collect();
        let mut dec = Decoder::default();
        let mut got = Vec::new();
        for b in bytes {
            dec.push(&[b]);
            while let Some(d) = dec.pop() {
                got.push(d);
            }
        }
        let expected: Vec<Decoded> = frames().into_iter().map(Decoded::Frame).collect();
        assert_eq!(got, expected);
        assert_eq!(dec.losses, 0);
    }

    #[test]
    fn corruption_detectee_et_resynchro() {
        let mut enc = Encoder::default();
        let f = frames();
        let mut bytes = Vec::new();
        bytes.extend(enc.encode(&f[0]));
        let mut bad = enc.encode(&f[4]);
        bad[20] ^= 0x10; // erreur dans la charge
        bytes.extend(bad);
        bytes.extend(enc.encode(&f[6]));
        let mut dec = Decoder::default();
        dec.push(&bytes);
        let mut got = Vec::new();
        while let Some(d) = dec.pop() {
            got.push(d);
        }
        assert_eq!(got.first(), Some(&Decoded::Frame(f[0].clone())));
        assert!(got.contains(&Decoded::Loss));
        assert_eq!(got.last(), Some(&Decoded::Frame(f[6].clone())), "resynchronisé sur la trame suivante");
    }

    #[test]
    fn trame_perdue_detectee() {
        let mut enc = Encoder::default();
        let f = frames();
        let a = enc.encode(&f[0]);
        let _perdue = enc.encode(&f[2]);
        let c = enc.encode(&f[6]);
        let mut dec = Decoder::default();
        dec.push(&a);
        dec.push(&c);
        assert_eq!(dec.pop(), Some(Decoded::Frame(f[0].clone())));
        assert_eq!(dec.pop(), Some(Decoded::Loss));
        assert_eq!(dec.pop(), Some(Decoded::Frame(f[6].clone())));
        assert_eq!(dec.pop(), None);
    }

    #[test]
    fn longueur_absurde_ignoree() {
        let mut dec = Decoder::default();
        dec.push(&[SYNC, T_DATA, 0, 0, 1, 0xFF, 0xFF]);
        assert_eq!(dec.pop(), Some(Decoded::Loss));
        assert_eq!(dec.pop(), None);
    }
}
