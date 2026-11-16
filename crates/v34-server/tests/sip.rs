//! Passerelle SIP de bout en bout : un faux adaptateur téléphonique (SIP et
//! RTP « à la main » sur UDP) face au central, à sa ligne de test et à un
//! abonné de l'appli (`CentralClient`).
//!
//! Les deux tests « un vrai modem derrière l'adaptateur » tournent en temps
//! réel (~20 s), d'où `#[ignore]` ; `SIP_TRACE=1` affiche le journal du
//! central (messages SIP, corrections de cadence) :
//!     SIP_TRACE=1 cargo test --release -p v34-server --test sip -- --ignored --nocapture

use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use v34_net::auth::PasswordHash;
use v34_net::central::{CentralClient, CentralConfig, CentralEvent};
use v34_net::signal::HangupReason;
use v34_net::{Codec, FRAME_SAMPLES};
use v34_server::config::empreinte_sip;
use v34_server::log::{set_level, Level};
use v34_server::sip::digest::{self, Credentials};
use v34_server::sip::message::{self, Message};
use v34_server::sip::rtp::{self, Header};
use v34_server::sip::sdp;
use v34_server::{Config, Server, ServerHandle};

const ALICE: (&str, &str, &str) = ("alice", "mot-de-passe-alice", "0142424242");
const ATA: (&str, &str, &str) = ("ata1", "secret-ata1", "0142000001");
const ECHO: &str = "0800000000";

fn start_server() -> ServerHandle {
    set_level(if std::env::var("SIP_TRACE").is_ok() { Level::Debug } else { Level::Error });
    let h = PasswordHash::new(ALICE.1, 1000);
    let toml = format!(
        r#"
        [serveur]
        ecoute = "127.0.0.1:0"
        journal = "error"
        [central]
        keepalive_s = 5
        expiration_s = 15
        sonnerie_max_s = 20
        [modems]
        delai_decroche_ms = 100
        [sip]
        actif = true
        domaine = "v34net"
        [[abonnes]]
        nom = "{}"
        numero = "{}"
        mot_de_passe_hash = "{h}"
        [[postes_sip]]
        nom = "{}"
        numero = "{}"
        empreinte = "{}"
        "#,
        ALICE.0,
        ALICE.2,
        ATA.0,
        ATA.2,
        empreinte_sip(ATA.0, "v34net", ATA.1)
    );
    Server::bind(Config::from_toml(&toml).expect("configuration de test")).unwrap().spawn().unwrap()
}

fn alice(server: &ServerHandle) -> CentralClient {
    let mut cfg = CentralConfig::new(server.local_addr().to_string(), ALICE.0, ALICE.1);
    cfg.codec = Codec::Alaw;
    let mut c = CentralClient::connect(cfg).unwrap();
    wait_event(&mut c, "enregistrement d'alice", |e| matches!(e, CentralEvent::Registered { .. }));
    c
}

