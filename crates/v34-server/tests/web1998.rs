//! Test d'intégration de l'hôte virtuel `http://web1998/` à travers le
//! tunnel FAI, SANS modem ni Internet : le bout client (proxy SOCKS5 /
//! HTTP CONNECT de v34-net) et le bout serveur sont reliés par deux canaux
//! d'octets, et l'appli n'a rien d'autre à faire qu'appeler `tunnel::run`
//! avec la politique par défaut.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use v34_net::tunnel::{ClientConfig, TunnelClient, TunnelEvent};
use v34_server::config::Web1998 as Web1998Config;
use v34_server::config::Fai;
use v34_server::log::{set_level, Level};
use v34_server::tunnel::{run, TunnelPolicy};
use v34_server::web1998::{is_virtual_host, HOST};

fn link(policy: TunnelPolicy) -> TunnelClient {
    set_level(Level::Error);
    let (c2s_tx, c2s_rx) = mpsc::sync_channel(4);
    let (s2c_tx, s2c_rx) = mpsc::sync_channel(4);
    std::thread::spawn(move || run(Arc::new(policy), "test".into(), "alice", c2s_rx, s2c_tx));
    let client = TunnelClient::spawn(TcpListener::bind("127.0.0.1:0").unwrap(), ClientConfig::default(), s2c_rx, c2s_tx).unwrap();
    let t0 = Instant::now();
    loop {
        if let Some(TunnelEvent::Ready { .. }) = client.try_event() {
            return client;
        }
        assert!(t0.elapsed() < Duration::from_secs(5), "tunnel prêt");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// SOCKS5 avec un nom de domaine ; renvoie le code de réponse et la socket.
fn socks5(proxy: SocketAddr, host: &str, port: u16) -> (u8, TcpStream) {
    let mut s = TcpStream::connect(proxy).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(&[5, 1, 0]).unwrap();
    let mut r = [0u8; 2];
    s.read_exact(&mut r).unwrap();
    let mut req = vec![5, 1, 0, 3, host.len() as u8];
    req.extend_from_slice(host.as_bytes());
    req.extend_from_slice(&port.to_be_bytes());
    s.write_all(&req).unwrap();
    let mut rep = [0u8; 10];
    s.read_exact(&mut rep).unwrap();
    (rep[1], s)
}

fn http_get(mut s: TcpStream, path: &str) -> String {
    write!(s, "GET {path} HTTP/1.1\r\nHost: web1998\r\nUser-Agent: test\r\n\r\n").unwrap();
    let mut resp = Vec::new();
    s.read_to_end(&mut resp).unwrap();
    String::from_utf8_lossy(&resp).into_owned()
}

#[test]
fn hote_virtuel_servi_par_le_tunnel() {
    assert!(is_virtual_host("web1998", 80) && is_virtual_host("WEB1998", 80) && !is_virtual_host("web1998", 443));
    // politique par défaut : réseau local fermé, web 1998 actif
    let client = link(TunnelPolicy::default());
    let proxy = client.local_addr();

    let (code, s) = socks5(proxy, HOST, 80);
    assert_eq!(code, 0, "SOCKS5 vers l'hôte virtuel accepté");
    let home = http_get(s, "/");
    assert!(home.starts_with("HTTP/1.0 200 OK\r\n"), "{home}");
    assert!(home.contains("Content-Type: text/html; charset=utf-8"));
    assert!(home.contains("<title>Tinyverse 1998"), "{home}");
    assert!(home.contains("bgcolor=\"#c0c0c0\""));
    assert!(home.contains("/p?u=https://frogfind.com/") && home.contains("/p?u=https://lite.cnn.com/"));

    // casse indifférente, et image générée
    let (code, s) = socks5(proxy, "Web1998", 80);
    assert_eq!(code, 0);
    let gif = http_get(s, "/construction.gif");
    assert!(gif.starts_with("HTTP/1.0 200 OK") && gif.contains("Content-Type: image/gif\r\n"));

    // HTTP CONNECT vers l'hôte virtuel (navigateur réglé en proxy HTTPS)
    let mut h = TcpStream::connect(proxy).unwrap();
    h.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(h, "CONNECT web1998:80 HTTP/1.1\r\nHost: web1998:80\r\n\r\n").unwrap();
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        h.read_exact(&mut b).unwrap();
        head.push(b[0]);
    }
    assert!(head.starts_with(b"HTTP/1.1 200"), "{}", String::from_utf8_lossy(&head));
    let notfound = http_get(h, "/pas/ici");
    assert!(notfound.starts_with("HTTP/1.0 404") && notfound.contains("Fichier introuvable"));

    // le service refuse de récupérer le réseau local du serveur
    for u in ["http://127.0.0.1/", "http://127.0.0.1:3434/", "http://localhost/", "http://192.168.0.1/", "http://10.0.0.1/"] {
        let (code, s) = socks5(proxy, HOST, 80);
        assert_eq!(code, 0);
        let r = http_get(s, &format!("/p?u={u}"));
        assert!(r.starts_with("HTTP/1.0 403"), "{u} : {r}");
        assert!(r.contains("Adresse interdite"), "{u}");
    }

    // web1998 sur un autre port : pas intercepté (port non autorisé)
    let (code, _) = socks5(proxy, HOST, 8080);
    assert_eq!(code, 2, "refusé par les règles");
}

#[test]
fn hote_virtuel_desactive() {
    let off = Web1998Config { actif: false, ..Web1998Config::default() };
    let policy = TunnelPolicy::from_configs(&Fai::default(), &off).unwrap();
    assert!(policy.web1998.is_none());
    let client = link(policy);
    let (code, _) = socks5(client.local_addr(), HOST, 80);
    assert_ne!(code, 0, "nom inconnu quand le service est désactivé");
}
