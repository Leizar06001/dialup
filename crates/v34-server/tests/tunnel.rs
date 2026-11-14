//! Test d'intégration du tunnel FAI SANS modem : le bout client (proxy
//! SOCKS5 / HTTP CONNECT de v34-net) et le bout serveur (v34-server) sont
//! reliés directement par deux canaux d'octets, comme le ferait le modem.
//! Un petit serveur TCP d'écho local sert de « site Internet ».

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use v34_net::tunnel::{ClientConfig, TunnelClient, TunnelEvent};
use v34_server::log::{set_level, Level};
use v34_server::tunnel::{run, TunnelPolicy};

fn echo_server() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            std::thread::spawn(move || {
                let mut s = s;
                let mut buf = [0u8; 4096];
                while let Ok(n) = s.read(&mut buf) {
                    if n == 0 || s.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
            });
        }
    });
    port
}

fn policy(echo_port: u16, allow_private: bool) -> TunnelPolicy {
    TunnelPolicy {
        banner: "V34NET (test)".into(),
        max_streams: 8,
        allowed_ports: vec![echo_port, 80, 443],
        allow_private,
        denied: Vec::new(),
        connect_timeout: Duration::from_secs(2),
        web1998: None,
        salon: Some(v34_server::salon::Salon::new()),
    }
}

/// Relie un client et un serveur de tunnel par des canaux d'octets.
fn link(policy: TunnelPolicy, cfg: ClientConfig) -> (TunnelClient, JoinHandle<String>) {
    link_as(Arc::new(policy), cfg, "alice")
}

fn link_as(policy: Arc<TunnelPolicy>, cfg: ClientConfig, user: &str) -> (TunnelClient, JoinHandle<String>) {
    set_level(Level::Error);
    let (c2s_tx, c2s_rx) = mpsc::sync_channel(4);
    let (s2c_tx, s2c_rx) = mpsc::sync_channel(4);
    let user = user.to_string();
    let server = std::thread::spawn(move || run(policy, "test".into(), &user, c2s_rx, s2c_tx));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = TunnelClient::spawn(listener, cfg, s2c_rx, c2s_tx).unwrap();
    (client, server)
}

