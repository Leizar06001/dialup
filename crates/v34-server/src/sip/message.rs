//! Messages SIP (RFC 3261) : analyse et construction.
//!
//! Juste ce qu'il faut pour un adaptateur téléphonique : en-têtes
//! insensibles à la casse, formes compactes (`v`, `f`, `t`, `i`, `m`,
//! `l`, `c`, `k`), lignes de continuation, corps limité par
//! `Content-Length`.

use std::fmt::Write as _;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Start {
    Request { method: String, uri: String },
    Response { code: u16, reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub start: Start,
    /// (nom canonique, valeur), dans l'ordre
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Le datagramme ressemble-t-il à du SIP (méthode en majuscules ou réponse) ?
pub fn looks_like_sip(buf: &[u8]) -> bool {
    let n = buf.iter().take(16).take_while(|b| b.is_ascii_uppercase()).count();
    buf.starts_with(b"SIP/2.0 ") || (n >= 3 && buf.get(n) == Some(&b' '))
}

/// Nom canonique d'un en-tête (formes compactes développées).
pub fn canonical(name: &str) -> String {
    const CONNUS: [&str; 22] = [
        "Via",
        "From",
        "To",
        "Call-ID",
        "CSeq",
        "Contact",
        "Content-Length",
        "Content-Type",
        "Max-Forwards",
        "Expires",
        "Authorization",
        "Proxy-Authorization",
        "WWW-Authenticate",
        "Proxy-Authenticate",
        "User-Agent",
        "Server",
        "Allow",
        "Supported",
        "Require",
        "Record-Route",
        "Route",
        "Event",
    ];
    let full = match name.to_ascii_lowercase().as_str() {
        "v" => "Via",
        "f" => "From",
        "t" => "To",
        "i" => "Call-ID",
        "m" => "Contact",
        "l" => "Content-Length",
        "c" => "Content-Type",
        "k" => "Supported",
        "o" => "Event",
        _ => return CONNUS.iter().find(|c| c.eq_ignore_ascii_case(name)).map_or_else(|| name.to_string(), |c| c.to_string()),
    };
    full.to_string()
}

impl Message {
    pub fn request(method: &str, uri: &str) -> Message {
        Message { start: Start::Request { method: method.into(), uri: uri.into() }, headers: Vec::new(), body: Vec::new() }
    }

    pub fn response(code: u16) -> Message {
        Message { start: Start::Response { code, reason: reason(code).into() }, headers: Vec::new(), body: Vec::new() }
    }

    /// Analyse un datagramme ; `None` s'il n'est pas un message SIP valide.
    pub fn parse(buf: &[u8]) -> Option<Message> {
        let (head, body) = match find(buf, b"\r\n\r\n") {
            Some(i) => (&buf[..i], &buf[i + 4..]),
            None => match find(buf, b"\n\n") {
                Some(i) => (&buf[..i], &buf[i + 2..]),
                None => (buf, &[][..]),
            },
        };
        let head = std::str::from_utf8(head).ok()?;
        let mut lines = head.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l));
        let first = lines.next()?.trim();
        let start = if let Some(rest) = first.strip_prefix("SIP/2.0 ") {
            let (code, reason) = rest.split_once(' ').unwrap_or((rest, ""));
            let code: u16 = code.parse().ok().filter(|c| (100..700).contains(c))?;
            Start::Response { code, reason: reason.to_string() }
        } else {
            let mut it = first.split(' ');
            let (method, uri, version) = (it.next()?, it.next()?, it.next()?);
            if version != "SIP/2.0" || method.is_empty() || !method.bytes().all(|b| b.is_ascii_uppercase()) {
                return None;
            }
            Start::Request { method: method.to_string(), uri: uri.to_string() }
        };
        let mut headers: Vec<(String, String)> = Vec::new();
        for line in lines {
            if line.starts_with([' ', '\t']) {
                // continuation de l'en-tête précédent
                let (_, v) = headers.last_mut()?;
                v.push(' ');
                v.push_str(line.trim());
                continue;
            }
            if line.trim().is_empty() {
                continue;
            }
            let (name, value) = line.split_once(':')?;
            headers.push((canonical(name.trim()), value.trim().to_string()));
        }
        let mut msg = Message { start, headers, body: Vec::new() };
        msg.body = match msg.header("Content-Length") {
            Some(l) => {
                let n: usize = l.parse().ok()?;
                body.get(..n)?.to_vec()
            }
            None => body.to_vec(),
        };
        Some(msg)
    }

    /// Premier en-tête de ce nom (insensible à la casse, formes compactes).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers(name).next()
    }

    pub fn headers<'a>(&'a self, name: &str) -> impl Iterator<Item = &'a str> + 'a {
        let name = canonical(name);
        self.headers.iter().filter(move |(n, _)| n.eq_ignore_ascii_case(&name)).map(|(_, v)| v.as_str())
    }

    pub fn push(&mut self, name: &str, value: impl Into<String>) -> &mut Self {
        self.headers.push((name.to_string(), value.into()));
        self
    }

    pub fn method(&self) -> Option<&str> {
        match &self.start {
            Start::Request { method, .. } => Some(method),
            Start::Response { .. } => None,
        }
    }

    pub fn code(&self) -> Option<u16> {
        match self.start {
            Start::Response { code, .. } => Some(code),
            Start::Request { .. } => None,
        }
    }

    pub fn uri(&self) -> Option<&str> {
        match &self.start {
            Start::Request { uri, .. } => Some(uri),
            Start::Response { .. } => None,
        }
    }

    pub fn call_id(&self) -> Option<&str> {
        self.header("Call-ID")
    }

    /// (numéro, méthode) de l'en-tête CSeq.
    pub fn cseq(&self) -> Option<(u32, &str)> {
        let (n, m) = self.header("CSeq")?.split_once(char::is_whitespace)?;
        Some((n.trim().parse().ok()?, m.trim()))
    }

    /// Paramètre `branch` du premier Via.
    pub fn branch(&self) -> Option<&str> {
        param(first_value(self.header("Via")?), "branch")
    }

    pub fn from_tag(&self) -> Option<&str> {
        param(self.header("From")?, "tag")
    }

    pub fn to_tag(&self) -> Option<&str> {
        param(self.header("To")?, "tag")
    }

    pub fn body_str(&self) -> &str {
        std::str::from_utf8(&self.body).unwrap_or("")
    }

    /// Texte du message (Content-Length recalculé).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut s = String::with_capacity(512 + self.body.len());
        match &self.start {
            Start::Request { method, uri } => {
                let _ = write!(s, "{method} {uri} SIP/2.0\r\n");
            }
            Start::Response { code, reason } => {
                let _ = write!(s, "SIP/2.0 {code} {reason}\r\n");
            }
        }
        for (n, v) in &self.headers {
            if !n.eq_ignore_ascii_case("Content-Length") {
                let _ = write!(s, "{n}: {v}\r\n");
            }
        }
        let _ = write!(s, "Content-Length: {}\r\n\r\n", self.body.len());
        let mut out = s.into_bytes();
        out.extend_from_slice(&self.body);
        out
    }

    /// Première ligne, pour le journal.
    pub fn summary(&self) -> String {
        match &self.start {
            Start::Request { method, uri } => format!("{method} {uri}"),
            Start::Response { code, reason } => {
                let m = self.cseq().map(|(_, m)| m).unwrap_or("?");
                format!("{code} {reason} ({m})")
            }
        }
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Première valeur d'un en-tête à valeurs multiples (`Via: a, b`).
pub fn first_value(v: &str) -> &str {
    let mut quoted = false;
    for (i, c) in v.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => return v[..i].trim(),
            _ => {}
        }
    }
    v.trim()
}