fn wait_event(c: &mut CentralClient, what: &str, pred: impl Fn(&CentralEvent) -> bool) -> CentralEvent {
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(5) {
        c.poll().unwrap();
        while let Some(ev) = c.next_event() {
            if pred(&ev) {
                return ev;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("délai dépassé en attendant : {what}");
}

fn rand_tag() -> String {
    digest::hex(&v34_net::crypto::random_bytes::<6>())
}

/// Valeur entre guillemets d'un paramètre de défi (`nonce="…"`).
fn quoted(h: &str, key: &str) -> String {
    let k = format!("{key}=\"");
    let i = h.find(&k).unwrap_or_else(|| panic!("{key} absent de {h}")) + k.len();
    h[i..i + h[i..].find('"').unwrap()].to_string()
}

/// Le faux adaptateur.
struct Ua {
    sip: UdpSocket,
    rtp: UdpSocket,
    server: SocketAddr,
    user: &'static str,
    password: &'static str,
    tag: String,
}

impl Ua {
    fn new(server: &ServerHandle, user: &'static str, password: &'static str) -> Ua {
        let sip = UdpSocket::bind("127.0.0.1:0").unwrap();
        sip.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
        let rtp = UdpSocket::bind("127.0.0.1:0").unwrap();
        rtp.set_nonblocking(true).unwrap();
        Ua { sip, rtp, server: server.local_addr(), user, password, tag: rand_tag() }
    }

    fn local(&self) -> SocketAddr {
        self.sip.local_addr().unwrap()
    }

    fn rtp_port(&self) -> u16 {
        self.rtp.local_addr().unwrap().port()
    }

    fn send(&self, m: &str) {
        self.sip.send_to(m.as_bytes(), self.server).unwrap();
    }

    fn recv(&self) -> Option<Message> {
        let mut buf = [0u8; 8192];
        let (n, _) = self.sip.recv_from(&mut buf).ok()?;
        Some(Message::parse(&buf[..n]).expect("SIP valide du central"))
    }

    /// Attend un message SIP (en faisant tourner les clients de l'appli).
    fn expect(&self, clients: &mut [&mut CentralClient], what: &str, pred: impl Fn(&Message) -> bool) -> Message {
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(5) {
            for c in clients.iter_mut() {
                c.poll().unwrap();
            }
            if let Some(m) = self.recv() {
                if pred(&m) {
                    return m;
                }
            }
        }
        panic!("délai dépassé en attendant : {what}");
    }

    fn silent(&self, within: Duration) -> Option<Message> {
        let t0 = Instant::now();
        while t0.elapsed() < within {
            if let Some(m) = self.recv() {
                return Some(m);
            }
        }
        None
    }

    fn authorization(&self, hdr: &str, method: &str, uri: &str, challenge: &Message) -> String {
        let www = challenge.header("WWW-Authenticate").or(challenge.header("Proxy-Authenticate")).expect("défi");
        let (realm, nonce) = (quoted(www, "realm"), quoted(www, "nonce"));
        let mut c = Credentials {
            username: self.user.into(),
            realm: realm.clone(),
            nonce: nonce.clone(),
            uri: uri.into(),
            qop: Some("auth".into()),
            nc: Some("00000001".into()),
            cnonce: Some("0a4f113b".into()),
            ..Default::default()
        };
        c.response = digest::expected(&digest::ha1(self.user, &realm, self.password), method, &c);
        format!(
            "{hdr}: Digest username=\"{}\", realm=\"{realm}\", nonce=\"{nonce}\", uri=\"{uri}\", response=\"{}\", algorithm=MD5, cnonce=\"0a4f113b\", qop=auth, nc=00000001\r\n",
            self.user, c.response
        )
    }

    fn register_msg(&self, cseq: u32, auth: &str) -> String {
        let (l, u) = (self.local(), self.user);
        format!(
            "REGISTER sip:{srv} SIP/2.0\r\nVia: SIP/2.0/UDP {l};branch=z9hG4bK{b};rport\r\nMax-Forwards: 70\r\n\
             From: <sip:{u}@v34net>;tag={t}\r\nTo: <sip:{u}@v34net>\r\nCall-ID: reg-{t}@{l}\r\nCSeq: {cseq} REGISTER\r\n\
             Contact: <sip:{u}@{l}>;expires=3600\r\nUser-Agent: Cisco/ATA192-MPP-11-1-0MSR3-9 (essai)\r\n\
             Allow: ACK, BYE, CANCEL, INFO, INVITE, NOTIFY, OPTIONS, REFER, UPDATE\r\nSupported: replaces\r\n{auth}Content-Length: 0\r\n\r\n",
            srv = self.server,
            b = rand_tag(),
            t = self.tag
        )
    }

    /// REGISTER → 401 → REGISTER signé → réponse finale.
    fn register(&self) -> Message {
        self.send(&self.register_msg(1, ""));
        let challenge = self.expect(&mut [], "401", |m| m.code() == Some(401));
        let uri = format!("sip:{}", self.server);
        self.send(&self.register_msg(2, &self.authorization("Authorization", "REGISTER", &uri, &challenge)));
        self.expect(&mut [], "réponse au REGISTER signé", |m| m.code().is_some_and(|c| c >= 200))
    }

    fn sdp(&self) -> String {
        format!(
            "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio {} RTP/AVP 8 0 101\r\n\
             a=rtpmap:8 PCMA/8000\r\na=rtpmap:101 telephone-event/8000\r\na=ptime:20\r\na=sendrecv\r\n",
            self.rtp_port()
        )
    }

    fn invite_msg(&self, number: &str, call_id: &str, cseq: u32, branch: &str, auth: &str) -> String {
        let (l, u, sdp) = (self.local(), self.user, self.sdp());
        format!(
            "INVITE sip:{number}@v34net SIP/2.0\r\nVia: SIP/2.0/UDP {l};branch={branch};rport\r\nMax-Forwards: 70\r\n\
             From: \"Ligne 1\" <sip:{u}@v34net>;tag={t}\r\nTo: <sip:{number}@v34net>\r\nCall-ID: {call_id}\r\nCSeq: {cseq} INVITE\r\n\
             Contact: <sip:{u}@{l}>\r\n{auth}Content-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{sdp}",
            sdp.len(),
            t = self.tag
        )
    }

    /// INVITE → 407 (+ ACK) → INVITE signé. Renvoie (Call-ID, CSeq, branche).
    fn dial(&self, number: &str) -> (String, u32, String) {
        let call_id = format!("{}@{}", rand_tag(), self.local());
        let b1 = format!("z9hG4bK{}", rand_tag());
        self.send(&self.invite_msg(number, &call_id, 1, &b1, ""));
        let challenge = self.expect(&mut [], "407", |m| m.code() == Some(407));
        let l = self.local();
        self.send(&format!(
            "ACK sip:{number}@v34net SIP/2.0\r\nVia: SIP/2.0/UDP {l};branch={b1};rport\r\nMax-Forwards: 70\r\n\
             From: \"Ligne 1\" <sip:{}@v34net>;tag={}\r\nTo: {}\r\nCall-ID: {call_id}\r\nCSeq: 1 ACK\r\nContent-Length: 0\r\n\r\n",
            self.user,
            self.tag,
            challenge.header("To").unwrap()
        ));
        let uri = format!("sip:{number}@v34net");
        let b2 = format!("z9hG4bK{}", rand_tag());
        self.send(&self.invite_msg(number, &call_id, 2, &b2, &self.authorization("Proxy-Authorization", "INVITE", &uri, &challenge)));
        (call_id, 2, b2)
    }

    /// Requête dans le dialogue ouvert par notre INVITE (ACK du 200, BYE).
    fn in_dialog(&self, method: &str, ok: &Message, cseq: u32) -> String {
        let target = message::addr_uri(ok.header("Contact").unwrap()).to_string();
        format!(
            "{method} {target} SIP/2.0\r\nVia: SIP/2.0/UDP {};branch=z9hG4bK{};rport\r\nMax-Forwards: 70\r\nFrom: {}\r\nTo: {}\r\n\
             Call-ID: {}\r\nCSeq: {cseq} {method}\r\nContent-Length: 0\r\n\r\n",
            self.local(),
            rand_tag(),
            ok.header("From").unwrap(),
            ok.header("To").unwrap(),
            ok.call_id().unwrap()
        )
    }

    /// Réponse à une requête du central (`sdp` : notre description dans un 200).
    fn respond(&self, req: &Message, code: u16, sdp: bool) -> String {
        let vias: String = req.headers("Via").map(|v| format!("Via: {v}\r\n")).collect();
        let body = if sdp { self.sdp() } else { String::new() };
        let ctype = if sdp { "Content-Type: application/sdp\r\n" } else { "" };
        format!(
            "SIP/2.0 {code} {}\r\n{vias}From: {}\r\nTo: {};tag={}\r\nCall-ID: {}\r\nCSeq: {}\r\nContact: <sip:{}@{}>\r\n{ctype}Content-Length: {}\r\n\r\n{body}",
            message::reason(code),
            req.header("From").unwrap(),
            req.header("To").unwrap(),
            self.tag,
            req.call_id().unwrap(),
            req.header("CSeq").unwrap(),
            self.user,
            self.local(),
            body.len()
        )
    }

    fn send_rtp(&self, to: SocketAddr, seq: u16, payload: &[u8]) {
        let h = Header { pt: 8, marker: seq == 0, seq, ts: 8000 + seq as u32 * FRAME_SAMPLES as u32, ssrc: 0xA7A1 };
        self.rtp.send_to(&rtp::encode(&h, payload), to).unwrap();
    }

    fn drain_rtp(&self) -> Vec<(Header, Vec<u8>)> {
        let mut out = Vec::new();
        let mut buf = [0u8; 2048];
        while let Ok((n, _)) = self.rtp.recv_from(&mut buf) {
            if let Some((h, p)) = rtp::parse(&buf[..n]) {
                out.push((h, p.to_vec()));
            }
        }
        out
    }
}

/// Adresse RTP annoncée dans un SDP.
fn media(m: &Message) -> SocketAddr {
    let s = sdp::Session::parse(m.body_str()).expect("SDP");
    let (ip, port) = s.audio.expect("flux audio");
    SocketAddr::new(ip.unwrap(), port)
}

#[test]
fn enregistrement_puis_appel_de_la_ligne_de_test_en_rtp() {
    let server = start_server();
    let ua = Ua::new(&server, ATA.0, ATA.1);
    let ok = ua.register();
    assert_eq!(ok.code(), Some(200));
    assert_eq!(ok.header("Expires"), Some("120"), "durée plafonnée par enregistrement_max_s");
    assert!(ok.header("Date").is_some_and(|d| d.ends_with("GMT")));
    assert!(message::param(ok.header("Via").unwrap(), "received").is_some());

    let (call_id, cseq, _) = ua.dial(ECHO);
    ua.expect(&mut [], "100 Trying", |m| m.code() == Some(100));
    ua.expect(&mut [], "180 Ringing", |m| m.code() == Some(180));
    let ok = ua.expect(&mut [], "200 OK (le modem décroche)", |m| m.code() == Some(200) && m.cseq() == Some((cseq, "INVITE")));
    assert_eq!(ok.call_id(), Some(call_id.as_str()));
    assert!(ok.to_tag().is_some());
    let answer = sdp::Session::parse(ok.body_str()).unwrap();
    assert_eq!(answer.payloads, vec![8], "réponse : loi A seule");
    let srv_media = media(&ok);
    assert_eq!(srv_media, server.local_addr(), "le RTP passe par le port du central");
    ua.send(&ua.in_dialog("ACK", &ok, cseq));

    // De l'audio dans les deux sens pendant 1,5 s : le modem du central
    // émet sa tonalité de réponse, en paquets RTP de 20 ms.
    let silence = vec![0xD5u8; FRAME_SAMPLES];
    let mut got = Vec::new();
    for seq in 0..75u16 {
        ua.send_rtp(srv_media, seq, &silence);
        std::thread::sleep(Duration::from_millis(20));
        got.extend(ua.drain_rtp());
    }
    assert!(got.len() >= 50, "trop peu de paquets RTP du modem : {}", got.len());
    assert!(got.iter().all(|(h, p)| h.pt == 8 && p.len() == FRAME_SAMPLES), "une trame G.711 loi A par paquet");
    assert!(got.windows(2).all(|w| w[1].0.ssrc == w[0].0.ssrc));
    assert!(got.iter().any(|(_, p)| p.iter().any(|&b| b != 0xD5 && b != 0x55)), "le modem émet un signal");
    assert!(ua.silent(Duration::from_millis(200)).is_none_or(|m| m.code() == Some(200)), "pas de raccrochage intempestif");

    // On raccroche : 200, et l'appel disparaît du central.
    ua.send(&ua.in_dialog("BYE", &ok, cseq + 1));
    ua.expect(&mut [], "200 OK du BYE", |m| m.code() == Some(200) && m.cseq().is_some_and(|c| c.1 == "BYE"));
    let t0 = Instant::now();
    while server.stats().calls != 0 && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(server.stats().calls, 0);
}

#[test]
fn l_appli_appelle_le_poste_et_l_audio_passe_bit_a_bit() {
    let server = start_server();
    let ua = Ua::new(&server, ATA.0, ATA.1);
    assert_eq!(ua.register().code(), Some(200));
    let mut a = alice(&server);

    a.dial(ATA.2).unwrap();
    let inv = ua.expect(&mut [&mut a], "INVITE du central", |m| m.method() == Some("INVITE"));
    assert_eq!(inv.uri(), Some(format!("sip:{}@{}", ATA.0, ua.local()).as_str()), "adressé au contact enregistré");
    assert_eq!(message::uri_user(message::addr_uri(inv.header("From").unwrap())), Some(ALICE.2), "numéro de l'appelant présenté");
    assert_eq!(sdp::Session::parse(inv.body_str()).unwrap().payloads, vec![8, 0]);
    let srv_media = media(&inv);
    ua.send(&ua.respond(&inv, 180, false));
    wait_event(&mut a, "RINGING chez alice", |e| matches!(e, CentralEvent::Ringing { .. }));
    ua.send(&ua.respond(&inv, 200, true));
    ua.expect(&mut [&mut a], "ACK du 200", |m| m.method() == Some("ACK"));
    let call_id = match wait_event(&mut a, "ANSWER chez alice", |e| matches!(e, CentralEvent::Answered { .. })) {
        CentralEvent::Answered { call_id } => call_id,
        _ => unreachable!(),
    };

    // alice envoie 0,25 ; le poste envoie −0,5 (en loi A) : chacun reçoit
    // exactement les codes de l'autre.
    let from_alice = rtp::encode_payload(Codec::Alaw, &[0.25; FRAME_SAMPLES]);
    let from_ata = rtp::encode_payload(Codec::Alaw, &[-0.5; FRAME_SAMPLES]);
    let (mut got_ata, mut got_alice) = (0, 0);
    let mut out = vec![0.0f32; FRAME_SAMPLES];
    for seq in 0..40u16 {
        a.send_block(&[0.25; FRAME_SAMPLES]).unwrap();
        ua.send_rtp(srv_media, seq, &from_ata);
        std::thread::sleep(Duration::from_millis(20));
        a.poll().unwrap();
        for (h, p) in ua.drain_rtp() {
            assert_eq!(h.pt, 8);
            assert_eq!(p, from_alice, "le poste reçoit les codes d'alice tels quels");
            got_ata += 1;
        }
        if a.recv_block(&mut out) && out.iter().any(|&s| s != 0.0) {
            let want = rtp::decode_payload(Codec::Alaw, &from_ata);
            assert_eq!(out, want, "alice reçoit les codes du poste tels quels");
            got_alice += 1;
        }
    }
    assert!(got_ata >= 25 && got_alice >= 15, "trop peu de trames relayées : vers le poste {got_ata}, vers alice {got_alice}");

    // alice raccroche : BYE au poste
    a.hangup().unwrap();
    let bye = ua.expect(&mut [&mut a], "BYE du central", |m| m.method() == Some("BYE"));
    ua.send(&ua.respond(&bye, 200, false));
    let t0 = Instant::now();
    while server.stats().calls != 0 && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(server.stats().calls, 0, "appel {call_id:08x} terminé");
}

#[test]
fn le_poste_appelle_l_appli_puis_raccroche() {
    let server = start_server();
    let ua = Ua::new(&server, ATA.0, ATA.1);
    assert_eq!(ua.register().code(), Some(200));
    let mut a = alice(&server);

    let (_, cseq, _) = ua.dial(ALICE.2);
    match wait_event(&mut a, "appel du poste chez alice", |e| matches!(e, CentralEvent::Incoming { .. })) {
        CentralEvent::Incoming { from, .. } => assert_eq!(from, ATA.2),
        _ => unreachable!(),
    }
    ua.expect(&mut [&mut a], "180", |m| m.code() == Some(180));
    a.answer().unwrap();
    let ok = ua.expect(&mut [&mut a], "200", |m| m.code() == Some(200) && m.cseq() == Some((cseq, "INVITE")));
    ua.send(&ua.in_dialog("ACK", &ok, cseq));
    let srv_media = media(&ok);
    let tone = rtp::encode_payload(Codec::Alaw, &[0.125; FRAME_SAMPLES]);
    let mut heard = false;
    let mut out = vec![0.0f32; FRAME_SAMPLES];
    for seq in 0..30u16 {
        ua.send_rtp(srv_media, seq, &tone);
        a.send_block(&[0.0; FRAME_SAMPLES]).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        a.poll().unwrap();
        heard |= a.recv_block(&mut out) && out.iter().all(|&s| (s - 0.125).abs() < 0.01);
    }
    assert!(heard, "alice entend le poste");
    // le 200 n'est plus répété après l'ACK
    assert!(ua.silent(Duration::from_millis(800)).is_none_or(|m| m.code() != Some(200)));

    ua.send(&ua.in_dialog("BYE", &ok, cseq + 1));
    ua.expect(&mut [&mut a], "200 du BYE", |m| m.code() == Some(200));
    let ev = wait_event(&mut a, "HANGUP chez alice", |e| matches!(e, CentralEvent::HungUp { .. }));
    assert!(matches!(ev, CentralEvent::HungUp { reason: HangupReason::Normal, .. }));
}

#[test]
fn le_poste_abandonne_avant_la_reponse() {
    let server = start_server();
    let ua = Ua::new(&server, ATA.0, ATA.1);
    assert_eq!(ua.register().code(), Some(200));
    let mut a = alice(&server);
    let (call_id, cseq, branch) = ua.dial(ALICE.2);
    wait_event(&mut a, "appel chez alice", |e| matches!(e, CentralEvent::Incoming { .. }));
    ua.expect(&mut [&mut a], "180", |m| m.code() == Some(180));
    ua.send(&format!(
        "CANCEL sip:{}@v34net SIP/2.0\r\nVia: SIP/2.0/UDP {};branch={branch};rport\r\nMax-Forwards: 70\r\n\
         From: \"Ligne 1\" <sip:{}@v34net>;tag={}\r\nTo: <sip:{}@v34net>\r\nCall-ID: {call_id}\r\nCSeq: {cseq} CANCEL\r\nContent-Length: 0\r\n\r\n",
        ALICE.2,
        ua.local(),
        ATA.0,
        ua.tag,
        ALICE.2
    ));
    ua.expect(&mut [&mut a], "200 du CANCEL", |m| m.code() == Some(200) && m.cseq().is_some_and(|c| c.1 == "CANCEL"));
    ua.expect(&mut [&mut a], "487 de l'INVITE", |m| m.code() == Some(487));
    wait_event(&mut a, "HANGUP chez alice", |e| matches!(e, CentralEvent::HungUp { .. }));
}

#[test]
fn mauvais_mot_de_passe_et_pas_d_amplification() {
    let server = start_server();
    let ua = Ua::new(&server, ATA.0, "pas-le-bon");
    assert_eq!(ua.register().code(), Some(403));

    // Requête minuscule d'une source inconnue : la réponse serait plus
    // grosse qu'elle, le central se tait.
    let probe = UdpSocket::bind("127.0.0.1:0").unwrap();
    probe.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
    let tiny = "OPTIONS sip:x SIP/2.0\r\nv: SIP/2.0/UDP 1.2.3.4\r\ni: 1\r\nCSeq: 1 OPTIONS\r\nf: <sip:a@b>\r\nt: <sip:a@b>\r\n\r\n";
    probe.send_to(tiny.as_bytes(), server.local_addr()).unwrap();
    let mut buf = [0u8; 2048];
    assert!(probe.recv_from(&mut buf).is_err(), "pas de réponse plus grosse que la requête");

    // Appel sans enregistrement valide : défi, puis refus
    let intrus = Ua::new(&server, "inconnu", "x");
    intrus.dial(ECHO);
    let r = intrus.expect(&mut [], "refus", |m| m.code().is_some_and(|c| c >= 400 && c != 407));
    assert_eq!(r.code(), Some(403));
    assert_eq!(server.stats().calls, 0);
}

#[test]
fn un_register_sobre_recoit_quand_meme_son_defi() {
    let server = start_server();
    let ua = Ua::new(&server, ATA.0, ATA.1);
    let l = ua.local();
    ua.send(&format!(
        "REGISTER sip:{} SIP/2.0\r\nVia: SIP/2.0/UDP {l};branch=z9hG4bK1\r\nFrom: <sip:ata1@v34net>;tag=1\r\nTo: <sip:ata1@v34net>\r\n\
         Call-ID: 1\r\nCSeq: 1 REGISTER\r\nContact: <sip:ata1@{l}>\r\nContent-Length: 0\r\n\r\n",
        server.local_addr()
    ));
    ua.expect(&mut [], "401 malgré une requête plus petite que le défi", |m| m.code() == Some(401));
}

/// Requête du poste dans un dialogue établi, avec un corps SDP.
fn reinvite(ua: &Ua, target: &str, from: &str, to: &str, call_id: &str, cseq: u32, sdp: &str) -> String {
    format!(
        "INVITE {target} SIP/2.0\r\nVia: SIP/2.0/UDP {};branch=z9hG4bK{};rport\r\nMax-Forwards: 70\r\nFrom: {from}\r\nTo: {to}\r\n\
         Call-ID: {call_id}\r\nCSeq: {cseq} INVITE\r\nContact: <sip:{}@{}>\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{sdp}",
        ua.local(),
        rand_tag(),
        ua.user,
        ua.local(),
        sdp.len()
    )
}

fn ack(ua: &Ua, target: &str, from: &str, to: &str, call_id: &str, cseq: u32) -> String {
    format!(
        "ACK {target} SIP/2.0\r\nVia: SIP/2.0/UDP {};branch=z9hG4bK{};rport\r\nMax-Forwards: 70\r\nFrom: {from}\r\nTo: {to}\r\n\
         Call-ID: {call_id}\r\nCSeq: {cseq} ACK\r\nContent-Length: 0\r\n\r\n",
        ua.local(),
        rand_tag()
    )
}

/// Le poste tente T.38 (refusé, l'appel continue), puis renvoie une offre
/// audio (acceptée) ; chaque réponse cesse d'être répétée après l'ACK.
fn re_invites(ua: &Ua, a: &mut CentralClient, target: &str, from: &str, to: &str, call_id: &str, cseq: u32) {
    let t38 = format!(
        "v=0\r\no=- 1 2 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio 0 RTP/AVP 8\r\nm=image {} udptl t38\r\na=T38FaxVersion:0\r\n",
        ua.rtp_port() + 2
    );
    ua.send(&reinvite(ua, target, from, to, call_id, cseq, &t38));
    let r = ua.expect(&mut [&mut *a], "488 au re-INVITE T.38", |m| m.cseq() == Some((cseq, "INVITE")) && m.code().is_some_and(|c| c >= 200));
    assert_eq!(r.code(), Some(488));
    ua.send(&ack(ua, target, from, to, call_id, cseq));
    ua.send(&reinvite(ua, target, from, to, call_id, cseq + 1, &ua.sdp()));
    let r = ua.expect(&mut [&mut *a], "200 au re-INVITE audio", |m| m.cseq() == Some((cseq + 1, "INVITE")) && m.code().is_some_and(|c| c >= 200));
    assert_eq!(r.code(), Some(200));
    assert_eq!(sdp::Session::parse(r.body_str()).unwrap().payloads, vec![8]);
    ua.send(&ack(ua, target, from, to, call_id, cseq + 1));
    // plus de répétition, et l'appel tient
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(1500) {
        a.poll().unwrap();
        if let Some(m) = ua.recv() {
            assert!(m.code().is_none() || m.cseq().is_some_and(|c| c.0 < cseq), "réponse répétée après l'ACK : {}", m.summary());
            assert_ne!(m.method(), Some("BYE"), "l'appel ne doit pas tomber");
        }
        while let Some(ev) = a.next_event() {
            assert!(!matches!(ev, CentralEvent::HungUp { .. }), "l'appel ne doit pas tomber");
        }
    }
}

#[test]
fn re_invites_t38_refuse_et_audio_accepte_quand_le_poste_appelle() {
    let server = start_server();
    let ua = Ua::new(&server, ATA.0, ATA.1);
    assert_eq!(ua.register().code(), Some(200));
    let mut a = alice(&server);
    let (call_id, cseq, _) = ua.dial(ALICE.2);
    wait_event(&mut a, "appel chez alice", |e| matches!(e, CentralEvent::Incoming { .. }));
    a.answer().unwrap();
    let ok = ua.expect(&mut [&mut a], "200", |m| m.code() == Some(200) && m.cseq() == Some((cseq, "INVITE")));
    ua.send(&ua.in_dialog("ACK", &ok, cseq));
    let target = message::addr_uri(ok.header("Contact").unwrap()).to_string();
    re_invites(&ua, &mut a, &target, ok.header("From").unwrap(), ok.header("To").unwrap(), &call_id, cseq + 1);
    // l'audio passe toujours
    let srv_media = media(&ok);
    let tone = rtp::encode_payload(Codec::Alaw, &[0.125; FRAME_SAMPLES]);
    let (mut heard, mut out) = (false, vec![0.0f32; FRAME_SAMPLES]);
    for seq in 0..30u16 {
        ua.send_rtp(srv_media, seq, &tone);
        a.send_block(&[0.0; FRAME_SAMPLES]).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        a.poll().unwrap();
        heard |= a.recv_block(&mut out) && out.iter().all(|&s| (s - 0.125).abs() < 0.01);
    }
    assert!(heard, "alice entend toujours le poste");
}

#[test]
fn re_invites_t38_refuse_et_audio_accepte_quand_on_appelle_le_poste() {
    let server = start_server();
    let ua = Ua::new(&server, ATA.0, ATA.1);
    assert_eq!(ua.register().code(), Some(200));
    let mut a = alice(&server);
    a.dial(ATA.2).unwrap();
    let inv = ua.expect(&mut [&mut a], "INVITE du central", |m| m.method() == Some("INVITE"));
    ua.send(&ua.respond(&inv, 200, true));
    ua.expect(&mut [&mut a], "ACK du 200", |m| m.method() == Some("ACK"));
    wait_event(&mut a, "ANSWER chez alice", |e| matches!(e, CentralEvent::Answered { .. }));
    // dans ce dialogue, le poste est l'appelé : From = lui (avec son tag), To = le central
    let target = message::addr_uri(inv.header("Contact").unwrap()).to_string();
    let from = format!("{};tag={}", inv.header("To").unwrap(), ua.tag);
    re_invites(&ua, &mut a, &target, &from, inv.header("From").unwrap(), inv.call_id().unwrap(), 1);
    let want = rtp::encode_payload(Codec::Alaw, &[0.25; FRAME_SAMPLES]);
    let mut got = 0;
    for _ in 0..30 {
        a.send_block(&[0.25; FRAME_SAMPLES]).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        got += ua.drain_rtp().iter().filter(|(_, p)| *p == want).count();
    }
    assert!(got >= 20, "le poste reçoit toujours alice : {got}");
}

// ───────────────────── un « vrai » modem derrière l'adaptateur ─────────────────────

/// Bilan d'un appel modem à travers le faux adaptateur.
struct Bilan {
    connected: Option<(u32, u32)>,
    echoed: bool,
    /// Trames que l'adaptateur a dû inventer, tampon vide (glissements)
    underruns: u32,
    problems: Vec<String>,
}

/// Un modem logiciel joue le modem branché sur l'adaptateur : il appelle la
/// ligne de test, dont le modem répond en RTP. L'adaptateur a sa propre
/// horloge, décalée de `skew` (0,01 : 1 % plus rapide), et un tampon fixe
/// de trois trames, comme du matériel. Le modem du central doit suivre
/// cette horloge, sinon le tampon de l'adaptateur se vide (ou déborde).
fn modem_derriere_l_ata(skew: f64, appli: bool) -> Bilan {
    use v34_core::{Modem, ModemCommand, ModemConfig, ModemEvent, Role};
    let server = start_server();
    let modem_alice = appli.then(|| ModemAlice::start(server.local_addr()));
    let ua = Ua::new(&server, ATA.0, ATA.1);
    assert_eq!(ua.register().code(), Some(200));
    let (_, cseq, _) = ua.dial(if appli { ALICE.2 } else { ECHO });
    let ok = ua.expect(&mut [], "200 OK", |m| m.code() == Some(200) && m.cseq() == Some((cseq, "INVITE")));
    ua.send(&ua.in_dialog("ACK", &ok, cseq));
    let srv_media = media(&ok);

    let mut modem = Modem::new(Role::Call, ModemConfig::default());
    modem.command(ModemCommand::Dial);
    let payload: Vec<u8> = (0..3000).map(|i| b"Un vrai modem, un ATA, le central. "[i % 35]).collect();
    let mut b = Bilan { connected: None, echoed: false, underruns: 0, problems: Vec::new() };
    let mut fifo: std::collections::VecDeque<Vec<u8>> = Default::default();
    let mut playing = false;
    let (mut rx, mut tx) = (vec![0.0f32; FRAME_SAMPLES], vec![0.0f32; FRAME_SAMPLES]);
    let mut got = Vec::new();
    let period = Duration::from_secs_f64(0.020 / (1.0 + skew));
    let (t0, mut next, mut seq) = (Instant::now(), Instant::now(), 0u16);
    while t0.elapsed() < Duration::from_secs(40) && !b.echoed {
        for (h, p) in ua.drain_rtp() {
            if h.pt == 8 && p.len() == FRAME_SAMPLES && fifo.len() < 50 {
                fifo.push_back(p);
            }
        }
        playing |= fifo.len() >= 3;
        let frame = if playing { fifo.pop_front() } else { None };
        if playing && frame.is_none() {
            b.underruns += 1;
        }
        let frame = frame.unwrap_or_else(|| vec![0xD5; FRAME_SAMPLES]);
        rx.copy_from_slice(&rtp::decode_payload(Codec::Alaw, &frame));
        modem.process(&rx, &mut tx);
        ua.send_rtp(srv_media, seq, &rtp::encode_payload(Codec::Alaw, &tx));
        seq = seq.wrapping_add(1);
        for ev in modem.drain_events() {
            match ev {
                ModemEvent::Connected { tx_rate, rx_rate } => {
                    b.connected.get_or_insert((tx_rate, rx_rate));
                    modem.command(ModemCommand::Send(payload.clone()));
                }
                ModemEvent::Data(d) => got.extend(d),
                ModemEvent::Disconnected { reason } => b.problems.push(format!("NO CARRIER {reason}")),
                ModemEvent::Log { msg, .. } if msg.contains("renégociation") || msg.to_lowercase().contains("retrain") || msg.contains("réapprentissage") => {
                    b.problems.push(msg)
                }
                _ => {}
            }
        }
        b.echoed = got.ends_with(&payload);
        next += period;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        }
    }
    let cadence = modem_alice.map(ModemAlice::stop).map_or(String::new(), |(e, k)| format!(", appli +{e}/−{k} blocs"));
    eprintln!(
        "{} horloge de l'ATA {:+.1} % : CONNECT {:?}, écho {}, trous {}, tampon {} trames, problèmes {:?}, {:.1} s{cadence}",
        if appli { "ATA → appli," } else { "ATA → ligne de test," },
        skew * 100.0,
        b.connected,
        if b.echoed { "OK" } else { "ÉCHEC" },
        b.underruns,
        fifo.len(),
        b.problems,
        t0.elapsed().as_secs_f32()
    );
    b
}

#[test]
#[ignore = "temps réel (~20 s) : cargo test --release -p v34-server --test sip -- --ignored --nocapture"]
fn un_modem_derriere_un_ata_rapide_connecte_la_ligne_de_test() {
    let b = modem_derriere_l_ata(0.01, false);
    assert!(b.connected.is_some() && b.echoed, "{:?}", b.problems);
    assert!(b.underruns <= 1, "le modem du central ne suit pas l'horloge de l'ATA : {} trous", b.underruns);
}

#[test]
#[ignore = "temps réel (~20 s)"]
fn un_modem_derriere_un_ata_lent_connecte_la_ligne_de_test() {
    let b = modem_derriere_l_ata(-0.01, false);
    assert!(b.connected.is_some() && b.echoed, "{:?}", b.problems);
    assert!(b.underruns <= 1, "{} trous", b.underruns);
}

/// Le modem de l'appli (un abonné du central), qui répond et renvoie en
/// écho ce qu'il reçoit, cadencé comme l'appli (`Pacer` calé sur l'émetteur).
struct ModemAlice {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    join: std::thread::JoinHandle<(u64, u64)>,
}

impl ModemAlice {
    fn start(server: SocketAddr) -> ModemAlice {
        use std::sync::atomic::{AtomicBool, Ordering};
        use v34_core::{Modem, ModemCommand, ModemConfig, ModemEvent, Role};
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let join = std::thread::spawn(move || {
            let mut cfg = CentralConfig::new(server.to_string(), ALICE.0, ALICE.1);
            cfg.codec = Codec::Alaw;
            let mut c = CentralClient::connect(cfg).unwrap();
            let mut modem = Modem::new(Role::Answer, ModemConfig::default());
            let mut pacer = v34_net::Pacer::new();
            let (mut rx, mut tx) = (vec![0.0f32; FRAME_SAMPLES], vec![0.0f32; FRAME_SAMPLES]);
            let (mut next, mut due) = (Instant::now(), 0.0);
            while !flag.load(Ordering::Relaxed) {
                let _ = c.poll();
                while let Some(ev) = c.next_event() {
                    match ev {
                        CentralEvent::Registered { .. } => {
                            let _ = ready_tx.send(());
                        }
                        CentralEvent::Incoming { .. } => {
                            let _ = c.answer();
                            modem.command(ModemCommand::Answer);
                        }
                        _ => {}
                    }
                }
                let active = matches!(c.call_state(), v34_net::central::CallState::Active { .. });
                due += 1.0 + pacer.update(c.jitter_level());
                while due >= 1.0 {
                    if active {
                        c.recv_block(&mut rx);
                    } else {
                        rx.fill(0.0);
                    }
                    modem.process(&rx, &mut tx);
                    if active {
                        let _ = c.send_block(&tx);
                    }
                    due -= 1.0;
                }
                for ev in modem.drain_events() {
                    if let ModemEvent::Data(d) = ev {
                        modem.command(ModemCommand::Send(d));
                    }
                }
                next += Duration::from_millis(20);
                let now = Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                }
            }
            (pacer.extra, pacer.skipped)
        });
        ready_rx.recv_timeout(Duration::from_secs(5)).expect("alice enregistrée");
        ModemAlice { stop, join }
    }

    fn stop(self) -> (u64, u64) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        self.join.join().unwrap()
    }
}

#[test]
#[ignore = "temps réel (~25 s)"]
fn un_modem_derriere_un_ata_appelle_le_modem_de_l_appli() {
    for skew in [0.0, 0.01, -0.01] {
        let b = modem_derriere_l_ata(skew, true);
        assert!(b.connected.is_some() && b.echoed, "horloge {skew:+} : {:?}", b.problems);
    }
}
