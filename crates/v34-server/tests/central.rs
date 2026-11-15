//! Test d'intégration du central : serveur sur 127.0.0.1 (port aléatoire),
//! clients réels (`v34_net::central::CentralClient`) sur UDP.

use std::net::UdpSocket;
use std::time::{Duration, Instant};

use v34_net::auth::PasswordHash;
use v34_net::central::{CallState, CentralClient, CentralConfig, CentralEvent};
use v34_net::signal::{AuthFailReason, HangupReason};
use v34_net::{Codec, Packet, FRAME_SAMPLES};
use v34_server::log::{set_level, Level};
use v34_server::{Config, Server, ServerHandle};

/// (nom, mot de passe, numéro attribué par l'annuaire)
const A: (&str, &str, &str) = ("alice", "mot-de-passe-alice", "0142424242");
const B: (&str, &str, &str) = ("bob", "mot-de-passe-bob", "0612345678");
const C: (&str, &str, &str) = ("charlie", "mot-de-passe-charlie", "0700000000");
const ISP: &str = "0836154286";
const ECHO: &str = "0800000000";
const ORACLE: &str = "0836672253";

fn start_server() -> ServerHandle {
    set_level(Level::Error);
    let mut toml = String::from(
        r#"
        [serveur]
        ecoute = "127.0.0.1:0"
        journal = "error"
        [central]
        keepalive_s = 5
        expiration_s = 15
        [modems]
        delai_decroche_ms = 100
        appels_max = 1
        "#,
    );
    for (nom, pw, num) in [A, B, C] {
        // peu d'itérations : le test reste rapide
        let h = PasswordHash::new(pw, 1000);
        toml.push_str(&format!("[[abonnes]]\nnom = \"{nom}\"\nnumero = \"{num}\"\nmot_de_passe_hash = \"{h}\"\n"));
    }
    let cfg = Config::from_toml(&toml).expect("configuration de test");
    Server::bind(cfg).unwrap().spawn().unwrap()
}

fn client(server: &ServerHandle, user: &str, password: &str) -> CentralClient {
    let mut cfg = CentralConfig::new(server.local_addr().to_string(), user, password);
    cfg.codec = Codec::L16; // relais bit à bit vérifiable
    CentralClient::connect(cfg).unwrap()
}