/// URI d'un en-tête `"Nom" <sip:…>;params` (ou `sip:…;params` sans chevrons).
pub fn addr_uri(v: &str) -> &str {
    let v = v.trim();
    match (v.find('<'), v.find('>')) {
        (Some(a), Some(b)) if b > a => &v[a + 1..b],
        _ => v.split(';').next().unwrap_or(v).trim(),
    }
}

/// Paramètre d'en-tête (`;tag=…`, `;branch=…`, `;expires=…`), hors URI.
/// Sans valeur (`;rport`), renvoie `Some("")`.
pub fn param<'a>(v: &'a str, name: &str) -> Option<&'a str> {
    let rest = match v.find('>') {
        Some(i) if v.contains('<') => &v[i + 1..],
        _ => v.split_once(';').map_or("", |(_, r)| r),
    };
    rest.split(';').map(str::trim).find_map(|p| {
        let (k, val) = p.split_once('=').unwrap_or((p, ""));
        k.trim().eq_ignore_ascii_case(name).then(|| val.trim().trim_matches('"'))
    })
}

/// Partie « utilisateur » d'une URI SIP (`sip:0142424242@hôte;user=phone`).
pub fn uri_user(uri: &str) -> Option<&str> {
    let rest = uri.strip_prefix("sips:").or_else(|| uri.strip_prefix("sip:")).or_else(|| uri.strip_prefix("tel:"))?;
    let user = match rest.split_once('@') {
        Some((u, _)) => u,
        None if uri.starts_with("tel:") => rest,
        None => return None,
    };
    let user = user.split([';', ':']).next().unwrap_or(user);
    (!user.is_empty()).then_some(user)
}

