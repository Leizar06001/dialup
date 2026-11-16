//! Écho lointain : à travers un adaptateur téléphonique (ATA), notre propre
//! signal revient, réfléchi par l'hybride de sa prise, après tout
//! l'aller-retour sur Internet (0,4 à 0,6 s mesurés avec un vrai modem).
//! L'annuleur d'écho proche (32 ms) ne le voit pas ; sans annuleur d'écho
//! lointain, la réception s'effondre dès que les deux modems émettent en
//! même temps (phase 4, données).

use v34_core::{EchoPath, LineParams, LineSim, Modem, ModemCommand, ModemConfig, ModemEvent, Role};

struct Run {
    connected: [Option<(u32, u32)>; 2],
    got: [Vec<u8>; 2],
    log: Vec<String>,
}

/// Ligne de `delay_ms` dans chaque sens ; chaque modem reçoit son propre
/// écho `echo_db` après un aller-retour complet.
fn run(delay_ms: f32, echo_db: f32, seconds: f32, payload: [&[u8]; 2]) -> Run {
    let p = LineParams { delay_ms, snr_db: 40.0, ..LineParams::perfect() };
    let e = LineParams { echo_db, echo_delay_ms: 2.0 * delay_ms, ..LineParams::perfect() };
    let mut call = Modem::new(Role::Call, ModemConfig::default());
    let mut ans = Modem::new(Role::Answer, ModemConfig::default());
    let mut c2a = LineSim::new(p.clone());
    let mut a2c = LineSim::new(p);
    let mut echo_c = EchoPath::new(e.clone());
    let mut echo_a = EchoPath::new(e);
    call.command(ModemCommand::Dial);
    ans.command(ModemCommand::Answer);
    const B: usize = 160;
    let (mut call_rx, mut ans_rx) = ([0.0f32; B], [0.0f32; B]);
    let (mut call_tx, mut ans_tx) = ([0.0f32; B], [0.0f32; B]);
    let mut r = Run { connected: [None, None], got: [Vec::new(), Vec::new()], log: Vec::new() };
    let mut sent = [false, false];
    for blk in 0..(seconds * 8000.0 / B as f32) as usize {
        call.process(&call_rx, &mut call_tx);
        ans.process(&ans_rx, &mut ans_tx);
        c2a.process(&call_tx, &mut ans_rx);
        echo_a.process(&ans_tx, &mut ans_rx);
        a2c.process(&ans_tx, &mut call_rx);
        echo_c.process(&call_tx, &mut call_rx);
        for (i, m) in [&mut call, &mut ans].into_iter().enumerate() {
            for ev in m.drain_events() {
                match ev {
                    ModemEvent::Connected { tx_rate, rx_rate } => {
                        r.connected[i] = Some((tx_rate, rx_rate));
                        r.log.push(format!("[{:6} ms] {i} CONNECT {tx_rate}/{rx_rate}", blk * 20));
                    }
                    ModemEvent::PhaseChanged(ph) => r.log.push(format!("[{:6} ms] {i} phase {ph:?}", blk * 20)),
                    ModemEvent::Data(d) => r.got[i].extend(d),
                    ModemEvent::Log { msg, .. } => r.log.push(format!("[{:6} ms] {} {msg}", blk * 20, if i == 0 { "APPELANT" } else { "APPELÉ  " })),
                    ModemEvent::Disconnected { reason } => r.log.push(format!("[{:6} ms] {i} NO CARRIER {reason}", blk * 20)),
                    _ => {}
                }
            }
            if r.connected[i].is_some() && !sent[i] {
                sent[i] = true;
                m.command(ModemCommand::Send(payload[i].to_vec()));
            }
        }
        if r.got[0].len() >= payload[1].len() && r.got[1].len() >= payload[0].len() && sent[0] && sent[1] {
            break;
        }
    }
    r
}

#[test]
fn donnees_malgre_un_echo_lointain_d_adaptateur() {
    let a: Vec<u8> = "Bonjour depuis l'appelant, à travers un ATA. ".repeat(8).into_bytes();
    let b: Vec<u8> = "Réponse de l'appelé, malgré l'écho lointain. ".repeat(8).into_bytes();
    // 250 ms par sens (500 ms aller-retour) ; écho à −20 dB, comme un hybride
    // d'ATA mal adapté
    let r = run(250.0, -20.0, 45.0, [&a, &b]);
    let dump = r.log.join("\n");
    let (Some(c), Some(d)) = (r.connected[0], r.connected[1]) else { panic!("pas de CONNECT\n{dump}") };
    eprintln!("CONNECT appelant {c:?}, appelé {d:?}");
    let trace = std::env::var("ECHO_TRACE").is_ok();
    for l in r.log.iter().filter(|l| trace || l.contains("SNR mesuré") || l.contains("Annuleur")) {
        eprintln!("{l}");
    }
    assert_eq!(r.got[1], a, "appelant → appelé\n{dump}");
    assert_eq!(r.got[0], b, "appelé → appelant\n{dump}");
    // le débit ne doit pas s'effondrer à cause de notre propre écho
    assert!(c.1 >= 21600 && d.1 >= 21600, "débits de réception trop bas : {c:?} / {d:?}\n{dump}");
}

#[test]
#[ignore = "référence : la même ligne sans écho"]
fn reference_sans_echo() {
    let a = b"x".repeat(200);
    let r = run(250.0, -100.0, 30.0, [&a, &a]);
    eprintln!("sans écho : {:?} / {:?}", r.connected[0], r.connected[1]);
    for l in r.log.iter().filter(|l| l.contains("SNR mesuré")) {
        eprintln!("{l}");
    }
}
