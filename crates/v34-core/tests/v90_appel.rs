//! Appels complets avec le 56k : l'abonné (appelant, modem analogique) et le
//! FAI (appelé, raccordé en numérique). Même banc que `connect.rs`.

use v34_core::{EchoPath, LineParams, LineSim, Modem, ModemCommand, ModemConfig, ModemEvent, Role};

struct Run {
    connected: [Option<(u32, u32)>; 2],
    /// Instant du CONNECT de l'abonné (ms)
    t_connect: Option<usize>,
    got: [Vec<u8>; 2],
    log: Vec<String>,
}

fn fai() -> ModemConfig {
    ModemConfig { digital: true, ..ModemConfig::default() }
}

fn run_with(p: LineParams, abonne: ModemConfig, fai: ModemConfig, seconds: f32, payload: [&[u8]; 2]) -> Run {
    let mut call = Modem::new(Role::Call, abonne);
    let mut ans = Modem::new(Role::Answer, fai);
    // le sens montant passe par la boucle locale de l'abonné comme le
    // descendant (le FAI, lui, n'a pas de boucle analogique)
    let mut c2a = LineSim::new(p.clone());
    let mut a2c = LineSim::new(p.clone());
    let mut echo_c = EchoPath::new(p.clone());
    // écho du côté du FAI aussi (hybride de la carte d'abonné du central)
    let mut echo_a = EchoPath::new(p);
    call.command(ModemCommand::Dial);
    ans.command(ModemCommand::Answer);
    const B: usize = 160;
    let (mut call_rx, mut ans_rx) = ([0.0f32; B], [0.0f32; B]);
    let (mut call_tx, mut ans_tx) = ([0.0f32; B], [0.0f32; B]);
    let mut r = Run { connected: [None, None], t_connect: None, got: [Vec::new(), Vec::new()], log: Vec::new() };
    let mut sent = [false, false];
    let blocks = (seconds * 8000.0 / B as f32) as usize;
    for blk in 0..blocks {
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
                        if i == 0 {
                            r.t_connect.get_or_insert(blk * 20);
                        }
                    }
                    ModemEvent::Data(d) => r.got[i].extend(d),
                    ModemEvent::Log { msg, .. } => r.log.push(format!("[{:6} ms] {} {msg}", blk * 20, if i == 0 { "ABONNÉ" } else { "FAI   " })),
                    ModemEvent::TxSignal(t) => r.log.push(format!("[{:6} ms] {} → {t}", blk * 20, if i == 0 { "ABONNÉ" } else { "FAI   " })),
                    ModemEvent::RxSignal(t) => r.log.push(format!("[{:6} ms] {} ← {t}", blk * 20, if i == 0 { "ABONNÉ" } else { "FAI   " })),
                    ModemEvent::Disconnected { reason } => r.log.push(format!("[{:6} ms] {} NO CARRIER {reason}", blk * 20, i)),
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

fn echange(p: LineParams, abonne: ModemConfig, fai: ModemConfig) -> Run {
    let a: Vec<u8> = "Requête de l'abonné vers le FAI. ".repeat(20).into_bytes();
    let b: Vec<u8> = "Réponse du FAI, page web de 1998. ".repeat(40).into_bytes();
    let r = run_with(p, abonne, fai, 30.0, [&a, &b]);
    let dump = r.log.join("\n");
    assert!(r.connected[0].is_some() && r.connected[1].is_some(), "pas de CONNECT\n{dump}");
    assert_eq!(String::from_utf8_lossy(&r.got[1]), String::from_utf8_lossy(&a), "abonné → FAI\n{dump}");
    assert_eq!(String::from_utf8_lossy(&r.got[0]), String::from_utf8_lossy(&b), "FAI → abonné\n{dump}");
    r
}

/// Le 56k est négocié en V.8 ; la phase 2 se fait rôles inversés (l'abonné
/// tient le rôle de l'appelé du V.34), INFO0d compris.
#[test]
fn v8_annonce_le_56k_et_phase2_inversee() {
    let r = echange(LineParams::preset("Urbaine").unwrap(), ModemConfig::default(), fai());
    let dump = r.log.join("\n");
    assert!(dump.contains("nous serons le modem analogique"), "{dump}");
    assert!(dump.contains("nous serons le modem numérique"), "{dump}");
    eprintln!("CONNECT abonné {:?} / FAI {:?}", r.connected[0], r.connected[1]);
}

/// Le 56k complet : descendant en PCM au-delà du V.34.
#[test]
fn connexion_56k() {
    for (ligne, mini, up_mini) in [("Parfaite", 56000, 31200), ("Urbaine", 36000, 9600)] {
        let r = echange(LineParams::preset(ligne).unwrap(), ModemConfig::default(), fai());
        let dump = r.log.join("\n");
        if std::env::var("V90_LOG").is_ok() {
            eprintln!("{dump}");
        }
        let (up, down) = r.connected[0].unwrap();
        assert!(down >= mini, "{ligne} : descendant {down} bit/s\n{dump}");
        assert!(up >= up_mini, "{ligne} : montant {up} bit/s\n{dump}");
        assert_eq!(r.connected[1], Some((down, up)), "{ligne}");
        eprintln!("{ligne} : CONNECT 56k — descendant {down} bit/s, montant {up} bit/s");
    }
}

/// Sans 56k d'un côté ou de l'autre : V.34 classique, rôles normaux.
#[test]
fn sans_56k_v34_classique() {
    for (abonne, f) in [(ModemConfig { v90: false, ..ModemConfig::default() }, fai()), (ModemConfig::default(), ModemConfig::default())] {
        let r = echange(LineParams::preset("Urbaine").unwrap(), abonne, f);
        assert!(!r.log.join("\n").contains("modem analogique"));
    }
}

/// Interrupteur « 56K EXPRESS » : sans les lenteurs d'époque, la poignée de
/// main est nettement plus courte ; même débit.
#[test]
fn express_plus_rapide() {
    let p = LineParams::preset("Parfaite").unwrap();
    let lent = echange(p.clone(), ModemConfig::default(), fai());
    let vite = echange(p, ModemConfig { express: true, ..ModemConfig::default() }, fai());
    let (tl, tv) = (lent.t_connect.unwrap(), vite.t_connect.unwrap());
    // la première réponse du FAI (ANSam) part 400 ms après le décroché
    eprintln!("CONNECT 56k après l'ANSam : d'époque {} ms, express {} ms", tl - 400, tv - 400);
    assert!(tl >= tv + 8000, "d'époque {tl} ms, express {tv} ms");
    assert!((19_000..=23_000).contains(&(tl - 400)), "d'époque : {} ms après l'ANSam (vrais modems : ~20 s)", tl - 400);
    // même débit descendant (le montant dépend du SNR mesuré, à ±1 cran)
    assert_eq!(lent.connected[0].unwrap().1, vite.connected[0].unwrap().1);
}

/// Appel long : le 56k doit tenir. Données en continu dans les deux sens
/// pendant 90 s ; bilan par tranches de 10 s.
fn appel_long(ligne: &str, secondes: usize, glissement_a: Option<usize>) -> (Vec<(usize, usize)>, Vec<String>) {
    let p = LineParams::preset(ligne).unwrap();
    let mut call = Modem::new(Role::Call, ModemConfig { express: true, ..ModemConfig::default() });
    let mut ans = Modem::new(Role::Answer, fai());
    let mut c2a = LineSim::new(p.clone());
    let mut a2c = LineSim::new(p.clone());
    let mut echo_c = EchoPath::new(p.clone());
    let mut echo_a = EchoPath::new(p);
    call.command(ModemCommand::Dial);
    ans.command(ModemCommand::Answer);
    const B: usize = 160;
    let (mut call_rx, mut ans_rx) = ([0.0f32; B], [0.0f32; B]);
    let (mut call_tx, mut ans_tx) = ([0.0f32; B], [0.0f32; B]);
    let mut connected = false;
    let mut t0 = 0;
    let mut tranches = vec![(0usize, 0usize); secondes / 10 + 1];
    let mut log = Vec::new();
    let mut retenue = 0.0f32;
    for blk in 0..(secondes + 20) * 50 {
        call.process(&call_rx, &mut call_tx);
        ans.process(&ans_rx, &mut ans_tx);
        c2a.process(&call_tx, &mut ans_rx);
        echo_a.process(&ans_tx, &mut ans_rx);
        a2c.process(&ans_tx, &mut call_rx);
        echo_c.process(&call_tx, &mut call_rx);
        // un échantillon perdu sur le trajet descendant (trou réseau mal
        // rattrapé) : les trames PCM sont décalées pour de bon
        if connected && glissement_a.is_some_and(|g| (blk - t0) / 50 >= g) {
            // un échantillon de retard, pour toujours
            let last = call_rx[B - 1];
            call_rx.copy_within(..B - 1, 1);
            call_rx[0] = retenue;
            retenue = last;
        }
        for (i, m) in [&mut call, &mut ans].into_iter().enumerate() {
            for ev in m.drain_events() {
                match ev {
                    ModemEvent::Connected { .. } if i == 0 && connected => {
                        log.push(format!("[{:5} ms après CONNECT] ABONNÉ reconnecté", (blk - t0) * 20));
                    }
                    ModemEvent::Connected { .. } if i == 0 && !connected => {
                        connected = true;
                        t0 = blk;
                    }
                    ModemEvent::Data(d) if connected => {
                        let k = ((blk - t0) / 500).min(tranches.len() - 1);
                        if i == 0 { tranches[k].0 += d.len() } else { tranches[k].1 += d.len() }
                    }
                    ModemEvent::Log { msg, .. } if connected => {
                        if msg.contains("éentra") || msg.contains("enégoc") || msg.contains("CONNECT") || msg.contains("56k") {
                            log.push(format!("[{:5} ms après CONNECT] {} {msg}", (blk - t0) * 20, if i == 0 { "ABONNÉ" } else { "FAI" }));
                        }
                    }
                    ModemEvent::Disconnected { reason } => log.push(format!("[{} ms] NO CARRIER {reason}", blk * 20)),
                    _ => {}
                }
            }
        }
        // flot continu : 2 Ko toutes les 0,2 s dans chaque sens
        if connected && blk % 10 == 0 && (blk - t0) < secondes * 50 {
            call.command(ModemCommand::Send(vec![b'a'; 300]));
            ans.command(ModemCommand::Send(vec![b'f'; 1200]));
        }
    }
    (tranches, log)
}

#[test]
#[ignore = "long : cargo test --release --test v90_appel -- --ignored --nocapture"]
fn le_56k_tient_90_secondes() {
    for ligne in ["Parfaite", "Urbaine"] {
        let (tranches, log) = appel_long(ligne, 90, None);
        eprintln!("{ligne} : octets reçus par tranche de 10 s (abonné, FAI) : {tranches:?}");
        for l in &log {
            eprintln!("  {l}");
        }
        assert!(tranches[1..8].iter().all(|&(a, f)| a > 30_000 && f > 8_000), "{ligne} : débit tombé {tranches:?}\n{}", log.join("\n"));
    }
}

/// Un échantillon perdu décale les trames PCM : le modem doit s'en rendre
/// compte, se réentraîner et repartir en 56k.
#[test]
#[ignore = "long : cargo test --release --test v90_appel -- --ignored --nocapture"]
fn le_56k_se_remet_d_un_glissement() {
    let (tranches, log) = appel_long("Parfaite", 60, Some(15));
    eprintln!("{tranches:?}\n{}", log.join("\n"));
    assert!(log.iter().any(|l| l.contains("Réception 56k perdue") || l.contains("56k muette")), "{}", log.join("\n"));
    assert!(log.iter().any(|l| l.contains("reconnecté")), "{}", log.join("\n"));
    assert!(tranches[4..6].iter().all(|&(a, _)| a > 30_000), "{tranches:?}");
}
