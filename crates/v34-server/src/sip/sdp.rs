//! Description de session (SDP, RFC 4566) : le strict nécessaire pour
//! s'entendre sur G.711 à 20 ms.
//!
//! Le central n'accepte que G.711 : loi A (type 8) de préférence, celle du
//! réseau français et du central, sinon loi µ (type 0). Une offre de
//! télécopie T.38 (`m=image … udptl t38`) sans audio utilisable est
//! refusée : l'adaptateur reste alors en G.711, ce que veut le fax du
//! central.

use std::net::IpAddr;

use v34_net::Codec;

/// Ce que l'autre côté propose (ou accepte).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Session {
    /// Adresse et port RTP du premier flux audio actif
    pub audio: Option<(Option<IpAddr>, u16)>,
    /// Types de charge utile proposés pour ce flux, dans l'ordre
    pub payloads: Vec<u8>,
    /// Paquétisation demandée (ms)
    pub ptime: Option<u32>,
    /// Un flux T.38 est proposé
    pub t38: bool,
}

impl Session {
    pub fn parse(body: &str) -> Option<Session> {
        let mut s = Session::default();
        let mut session_addr: Option<IpAddr> = None;
        // flux en cours d'analyse : (audio ?, adresse propre, port, charges)
        let mut cur: Option<(bool, Option<IpAddr>, u16, Vec<u8>)> = None;
        let mut seen_v = false;
        let finish = |cur: Option<(bool, Option<IpAddr>, u16, Vec<u8>)>, s: &mut Session, session_addr: Option<IpAddr>| {
            if let Some((true, addr, port, pts)) = cur {
                if port != 0 && s.audio.is_none() {
                    s.audio = Some((addr.or(session_addr), port));
                    s.payloads = pts;
                }
            }
        };
        for line in body.lines() {
            let line = line.trim();
            let Some((k, v)) = line.split_once('=') else { continue };
            match k {
                "v" => seen_v = true,
                "c" => {
                    // c=IN IP4 192.0.2.1
                    let addr = v.split_whitespace().nth(2).and_then(|a| a.split('/').next()?.parse().ok());
                    match cur.as_mut() {
                        Some(m) => m.1 = addr,
                        None => session_addr = addr,
                    }
                }
                "m" => {
                    finish(cur.take(), &mut s, session_addr);
                    let mut it = v.split_whitespace();
                    let (kind, port, proto) = (it.next().unwrap_or(""), it.next().unwrap_or("0"), it.next().unwrap_or(""));
                    let port: u16 = port.split('/').next().and_then(|p| p.parse().ok()).unwrap_or(0);
                    if kind == "image" && port != 0 && proto.eq_ignore_ascii_case("udptl") {
                        s.t38 = true;
                    }
                    let audio = kind == "audio" && proto.to_ascii_uppercase().starts_with("RTP/AVP");
                    cur = Some((audio, None, port, it.filter_map(|p| p.parse().ok()).collect()));
                }
                "a" => {
                    if let Some(p) = v.strip_prefix("ptime:") {
                        if cur.as_ref().is_some_and(|c| c.0) || cur.is_none() {
                            s.ptime = p.trim().parse().ok();
                        }
                    }
                }
                _ => {}
            }
        }
        finish(cur, &mut s, session_addr);
        seen_v.then_some(s)
    }

    /// Loi G.711 à utiliser : (codec, type de charge utile).
    pub fn choose(&self) -> Option<(Codec, u8)> {
        self.audio?;
        if self.payloads.contains(&8) {
            Some((Codec::Alaw, 8))
        } else if self.payloads.contains(&0) {
            Some((Codec::Ulaw, 0))
        } else {
            None
        }
    }
}

/// Notre description : un flux G.711 à 20 ms, en émission et réception.
/// `pts` : types de charge utile, par ordre de préférence (8 : loi A,
/// 0 : loi µ) ; une réponse n'en donne qu'un.
pub fn build(ip: IpAddr, port: u16, pts: &[u8], session_id: u64, version: u64) -> String {
    let family = if ip.is_ipv6() { "IP6" } else { "IP4" };
    let list: Vec<String> = pts.iter().map(u8::to_string).collect();
    let mut s = format!(
        "v=0\r\no=v34net {session_id} {version} IN {family} {ip}\r\ns=V34NET\r\nc=IN {family} {ip}\r\nt=0 0\r\nm=audio {port} RTP/AVP {}\r\n",
        list.join(" ")
    );
    for &pt in pts {
        s.push_str(&format!("a=rtpmap:{pt} {}/8000\r\n", if pt == 0 { "PCMU" } else { "PCMA" }));
    }
    s.push_str("a=ptime:20\r\na=sendrecv\r\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const CISCO: &str = "v=0\r\no=- 1234 1234 IN IP4 192.168.1.20\r\ns=-\r\nc=IN IP4 192.168.1.20\r\nt=0 0\r\n\
m=audio 16384 RTP/AVP 8 0 18 101\r\na=rtpmap:8 PCMA/8000\r\na=rtpmap:101 telephone-event/8000\r\na=ptime:30\r\na=sendrecv\r\n";

    #[test]
    fn offre_cisco() {
        let s = Session::parse(CISCO).unwrap();
        assert_eq!(s.audio, Some((Some("192.168.1.20".parse().unwrap()), 16384)));
        assert_eq!(s.payloads, vec![8, 0, 18, 101]);
        assert_eq!(s.ptime, Some(30));
        assert!(!s.t38);
        assert_eq!(s.choose(), Some((Codec::Alaw, 8)));
    }

    #[test]
    fn loi_mu_seule_puis_t38_seul() {
        let s = Session::parse("v=0\r\nc=IN IP4 10.0.0.1\r\nm=audio 4000 RTP/AVP 0 101\r\n").unwrap();
        assert_eq!(s.choose(), Some((Codec::Ulaw, 0)));
        let t = Session::parse("v=0\r\nc=IN IP4 10.0.0.1\r\nm=audio 0 RTP/AVP 8\r\nm=image 4002 udptl t38\r\n").unwrap();
        assert!(t.t38);
        assert_eq!(t.audio, None, "audio coupé (port 0)");
        assert_eq!(t.choose(), None);
        assert!(Session::parse("pas du SDP").is_none());
    }

    #[test]
    fn notre_description_se_relit() {
        let b = build("203.0.113.7".parse().unwrap(), 3434, &[8, 0], 42, 1);
        let s = Session::parse(&b).unwrap();
        assert_eq!(s.audio, Some((Some("203.0.113.7".parse().unwrap()), 3434)));
        assert_eq!(s.payloads, vec![8, 0]);
        assert_eq!(s.choose(), Some((Codec::Alaw, 8)));
        assert_eq!(s.ptime, Some(20));
    }
}