/// Phrase standard d'un code de réponse.
pub fn reason(code: u16) -> &'static str {
    match code {
        100 => "Trying",
        180 => "Ringing",
        183 => "Session Progress",
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        407 => "Proxy Authentication Required",
        408 => "Request Timeout",
        420 => "Bad Extension",
        480 => "Temporarily Unavailable",
        481 => "Call/Transaction Does Not Exist",
        486 => "Busy Here",
        487 => "Request Terminated",
        488 => "Not Acceptable Here",
        491 => "Request Pending",
        500 => "Server Internal Error",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        603 => "Decline",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INVITE: &str = "INVITE sip:0800000000@dialup.example.org:3434 SIP/2.0\r\n\
v: SIP/2.0/UDP 192.168.1.20:5060;branch=z9hG4bK-1234;rport\r\n\
From: \"Ligne 1\" <sip:ata1@dialup.example.org>;tag=abc\r\n\
t: <sip:0800000000@dialup.example.org>\r\n\
i: 5f3a@192.168.1.20\r\n\
CSeq: 101 INVITE\r\n\
Subject: essai\r\n\x20sur deux lignes\r\n\
c: application/sdp\r\n\
l: 9\r\n\
\r\n\
v=0\r\no=xyz";

    #[test]
    fn analyse_formes_compactes_et_corps() {
        assert!(looks_like_sip(INVITE.as_bytes()));
        let m = Message::parse(INVITE.as_bytes()).unwrap();
        assert_eq!(m.method(), Some("INVITE"));
        assert_eq!(m.uri(), Some("sip:0800000000@dialup.example.org:3434"));
        assert_eq!(m.call_id(), Some("5f3a@192.168.1.20"));
        assert_eq!(m.cseq(), Some((101, "INVITE")));
        assert_eq!(m.branch(), Some("z9hG4bK-1234"));
        assert_eq!(m.from_tag(), Some("abc"));
        assert_eq!(m.to_tag(), None);
        assert_eq!(m.header("subject"), Some("essai sur deux lignes"));
        assert_eq!(m.header("Content-Type"), Some("application/sdp"));
        assert_eq!(m.body_str(), "v=0\r\no=xy", "corps limité par Content-Length");
        assert_eq!(param(m.header("Via").unwrap(), "rport"), Some(""));
        assert_eq!(uri_user(m.uri().unwrap()), Some("0800000000"));
        assert_eq!(uri_user(addr_uri(m.header("From").unwrap())), Some("ata1"));
    }

    #[test]
    fn reponse_et_aller_retour() {
        let mut r = Message::response(486);
        r.push("Via", "SIP/2.0/UDP 10.0.0.1:5060;branch=z9hG4bKx").push("CSeq", "7 INVITE");
        r.body = b"x".to_vec();
        let b = r.to_bytes();
        assert!(b.starts_with(b"SIP/2.0 486 Busy Here\r\n"));
        let m = Message::parse(&b).unwrap();
        assert_eq!(m.code(), Some(486));
        assert_eq!(m.cseq(), Some((7, "INVITE")));
        assert_eq!(m.body, b"x");
        assert_eq!(m.summary(), "486 Busy Here (INVITE)");
    }

    #[test]
    fn refus_et_valeurs() {
        assert!(Message::parse(b"V3\x01\x08").is_none());
        assert!(!looks_like_sip(b"V3\x01\x08\0\0\0\0"));
        assert!(!looks_like_sip(&[0x80, 8, 0, 1]));
        assert!(Message::parse(b"INVITE sip:x SIP/1.0\r\n\r\n").is_none());
        assert!(Message::parse(b"SIP/2.0 999 Bof\r\n\r\n").is_none());
        assert!(Message::parse(b"OPTIONS sip:x SIP/2.0\r\nContent-Length: 50\r\n\r\ncourt").is_none(), "corps tronqué");
        assert_eq!(first_value("SIP/2.0/UDP a;branch=1, SIP/2.0/UDP b"), "SIP/2.0/UDP a;branch=1");
        assert_eq!(addr_uri("<sip:a@b:5060;transport=udp>;expires=60"), "sip:a@b:5060;transport=udp");
        assert_eq!(param("<sip:a@b;expires=1>;expires=60", "expires"), Some("60"));
        assert_eq!(param("sip:a@b;tag=9", "tag"), Some("9"));
        assert_eq!(uri_user("tel:0142424242"), Some("0142424242"));
        assert_eq!(uri_user("sip:example.org"), None);
    }
}
