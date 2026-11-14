//! Filtrage des destinations du tunnel FAI.
//!
//! Le serveur ouvre des connexions TCP pour le compte de ses abonnés : il
//! ne doit pas leur ouvrir le réseau local du VPS (services en écoute sur
//! 127.0.0.1, réseau privé de l'hébergeur, métadonnées du cloud en
//! 169.254.169.254…). On filtre l'adresse APRÈS résolution DNS et on se
//! connecte à cette adresse précise : pas de contournement par un nom qui
//! pointe vers 127.0.0.1, ni par « DNS rebinding ».

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Adresse non routable sur Internet (privée, locale, réservée…).
pub fn is_forbidden(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => forbidden_v4(v4),
        IpAddr::V6(v6) => forbidden_v6(v6),
    }
}

fn forbidden_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    a == 0                                   // 0.0.0.0/8 « ce réseau »
        || a == 10                           // 10/8 privé
        || (a == 100 && (64..128).contains(&b)) // 100.64/10 CGNAT
        || a == 127                          // 127/8 boucle locale
        || (a == 169 && b == 254)            // 169.254/16 lien local (métadonnées cloud)
        || (a == 172 && (16..32).contains(&b)) // 172.16/12 privé
        || (a == 192 && b == 0 && c == 0)    // 192.0.0/24 IETF
        || (a == 192 && b == 0 && c == 2)    // 192.0.2/24 documentation
        || (a == 192 && b == 168)            // 192.168/16 privé
        || (a == 198 && (b == 18 || b == 19)) // 198.18/15 bancs de test
        || (a == 198 && b == 51 && c == 100) // documentation
        || (a == 203 && b == 0 && c == 113)  // documentation
        || a >= 224 // multicast 224/4, réservé 240/4, diffusion
}

fn forbidden_v6(ip: Ipv6Addr) -> bool {
    let s = ip.segments();
    if ip.is_unspecified() || ip.is_loopback() {
        return true;
    }
    // ::ffff:a.b.c.d (IPv4 mappée) et ::a.b.c.d (compatible, obsolète)
    if s[0..5] == [0, 0, 0, 0, 0] && (s[5] == 0xFFFF || s[5] == 0) {
        let v4 = Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8);
        return s[5] == 0 || forbidden_v4(v4);
    }
    // 64:ff9b::/96 (NAT64) : on juge l'IPv4 embarquée
    if s[0] == 0x64 && s[1] == 0xFF9B && s[2..6] == [0, 0, 0, 0] {
        let v4 = Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8);
        return forbidden_v4(v4);
    }
    // 2002::/16 (6to4) : IPv4 embarquée dans les segments 1-2
    if s[0] == 0x2002 {
        let v4 = Ipv4Addr::new((s[1] >> 8) as u8, s[1] as u8, (s[2] >> 8) as u8, s[2] as u8);
        return forbidden_v4(v4);
    }
    (s[0] & 0xFE00) == 0xFC00          // fc00::/7 adresses locales uniques
        || (s[0] & 0xFFC0) == 0xFE80   // fe80::/10 lien local
        || (s[0] & 0xFFC0) == 0xFEC0   // fec0::/10 site local (obsolète)
        || (s[0] & 0xFF00) == 0xFF00   // ff00::/8 multicast
        || (s[0] == 0x2001 && s[1] == 0x0DB8) // documentation
        || (s[0] == 0x0100 && s[1..4] == [0, 0, 0]) // 100::/64 trou noir
}

/// Réseau « adresse/préfixe » (ex. `203.0.113.7/32`, `2001:db8::/32`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cidr {
    net: IpAddr,
    prefix: u8,
}

impl Cidr {
    pub fn parse(s: &str) -> Option<Cidr> {
        let (addr, prefix) = match s.split_once('/') {
            Some((a, p)) => (a.trim().parse::<IpAddr>().ok()?, Some(p.trim().parse::<u8>().ok()?)),
            None => (s.trim().parse::<IpAddr>().ok()?, None),
        };
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = prefix.unwrap_or(max);
        (prefix <= max).then_some(Cidr { net: addr, prefix })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = match ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
            v4 => v4,
        };
        match (self.net, ip) {
            (IpAddr::V4(n), IpAddr::V4(a)) => mask_eq(&n.octets(), &a.octets(), self.prefix),
            (IpAddr::V6(n), IpAddr::V6(a)) => mask_eq(&n.octets(), &a.octets(), self.prefix),
            _ => false,
        }
    }
}

fn mask_eq(a: &[u8], b: &[u8], prefix: u8) -> bool {
    let full = (prefix / 8) as usize;
    if a[..full] != b[..full] {
        return false;
    }
    let rem = prefix % 8;
    if rem == 0 {
        return true;
    }
    let m = 0xFFu8 << (8 - rem);
    (a[full] & m) == (b[full] & m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn adresses_interdites() {
        for s in [
            "127.0.0.1",
            "127.255.0.9",
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "224.0.0.1",
            "255.255.255.255",
            "::1",
            "::",
            "fe80::1",
            "fd12:3456::1",
            "ff02::1",
            "::ffff:127.0.0.1",
            "::ffff:192.168.0.10",
            "64:ff9b::a00:1",
            "2002:7f00:1::",
            "2001:db8::1",
        ] {
            assert!(is_forbidden(ip(s)), "{s} devrait être interdite");
        }
    }

    #[test]
    fn adresses_publiques_autorisees() {
        for s in ["1.1.1.1", "8.8.8.8", "172.15.0.1", "172.32.0.1", "100.128.0.1", "93.184.216.34", "2606:4700::1111", "::ffff:8.8.8.8", "64:ff9b::808:808"] {
            assert!(!is_forbidden(ip(s)), "{s} devrait être autorisée");
        }
    }

    #[test]
    fn reseaux_cidr() {
        let c = Cidr::parse("203.0.113.0/24").unwrap();
        assert!(c.contains(ip("203.0.113.77")));
        assert!(!c.contains(ip("203.0.114.1")));
        assert!(c.contains(ip("::ffff:203.0.113.5")));
        let h = Cidr::parse("51.15.1.2").unwrap();
        assert!(h.contains(ip("51.15.1.2")));
        assert!(!h.contains(ip("51.15.1.3")));
        let v6 = Cidr::parse("2a01:4f8::/29").unwrap();
        assert!(v6.contains(ip("2a01:4f8:c0c:1::1")));
        assert!(!v6.contains(ip("2a02::1")));
        let odd = Cidr::parse("10.0.0.0/9").unwrap();
        assert!(odd.contains(ip("10.127.0.1")));
        assert!(!odd.contains(ip("10.128.0.1")));
        assert!(Cidr::parse("1.2.3.4/33").is_none());
        assert!(Cidr::parse("pas une adresse").is_none());
        assert!(Cidr::parse("0.0.0.0/0").unwrap().contains(ip("9.9.9.9")));
    }
}
