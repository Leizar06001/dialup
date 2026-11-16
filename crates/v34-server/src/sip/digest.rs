//! Authentification « digest » de SIP (RFC 3261 §22, RFC 2617) en MD5.
//!
//! Le serveur envoie un défi (`WWW-Authenticate` ou `Proxy-Authenticate`)
//! avec un nonce ; le poste répond (`Authorization` / `Proxy-Authorization`)
//! par MD5(HA1:nonce:…:HA2), où HA1 = MD5(nom:domaine:mot de passe). Le
//! central ne garde que HA1 (l'« empreinte » du poste) : elle ne révèle
//! pas le mot de passe, mais suffit à s'authentifier en SIP. MD5 est
//! imposé par les adaptateurs : c'est le seul algorithme qu'ils
//! connaissent tous.

use md5::{Digest, Md5};

use v34_net::crypto::ct_eq;

/// MD5 de la concaténation des morceaux, en hexadécimal minuscule.
pub fn md5_hex(parts: &[&[u8]]) -> String {
    let mut h = Md5::new();
    for p in parts {
        h.update(p);
    }
    hex(&h.finalize())
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// HA1 = MD5(nom:domaine:mot de passe).
pub fn ha1(user: &str, realm: &str, password: &str) -> String {
    md5_hex(&[user.as_bytes(), b":", realm.as_bytes(), b":", password.as_bytes()])
}

/// Réponse d'un poste à un défi.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Credentials {
    pub username: String,
    pub realm: String,
    pub nonce: String,
    pub uri: String,
    pub response: String,
    pub algorithm: Option<String>,
    pub qop: Option<String>,
    pub nc: Option<String>,
    pub cnonce: Option<String>,
}

/// Analyse un en-tête `Authorization: Digest k="v", k=v, …`.
pub fn parse(value: &str) -> Option<Credentials> {
    let v = value.trim();
    let rest = v.get(..6).filter(|s| s.eq_ignore_ascii_case("digest")).map(|_| &v[6..])?;
    let mut c = Credentials::default();
    for (k, val) in params(rest) {
        match k.to_ascii_lowercase().as_str() {
            "username" => c.username = val,
            "realm" => c.realm = val,
            "nonce" => c.nonce = val,
            "uri" => c.uri = val,
            "response" => c.response = val,
            "algorithm" => c.algorithm = Some(val),
            "qop" => c.qop = Some(val),
            "nc" => c.nc = Some(val),
            "cnonce" => c.cnonce = Some(val),
            _ => {}
        }
    }
    (!c.username.is_empty() && !c.nonce.is_empty() && !c.response.is_empty()).then_some(c)
}

/// Paires `clé=valeur` séparées par des virgules, valeurs entre guillemets
/// ou non.
fn params(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace() || *c == ',') {
            chars.next();
        }
        let key: String = std::iter::from_fn(|| chars.next_if(|c| *c != '=' && *c != ',')).collect();
        if key.trim().is_empty() {
            break;
        }
        let mut val = String::new();
        if chars.next_if_eq(&'=').is_some() {
            while chars.next_if(|c| c.is_whitespace()).is_some() {}
            if chars.next_if_eq(&'"').is_some() {
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => val.extend(chars.next()),
                        '"' => break,
                        c => val.push(c),
                    }
                }
            } else {
                val = std::iter::from_fn(|| chars.next_if(|c| *c != ',')).collect::<String>().trim().to_string();
            }
        }
        out.push((key.trim().to_string(), val));
    }
    out
}

/// Réponse attendue pour cette méthode, avec ou sans « qop=auth ».
pub fn expected(ha1: &str, method: &str, c: &Credentials) -> String {
    let ha2 = md5_hex(&[method.as_bytes(), b":", c.uri.as_bytes()]);
    match c.qop.as_deref() {
        Some(q) if q.eq_ignore_ascii_case("auth") => {
            let (nc, cnonce) = (c.nc.as_deref().unwrap_or(""), c.cnonce.as_deref().unwrap_or(""));
            md5_hex(&[ha1.as_bytes(), b":", c.nonce.as_bytes(), b":", nc.as_bytes(), b":", cnonce.as_bytes(), b":auth:", ha2.as_bytes()])
        }
        _ => md5_hex(&[ha1.as_bytes(), b":", c.nonce.as_bytes(), b":", ha2.as_bytes()]),
    }
}

/// La réponse prouve-t-elle la connaissance de HA1 ?
pub fn verify(ha1: &str, method: &str, c: &Credentials) -> bool {
    if c.algorithm.as_deref().is_some_and(|a| !a.eq_ignore_ascii_case("md5")) {
        return false;
    }
    if c.qop.as_deref().is_some_and(|q| !q.eq_ignore_ascii_case("auth")) {
        return false;
    }
    let want = expected(ha1, method, c);
    ct_eq(want.as_bytes(), c.response.to_ascii_lowercase().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vecteurs_md5_rfc1321() {
        assert_eq!(md5_hex(&[b""]), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(&[b"abc"]), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(md5_hex(&[b"message ", b"digest"]), "f96b697d7cb7938d525a2f31aaf161d0");
    }

    #[test]
    fn exemple_rfc2617() {
        // RFC 2617 §3.5 : Mufasa / Circle Of Life, avec qop=auth
        let h = ha1("Mufasa", "testrealm@host.com", "Circle Of Life");
        let c = parse(
            r#"Digest username="Mufasa", realm="testrealm@host.com", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093",
            uri="/dir/index.html", qop=auth, nc=00000001, cnonce="0a4f113b",
            response="6629fae49393a05397450978507c4ef1", opaque="5ccc069c403ebaf9f0171e9517f40e41""#,
        )
        .unwrap();
        assert_eq!(c.username, "Mufasa");
        assert_eq!(c.qop.as_deref(), Some("auth"));
        assert!(verify(&h, "GET", &c));
        assert!(!verify(&h, "POST", &c));
        assert!(!verify(&ha1("Mufasa", "testrealm@host.com", "autre"), "GET", &c));
    }

    #[test]
    fn sans_qop_et_valeurs_sans_guillemets() {
        let h = ha1("ata1", "v34net", "secret");
        let mut c = Credentials {
            username: "ata1".into(),
            realm: "v34net".into(),
            nonce: "abc".into(),
            uri: "sip:dialup.example.org:3434".into(),
            algorithm: Some("MD5".into()),
            ..Default::default()
        };
        c.response = expected(&h, "REGISTER", &c).to_ascii_uppercase();
        assert!(verify(&h, "REGISTER", &c));
        let p = parse("Digest username=ata1,realm=v34net,nonce=abc,uri=\"sip:x\",response=00,algorithm=MD5").unwrap();
        assert_eq!((p.username.as_str(), p.realm.as_str(), p.uri.as_str()), ("ata1", "v34net", "sip:x"));
        assert!(parse("Basic dXNlcjpwYXNz").is_none());
    }
}