/// Fait tourner les clients jusqu'à ce que `c` produise un événement
/// satisfaisant `pred` (les autres événements sont ignorés).
fn wait_for(c: &mut CentralClient, others: &mut [&mut CentralClient], what: &str, pred: impl Fn(&CentralEvent) -> bool) -> CentralEvent {
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(5) {
        c.poll().unwrap();
        for o in others.iter_mut() {
            o.poll().unwrap();
        }
        while let Some(ev) = c.next_event() {
            if pred(&ev) {
                return ev;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("délai dépassé en attendant : {what}");
}

/// Enregistre un abonné et vérifie le numéro attribué.
fn registered(server: &ServerHandle, (user, pw, number): (&str, &str, &str)) -> CentralClient {
    let mut c = client(server, user, pw);
    match wait_for(&mut c, &mut [], "enregistrement", |e| matches!(e, CentralEvent::Registered { .. })) {
        CentralEvent::Registered { number: n, .. } => assert_eq!(n, number, "numéro attribué par l'annuaire"),
        _ => unreachable!(),
    }
    assert!(c.is_registered());
    assert_eq!(c.number(), Some(number));
    c
}

fn frame(v: f32) -> Vec<f32> {
    vec![v; FRAME_SAMPLES]
}

#[test]
fn appel_entre_deux_abonnes_avec_audio_dans_les_deux_sens() {
    let server = start_server();
    let mut a = registered(&server, A);
    let mut b = registered(&server, B);

    a.dial(B.2).unwrap();
    let call_id = match wait_for(&mut b, &mut [&mut a], "RING chez B", |e| matches!(e, CentralEvent::Incoming { .. })) {
        CentralEvent::Incoming { call_id, from } => {
            assert_eq!(from, A.2, "numéro de l'appelant présenté");
            call_id
        }
        _ => unreachable!(),
    };
    wait_for(&mut a, &mut [&mut b], "RINGING chez A", |e| matches!(e, CentralEvent::Ringing { .. }));
    b.answer().unwrap();
    wait_for(&mut a, &mut [&mut b], "ANSWER chez A", |e| *e == CentralEvent::Answered { call_id });
    assert_eq!(a.call_state(), &CallState::Active { call_id });

    // Un intrus non authentifié qui connaît l'identifiant d'appel ne doit
    // rien pouvoir injecter.
    let intruder = UdpSocket::bind("127.0.0.1:0").unwrap();
    for seq in 0..10 {
        let p = Packet { codec: Codec::L16, call_id, seq, timestamp: seq * FRAME_SAMPLES as u32, samples: frame(0.9), backup: Vec::new() };
        intruder.send_to(&p.encode(), server.local_addr()).unwrap();
    }

    // 25 blocs de 20 ms dans chaque sens.
    let (mut got_at_b, mut got_at_a) = (0, 0);
    let mut out = vec![0.0f32; FRAME_SAMPLES];
    for _ in 0..25 {
        a.send_block(&frame(0.25)).unwrap();
        b.send_block(&frame(-0.5)).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        if b.recv_block(&mut out) {
            assert!(out.iter().all(|&s| (s - 0.25).abs() < 1e-3), "B reçoit l'audio de A (et seulement lui) : {}", out[0]);
            got_at_b += 1;
        }
        if a.recv_block(&mut out) {
            assert!(out.iter().all(|&s| (s + 0.5).abs() < 1e-3), "A reçoit l'audio de B : {}", out[0]);
            got_at_a += 1;
        }
    }
    assert!(got_at_b >= 15 && got_at_a >= 15, "trop peu de blocs relayés : A→B {got_at_b}, B→A {got_at_a}");
    assert!(server.stats().relayed_packets >= 40);
    assert!(server.stats().dropped_packets >= 10, "paquets de l'intrus rejetés");

    // Raccrochage par A : B est prévenu, l'appel disparaît du central.
    a.hangup().unwrap();
    let ev = wait_for(&mut b, &mut [&mut a], "HANGUP chez B", |e| matches!(e, CentralEvent::HungUp { .. }));
    assert_eq!(ev, CentralEvent::HungUp { call_id, reason: HangupReason::Normal });
    assert_eq!(b.call_state(), &CallState::Idle);
    let t0 = Instant::now();
    while server.stats().calls != 0 && t0.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(server.stats().calls, 0);
    assert_eq!(server.stats().registrations, 2);
}

#[test]
fn numero_inconnu_et_occupe() {
    let server = start_server();
    let mut a = registered(&server, A);
    let mut b = registered(&server, B);
    let mut c = registered(&server, C);

    a.dial("0999999999").unwrap();
    let ev = wait_for(&mut a, &mut [], "UNKNOWN_NUMBER", |e| matches!(e, CentralEvent::UnknownNumber { .. }));
    assert_eq!(ev, CentralEvent::UnknownNumber { number: "0999999999".into() });

    a.dial(A.2).unwrap();
    wait_for(&mut a, &mut [], "BUSY (soi-même)", |e| matches!(e, CentralEvent::Busy { .. }));

    // B est en communication avec A : C tombe sur « occupé ».
    a.dial(B.2).unwrap();
    wait_for(&mut b, &mut [&mut a], "RING chez B", |e| matches!(e, CentralEvent::Incoming { .. }));
    c.dial(B.2).unwrap();
    let ev = wait_for(&mut c, &mut [&mut a, &mut b], "BUSY", |e| matches!(e, CentralEvent::Busy { .. }));
    assert_eq!(ev, CentralEvent::Busy { number: B.2.into() });
    assert_eq!(c.call_state(), &CallState::Idle);

    // B refuse (raccroche sans décrocher) : A est prévenu.
    b.hangup().unwrap();
    wait_for(&mut a, &mut [&mut b], "HANGUP chez A", |e| matches!(e, CentralEvent::HungUp { .. }));
}

#[test]
fn identifiants_refuses_sans_dire_pourquoi_puis_limitation() {
    let server = start_server();
    // mauvais mot de passe et nom inconnu : même refus
    for (user, pw) in [(A.0, "pas-le-bon"), ("inconnu", "peu-importe")] {
        let mut bad = client(&server, user, pw);
        let ev = wait_for(&mut bad, &mut [], "AUTH_FAIL", |e| matches!(e, CentralEvent::AuthFailed { .. }));
        assert_eq!(ev, CentralEvent::AuthFailed { reason: AuthFailReason::BadCredentials });
        assert!(!bad.is_registered());
        assert!(bad.dial(B.2).is_err(), "impossible d'appeler sans être enregistré");
    }
    assert_eq!(server.stats().registrations, 0);

    // Au-delà de 5 échecs depuis la même IP : « trop de tentatives », même
    // avec le bon mot de passe.
    for _ in 0..4 {
        let mut bad = client(&server, A.0, "toujours-faux");
        wait_for(&mut bad, &mut [], "AUTH_FAIL", |e| matches!(e, CentralEvent::AuthFailed { .. }));
    }
    let t0 = Instant::now();
    while server.stats().auth_failures < 6 && t0.elapsed() < Duration::from_secs(1) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(server.stats().auth_failures >= 6, "{:?}", server.stats());
    let mut good = client(&server, A.0, A.1);
    let ev = wait_for(&mut good, &mut [], "AUTH_FAIL", |e| matches!(e, CentralEvent::AuthFailed { .. }));
    assert_eq!(ev, CentralEvent::AuthFailed { reason: AuthFailReason::TooManyAttempts });

    // Datagrammes quelconques ou audio sans appel : silence total.
    let raw = UdpSocket::bind("127.0.0.1:0").unwrap();
    raw.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
    raw.send_to(b"bonjour", server.local_addr()).unwrap();
    let p = Packet { codec: Codec::Alaw, call_id: 1234, seq: 0, timestamp: 0, samples: frame(0.1), backup: Vec::new() };
    raw.send_to(&p.encode(), server.local_addr()).unwrap();
    let mut buf = [0u8; 2048];
    assert!(raw.recv_from(&mut buf).is_err(), "le central ne répond pas aux inconnus");
}

/// Appelle un numéro spécial et vérifie que son modem décroche et émet.
fn special_number_answers(number: &str) {
    let server = start_server();
    let mut c = client(&server, C.0, C.1);
    let ev = wait_for(&mut c, &mut [], "enregistrement", |e| matches!(e, CentralEvent::Registered { .. }));
    let CentralEvent::Registered { services, .. } = ev else { unreachable!() };
    assert!(services.contains(&(ISP.to_string(), "fai".to_string())), "FAI annoncé : {services:?}");
    assert!(services.contains(&(ECHO.to_string(), "echo".to_string())), "ligne de test annoncée");
    assert!(services.contains(&(ORACLE.to_string(), "oracle".to_string())), "3615 ORACLE annoncé");
    assert_eq!(c.isp_number(), Some(ISP));

    c.dial(number).unwrap();
    wait_for(&mut c, &mut [], "RINGING", |e| matches!(e, CentralEvent::Ringing { .. }));
    let call_id = match wait_for(&mut c, &mut [], "décroché du modem", |e| matches!(e, CentralEvent::Answered { .. })) {
        CentralEvent::Answered { call_id } => call_id,
        _ => unreachable!(),
    };
    assert_eq!(server.stats().modem_calls, 1);

    // Un second appel vers un numéro spécial est refusé (modems.appels_max = 1).
    let mut a = registered(&server, A);
    a.dial(ISP).unwrap();
    wait_for(&mut a, &mut [&mut c], "BUSY", |e| matches!(e, CentralEvent::Busy { .. }));

    // Le modem répond (silence ≥ 200 ms puis ANSam à 2100 Hz, ou l'ANS pur
    // d'un serveur Minitel) : on doit entendre un signal non nul en moins de 2 s.
    let mut out = vec![0.0f32; FRAME_SAMPLES];
    let mut max_rms = 0.0f32;
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(2000) && max_rms < 0.02 {
        c.send_block(&frame(0.0)).unwrap();
        if c.recv_block(&mut out) {
            let rms = (out.iter().map(|s| s * s).sum::<f32>() / out.len() as f32).sqrt();
            max_rms = max_rms.max(rms);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(max_rms >= 0.02, "le modem du {number} n'émet rien (RMS max {max_rms})");
    assert!(c.has_peer_audio());
    assert_eq!(c.call_state(), &CallState::Active { call_id });

    // On raccroche : l'appel et le modem disparaissent.
    c.hangup().unwrap();
    let t0 = Instant::now();
    while (server.stats().modem_calls != 0 || server.stats().calls != 0) && t0.elapsed() < Duration::from_secs(3) {
        c.poll().unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(server.stats().modem_calls, 0);
}

#[test]
fn le_fai_decroche_et_emet_de_l_audio() {
    special_number_answers(ISP);
}

#[test]
fn la_ligne_de_test_decroche_et_emet_de_l_audio() {
    special_number_answers(ECHO);
}

#[test]
fn un_numero_cache_decroche_et_emet_de_l_audio() {
    special_number_answers("0836993699");
}

#[test]
fn l_oracle_decroche_et_emet_de_l_audio() {
    special_number_answers(ORACLE);
}

/// Fait tourner les clients jusqu'à ce que `pred` soit vrai (ou 5 s).
fn poll_until(clients: &mut [&mut CentralClient], what: &str, pred: impl Fn(&[&mut CentralClient]) -> bool) {
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(5) {
        for c in clients.iter_mut() {
            c.poll().unwrap();
            while c.next_event().is_some() {}
        }
        if pred(clients) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("délai dépassé en attendant : {what}");
}

#[test]
fn annuaire_avec_presence() {
    use v34_net::signal::Presence;
    let server = start_server();
    let mut a = registered(&server, A);
    let mut b = registered(&server, B);
    let status = |c: &CentralClient, num: &str| c.directory().iter().find(|e| e.number == num).map(|e| e.status);
    // Le premier annuaire est demandé dès l'enregistrement ; on force un
    // rafraîchissement pour voir B (enregistré après A).
    a.refresh_directory();
    poll_until(&mut [&mut a, &mut b], "annuaire de A", |c| status(c[0], B.2) == Some(Presence::Online));
    let dir = a.directory().to_vec();
    assert_eq!(dir.len(), 3, "{dir:?}");
    assert_eq!(status(&a, A.2), Some(Presence::Online));
    assert_eq!(status(&a, C.2), Some(Presence::Absent));
    assert_eq!(dir.iter().find(|e| e.number == B.2).unwrap().name, "bob");

    // B appelle A : les deux passent « en communication ».
    b.dial(A.2).unwrap();
    wait_for(&mut a, &mut [&mut b], "RING", |e| matches!(e, CentralEvent::Incoming { .. }));
    std::thread::sleep(Duration::from_millis(1100));
    b.refresh_directory();
    poll_until(&mut [&mut a, &mut b], "B occupé", |c| status(c[1], A.2) == Some(Presence::Busy) && status(c[1], B.2) == Some(Presence::Busy));
}

/// Comptes du site : sans accès Internet, le FAI refuse ; accordé, il décroche.
#[test]
fn acces_fai_des_comptes_du_site() {
    use v34_server::comptes::Comptes;
    set_level(Level::Error);
    let mut toml = String::from("[serveur]\necoute = \"127.0.0.1:0\"\njournal = \"error\"\n[modems]\ndelai_decroche_ms = 100\n");
    let h = PasswordHash::new(A.1, 1000);
    toml.push_str(&format!("[[abonnes]]\nnom = \"{}\"\nnumero = \"{}\"\nmot_de_passe_hash = \"{h}\"\n", A.0, A.2));
    let comptes = Comptes::en_memoire();
    let hg = PasswordHash::new("mdp-gaston", 1000).to_string();
    let jeton = comptes.inscrire("gaston", "0699999999", &hg, "gaston@example.org").unwrap();
    comptes.valider(&jeton, |_, _| true).unwrap();
    let server = Server::bind(Config::from_toml(&toml).unwrap()).unwrap().with_comptes(comptes.clone()).spawn().unwrap();

    // gaston s'enregistre avec le numéro choisi sur le site…
    let mut g = registered(&server, ("gaston", "mdp-gaston", "0699999999"));
    // … mais le FAI lui est refusé tant que l'administrateur n'a pas accordé l'accès.
    g.dial(ISP).unwrap();
    match wait_for(&mut g, &mut [], "refus du FAI", |e| matches!(e, CentralEvent::HungUp { .. })) {
        CentralEvent::HungUp { reason, .. } => assert_eq!(reason, HangupReason::Forbidden),
        _ => unreachable!(),
    }
    assert_eq!(server.stats().modem_calls, 0);
    // 3615 ORACLE aussi (par défaut, il est réservé aux abonnés du FAI).
    g.dial(ORACLE).unwrap();
    match wait_for(&mut g, &mut [], "refus de l'oracle", |e| matches!(e, CentralEvent::HungUp { .. })) {
        CentralEvent::HungUp { reason, .. } => assert_eq!(reason, HangupReason::Forbidden),
        _ => unreachable!(),
    }
    assert_eq!(server.stats().modem_calls, 0);
    // La ligne de test reste accessible à tous.
    g.dial(ECHO).unwrap();
    wait_for(&mut g, &mut [], "écho décroche", |e| matches!(e, CentralEvent::Answered { .. }));
    g.hangup().unwrap();
    std::thread::sleep(Duration::from_millis(300));

    // Accès accordé : pris en compte à chaud, pour le FAI comme pour l'oracle.
    comptes.regler_fai("gaston", true).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    g.dial(ORACLE).unwrap();
    wait_for(&mut g, &mut [], "l'oracle décroche", |e| matches!(e, CentralEvent::Answered { .. }));
    g.hangup().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    g.dial(ISP).unwrap();
    wait_for(&mut g, &mut [], "le FAI décroche", |e| matches!(e, CentralEvent::Answered { .. }));
    // Accès retiré pendant l'appel : le central raccroche.
    comptes.regler_fai("gaston", false).unwrap();
    match wait_for(&mut g, &mut [], "coupure", |e| matches!(e, CentralEvent::HungUp { .. })) {
        CentralEvent::HungUp { reason, .. } => assert_eq!(reason, HangupReason::Forbidden),
        _ => unreachable!(),
    }
    // Compte supprimé : déconnecté, et ne peut plus s'enregistrer.
    comptes.supprimer("gaston").unwrap();
    // (nom inconnu : PBKDF2 complet côté client, lent en debug)
    let mut back = client(&server, "gaston", "mdp-gaston");
    let t0 = Instant::now();
    loop {
        back.poll().unwrap();
        if std::iter::from_fn(|| back.next_event()).any(|e| matches!(e, CentralEvent::AuthFailed { .. })) {
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(60), "le compte supprimé devrait être refusé");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Le central publie son état chaque seconde (page « État du réseau » du site).
#[test]
fn etat_publie_pour_le_site() {
    let dir = std::env::temp_dir().join(format!("v34-etat-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("etat.json");
    set_level(Level::Error);
    let h = PasswordHash::new(A.1, 1000);
    let toml = format!("[serveur]\necoute = \"127.0.0.1:0\"\n[[abonnes]]\nnom = \"{}\"\nnumero = \"{}\"\nmot_de_passe_hash = \"{h}\"\n", A.0, A.2);
    let server = Server::bind(Config::from_toml(&toml).unwrap()).unwrap().with_status_file(path.clone()).spawn().unwrap();
    let _a = registered(&server, A);
    std::thread::sleep(Duration::from_millis(1300));
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["abonnes"], 1);
    assert_eq!(v["connectes"], 1);
    assert_eq!(v["appels"], 0);
    assert_eq!(v["modems"]["max"], 4);
    assert_eq!(v["salon"], 0);
    assert!(v["maj"].as_u64().unwrap() >= v["depuis"].as_u64().unwrap());
    drop(server);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn facture_et_succes_apres_un_appel() {
    let dir = std::env::temp_dir().join(format!("v34-facture-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    set_level(Level::Error);
    let mut toml = String::from("[serveur]\necoute = \"127.0.0.1:0\"\n");
    for (nom, pw, num) in [A, B] {
        let h = PasswordHash::new(pw, 1000);
        toml.push_str(&format!("[[abonnes]]\nnom = \"{nom}\"\nnumero = \"{num}\"\nmot_de_passe_hash = \"{h}\"\n"));
    }
    let server = Server::bind(Config::from_toml(&toml).unwrap()).unwrap().with_facturation(dir.clone()).spawn().unwrap();
    let mut a = registered(&server, A);
    let mut b = registered(&server, B);
    a.dial(B.2).unwrap();
    wait_for(&mut b, &mut [&mut a], "RING chez B", |e| matches!(e, CentralEvent::Incoming { .. }));
    b.answer().unwrap();
    wait_for(&mut a, &mut [&mut b], "ANSWER chez A", |e| matches!(e, CentralEvent::Answered { .. }));
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(1100) {
        a.poll().unwrap();
        b.poll().unwrap();
        std::thread::sleep(Duration::from_millis(10));
    }
    a.hangup().unwrap();
    // L'appelant paie le crédit temps (0,74 F) ; l'appelé, rien
    let ev = wait_for(&mut a, &mut [&mut b], "COMPTE de A", |e| matches!(e, CentralEvent::Compte(c) if c.centimes > 0));
    let CentralEvent::Compte(c) = ev else { unreachable!() };
    assert_eq!((c.centimes, c.secondes), (74, 1));
    assert!(c.succes.iter().any(|s| s == "premier-appel"));
    wait_for(&mut b, &mut [&mut a], "COMPTE de B", |e| matches!(e, CentralEvent::Compte(c) if c.succes.iter().any(|s| s == "recu") && c.centimes == 0));
    // Un succès constaté par l'appli
    a.exploit("v90");
    wait_for(&mut a, &mut [&mut b], "v90 noté", |e| matches!(e, CentralEvent::Compte(c) if c.succes.iter().any(|s| s == "v90")));
    assert_eq!(v34_server::facturation::lire_tickets(&dir, c.mois).len(), 1);
    drop(server);
    let _ = std::fs::remove_dir_all(dir);
}

/// De bout en bout : un modem appelle un numéro caché (la Commission de
/// l'an 2000) à travers le central, reconnaît le sifflement du serveur
/// Minitel, passe en V.23, reçoit sa page en Vidéotex, tape « 3 » puis
/// ENVOI au clavier, lit la réponse, et termine par CONNEXION/FIN.
#[test]
fn minitel_de_bout_en_bout() {
    use v34_core::{Modem, ModemCommand, ModemConfig, ModemEvent, Phase, Role};
    use v34_net::videotex::{self as vt, Touche};

    let server = start_server();
    let mut c = registered(&server, C);
    c.dial("0800200000").unwrap();
    wait_for(&mut c, &mut [], "décroché", |e| matches!(e, CentralEvent::Answered { .. }));
    let mut modem = Modem::new(Role::Call, ModemConfig::default());
    modem.command(ModemCommand::Dial);
    let mut ecran = vt::Ecran::new();
    let (mut rx, mut tx) = (vec![0.0f32; FRAME_SAMPLES], vec![0.0f32; FRAME_SAMPLES]);
    let t0 = Instant::now();
    let mut next = t0;
    let (mut connect, mut minitel, mut fin) = (None, false, None);
    let (mut choisi, mut au_revoir) = (false, false);
    while t0.elapsed() < Duration::from_secs(45) && fin.is_none() {
        c.recv_block(&mut rx);
        modem.process(&rx, &mut tx);
        c.send_block(&tx).unwrap();
        for ev in modem.drain_events() {
            match ev {
                ModemEvent::PhaseChanged(Phase::Minitel) => minitel = true,
                ModemEvent::Connected { tx_rate, rx_rate } => {
                    assert_eq!((tx_rate, rx_rate), (75, 1200));
                    connect = Some(t0.elapsed());
                    ecran.connexion(true);
                }
                ModemEvent::Data(d) => ecran.recevoir(&d),
                ModemEvent::Disconnected { reason } => fin = Some(reason),
                _ => {}
            }
        }
        let texte = ecran.texte();
        if !choisi && texte.contains("CONNEXION/FIN : raccrocher.") {
            choisi = true;
            let mut b = b"3".to_vec();
            b.extend(Touche::Envoi.octets());
            modem.command(ModemCommand::Send(b));
        }
        if choisi && !au_revoir && texte.contains("Autre chose ?") {
            au_revoir = true;
            assert!(texte.contains("bogue"), "la réponse au 3 : {texte}");
            modem.command(ModemCommand::Send(Touche::ConnexionFin.octets().to_vec()));
        }
        next += Duration::from_millis(20);
        if let Some(d) = next.checked_duration_since(Instant::now()) {
            std::thread::sleep(d);
        }
    }
    let connect = connect.expect("CONNECT");
    assert!(minitel, "passé en Minitel");
    assert!(connect < Duration::from_secs(6), "CONNECT en {connect:?}");
    assert!(au_revoir, "écran : {}", ecran.texte());
    // le service a raccroché après son au revoir ; le Minitel l'a vu
    assert!(ecran.texte().contains("Au revoir !"), "{}", ecran.texte());
    assert!(fin.is_some(), "fin de l'appel");
    // la page : le titre en double hauteur sur son bandeau
    assert!(ecran.texte_rangee(2).contains("COMMISSION AN 2000"), "{}", ecran.texte());
    assert_ne!(ecran.cellule(2, 2).attr & vt::A_DOUBLE_HAUTEUR, 0);
}