fn wait_event(c: &TunnelClient, what: &str, pred: impl Fn(&TunnelEvent) -> bool) -> TunnelEvent {
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(5) {
        while let Some(ev) = c.try_event() {
            if pred(&ev) {
                return ev;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("délai dépassé en attendant : {what}");
}

/// Poignée de main SOCKS5 ; renvoie le code de réponse et la socket.
fn socks5(proxy: std::net::SocketAddr, atyp_addr: &[u8], port: u16) -> (u8, TcpStream) {
    let mut s = TcpStream::connect(proxy).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(&[5, 1, 0]).unwrap();
    let mut r = [0u8; 2];
    s.read_exact(&mut r).unwrap();
    assert_eq!(r, [5, 0], "méthode sans authentification acceptée");
    let mut req = vec![5, 1, 0];
    req.extend_from_slice(atyp_addr);
    req.extend_from_slice(&port.to_be_bytes());
    s.write_all(&req).unwrap();
    let mut rep = [0u8; 10];
    s.read_exact(&mut rep).unwrap();
    assert_eq!(rep[0], 5);
    (rep[1], s)
}

fn echo_roundtrip(s: &mut TcpStream, data: &[u8]) {
    let writer = {
        let mut w = s.try_clone().unwrap();
        let data = data.to_vec();
        std::thread::spawn(move || w.write_all(&data).unwrap())
    };
    let mut back = vec![0u8; data.len()];
    s.read_exact(&mut back).unwrap();
    writer.join().unwrap();
    assert_eq!(back, data, "l'écho revient intact à travers le tunnel");
}

#[test]
fn socks5_et_http_connect() {
    let port = echo_server();
    let (mut client, server) = link(policy(port, true), ClientConfig::default());
    let ev = wait_event(&client, "bannière", |e| matches!(e, TunnelEvent::Text(_)));
    assert!(matches!(&ev, TunnelEvent::Text(t) if t.contains("V34NET (test)") && t.contains("Bienvenue alice")), "{ev:?}");
    wait_event(&client, "HELLO du serveur", |e| matches!(e, TunnelEvent::Ready { .. }));
    let proxy = client.local_addr();

    // SOCKS5, adresse IPv4
    let (code, mut s) = socks5(proxy, &[1, 127, 0, 0, 1], port);
    assert_eq!(code, 0, "SOCKS5 : succès");
    echo_roundtrip(&mut s, b"bonjour le 3615");
    // gros volume : contrôle de flux à travers des canaux bornés
    let big: Vec<u8> = (0..100_000u32).map(|i| (i * 7 % 251) as u8).collect();
    echo_roundtrip(&mut s, &big);

    // SOCKS5, nom de domaine (résolu côté serveur), en parallèle du premier flux
    let (code, mut s2) = socks5(proxy, &[3, 9, b'l', b'o', b'c', b'a', b'l', b'h', b'o', b's', b't'], port);
    assert_eq!(code, 0, "SOCKS5 avec nom de domaine");
    echo_roundtrip(&mut s2, b"second flux");
    echo_roundtrip(&mut s, b"premier flux toujours vivant");

    // HTTP CONNECT
    let mut h = TcpStream::connect(proxy).unwrap();
    h.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(h, "CONNECT 127.0.0.1:{port} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").unwrap();
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        h.read_exact(&mut b).unwrap();
        head.push(b[0]);
    }
    assert!(head.starts_with(b"HTTP/1.1 200"), "{}", String::from_utf8_lossy(&head));
    echo_roundtrip(&mut h, b"GET / HTTP/1.0\r\n\r\n");

    // Port non autorisé → SOCKS5 « refusé par les règles » (2)
    let (code, _) = socks5(proxy, &[1, 127, 0, 0, 1], 25);
    assert_eq!(code, 2);
    let ev = wait_event(&client, "OPEN_FAIL", |e| matches!(e, TunnelEvent::OpenFailed { .. }));
    assert!(matches!(ev, TunnelEvent::OpenFailed { reason, .. } if reason.contains("25")));

    // Fermeture côté client local → le flux se ferme proprement
    drop(s2);
    wait_event(&client, "CLOSE", |e| matches!(e, TunnelEvent::Closed { .. }));

    // Arrêt du client : le serveur voit la liaison se terminer.
    client.stop();
    let end = server.join().unwrap();
    assert!(end.contains("terminée"), "{end}");
}

#[test]
fn reseau_local_du_vps_inaccessible() {
    let port = echo_server();
    let (client, _server) = link(policy(port, false), ClientConfig::default());
    wait_event(&client, "HELLO", |e| matches!(e, TunnelEvent::Ready { .. }));
    let (code, _) = socks5(client.local_addr(), &[1, 127, 0, 0, 1], port);
    assert_eq!(code, 2, "127.0.0.1 refusé");
    let name = b"localhost";
    let mut a = vec![3, name.len() as u8];
    a.extend_from_slice(name);
    let (code, _) = socks5(client.local_addr(), &a, port);
    assert_eq!(code, 2, "un nom qui pointe vers 127.0.0.1 est refusé aussi");
}

/// Deux abonnés, chacun son tunnel, dans le même salon du FAI.
#[test]
fn salon_partage_entre_abonnes() {
    use std::io::{BufRead, BufReader};
    let policy = Arc::new(policy(1, false));
    let (ca, _sa) = link_as(policy.clone(), ClientConfig::default(), "raziel");
    let (cb, _sb) = link_as(policy.clone(), ClientConfig::default(), "gaston");
    wait_event(&ca, "HELLO A", |e| matches!(e, TunnelEvent::Ready { .. }));
    wait_event(&cb, "HELLO B", |e| matches!(e, TunnelEvent::Ready { .. }));
    // « salon » en nom de domaine, port 6667 (hors des ports autorisés :
    // c'est un hôte virtuel)
    let host = |h: &str| [&[3u8, h.len() as u8][..], h.as_bytes()].concat();
    let (code, a) = socks5(ca.local_addr(), &host("salon"), 6667);
    assert_eq!(code, 0);
    let mut ra = BufReader::new(a.try_clone().unwrap());
    let line = |r: &mut BufReader<TcpStream>| {
        let mut l = String::new();
        r.read_line(&mut l).unwrap();
        l.trim_end().to_string()
    };
    assert!(line(&mut ra).contains("Bienvenue sur le salon du FAI, raziel"));
    line(&mut ra);
    let (code, b) = socks5(cb.local_addr(), &host("SALON"), 6667);
    assert_eq!(code, 0);
    let mut rb = BufReader::new(b.try_clone().unwrap());
    line(&mut rb);
    assert_eq!(line(&mut rb), "* Présents : raziel");
    assert_eq!(line(&mut ra), "* gaston entre dans le salon.");
    (&a).write_all("salut à tous !\n".as_bytes()).unwrap();
    assert_eq!(line(&mut rb), "<raziel> salut à tous !");
    (&b).write_all(b"yo\n").unwrap();
    assert_eq!(line(&mut ra), "<gaston> yo");
    // Un autre port sur « salon » n'est pas le salon : nom inconnu.
    let (code, _) = socks5(ca.local_addr(), &host("salon"), 80);
    assert_ne!(code, 0);
}
