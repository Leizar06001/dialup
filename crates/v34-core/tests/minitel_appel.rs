//! Un appel Minitel entre deux modems complets, à travers la ligne simulée :
//! l'appelant compose comme pour une liaison de données (il attend un
//! ANSam), l'appelé décroche en serveur Minitel (ANS pur, porteuse 1300 Hz).
//! L'appelant doit reconnaître le sifflement, passer en V.23, et les
//! caractères doivent passer dans les deux sens : un écran Vidéotex à
//! 1200 bit/s, des touches à 75 bit/s. Puis le serveur raccroche.
//!
//! Le V.23 n'a aucune correction d'erreurs (un vrai Minitel non plus) : un
//! clic sur la ligne abîme un caractère ou deux, qui s'affichent en pavé
//! (SUB). Les lignes sans clics doivent être parfaites.

use v34_core::{EchoPath, LineParams, LineSim, Modem, ModemCommand, ModemConfig, ModemEvent, Phase, Role};

const B: usize = 160;

#[derive(Default, Debug)]
struct Bilan {
    /// Secondes jusqu'au CONNECT (appelant, appelé)
    connect: [Option<f32>; 2],
    debits: Option<(u32, u32)>,
    ecran: Vec<u8>,
    clavier: Vec<u8>,
    phases: Vec<Phase>,
    raccroche: [bool; 2],
    fin_s: Option<f32>,
}

/// Une page Vidéotex : effacement, positionnement, attributs, accents (G2),
/// mosaïques, et tous les caractères de 0x20 à 0x7F.
fn page() -> Vec<u8> {
    let mut p = b"\x0c\x1f\x41\x41\x1b\x4d\x1b\x46 3615 ORACLE\x1f\x43\x41L'avenir en direct depuis 1998, \x19\x42etonnant !\x0e\x21\x7f\x3f\x0f".to_vec();
    p.extend(0x20u8..0x80);
    p
}

/// ENVOI, une question, SUITE, un « é »
const TOUCHES: &[u8] = b"\x13\x41QUI VA GAGNER ?\x13\x48\x19\x42e";

fn appel(p: &LineParams, graine: u64) -> Bilan {
    let mut call = Modem::new(Role::Call, ModemConfig::default());
    let mut ans = Modem::new(Role::Answer, ModemConfig { minitel: true, ..ModemConfig::default() });
    let (mut c2a, mut a2c) = (LineSim::with_seed(p.clone(), graine), LineSim::with_seed(p.clone(), graine * 31 + 11));
    let (mut echo_c, mut echo_a) = (EchoPath::new(p.clone()), EchoPath::new(p.clone()));
    call.command(ModemCommand::Dial);
    ans.command(ModemCommand::Answer);
    let (mut call_rx, mut ans_rx) = ([0.0f32; B], [0.0f32; B]);
    let (mut call_tx, mut ans_tx) = ([0.0f32; B], [0.0f32; B]);
    let mut b = Bilan::default();
    let (mut envoye_ecran, mut envoye_clavier, mut raccroche_demande) = (false, false, false);
    for bloc in 0..(60 * 8000 / B) {
        let t = (bloc * B) as f32 / 8000.0;
        call.process(&call_rx, &mut call_tx);
        ans.process(&ans_rx, &mut ans_tx);
        c2a.process(&call_tx, &mut ans_rx);
        echo_a.process(&ans_tx, &mut ans_rx);
        a2c.process(&ans_tx, &mut call_rx);
        echo_c.process(&call_tx, &mut call_rx);
        for ev in call.drain_events() {
            match ev {
                ModemEvent::Connected { tx_rate, rx_rate } => {
                    b.connect[0].get_or_insert(t);
                    b.debits = Some((tx_rate, rx_rate));
                }
                ModemEvent::Data(d) => b.ecran.extend(d),
                ModemEvent::PhaseChanged(p) => b.phases.push(p),
                ModemEvent::Disconnected { .. } => b.raccroche[0] = true,
                _ => {}
            }
        }
        for ev in ans.drain_events() {
            match ev {
                ModemEvent::Connected { tx_rate, rx_rate } => {
                    assert_eq!((tx_rate, rx_rate), (1200, 75));
                    b.connect[1].get_or_insert(t);
                }
                ModemEvent::Data(d) => b.clavier.extend(d),
                ModemEvent::Disconnected { .. } => b.raccroche[1] = true,
                _ => {}
            }
        }
        // le serveur envoie sa page dès CONNECT ; le Minitel tape ensuite
        if b.connect[1].is_some() && !envoye_ecran {
            envoye_ecran = true;
            ans.command(ModemCommand::Send(page()));
        }
        if b.connect[0].is_some() && !envoye_clavier {
            envoye_clavier = true;
            call.command(ModemCommand::Send(TOUCHES.to_vec()));
        }
        // tout est arrivé (la page en 1,4 s, les touches en 2,8 s) : le serveur raccroche
        if b.connect[0].is_some_and(|c| t > c + 4.5) && !raccroche_demande {
            raccroche_demande = true;
            ans.command(ModemCommand::Hangup);
        }
        if b.raccroche == [true, true] {
            b.fin_s = Some(t);
            break;
        }
    }
    assert_eq!(call.status().phase, Phase::Idle);
    b
}

/// Distance d'édition : caractères abîmés, perdus ou en trop.
fn ecart(a: &[u8], b: &[u8]) -> usize {
    let mut prec: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut cour = vec![i + 1];
        for (j, y) in b.iter().enumerate() {
            cour.push((prec[j] + usize::from(x != y)).min(prec[j + 1] + 1).min(cour[j] + 1));
        }
        prec = cour;
    }
    prec[b.len()]
}

/// L'appel aboutit en Minitel, sans V.34, vite, et se termine des deux côtés.
fn verifier(nom: &str, p: &LineParams) -> Bilan {
    let b = appel(p, 7);
    let (cc, ca) = (b.connect[0].unwrap_or_else(|| panic!("{nom} : pas de CONNECT chez l'appelant, phases {:?}", b.phases)), b.connect[1].expect("CONNECT chez l'appelé"));
    eprintln!("{nom} : CONNECT appelant {cc:.2} s, appelé {ca:.2} s ; fin de l'appel {:?} s", b.fin_s);
    assert!(b.phases.contains(&Phase::Minitel), "{nom} : phases {:?}", b.phases);
    assert!(!b.phases.contains(&Phase::Phase2), "{nom} : pas de V.34 : {:?}", b.phases);
    assert_eq!(b.debits, Some((75, 1200)), "{nom}");
    // silence 0,4 s + ANS 2,6 s + 75 ms + 150 ms d'écoute + 300 ms de porteuse de retour
    assert!(cc < 4.0, "{nom} : CONNECT en {cc:.2} s");
    assert_eq!(b.raccroche, [true, true], "{nom} : fin d'appel signalée des deux côtés");
    b
}

fn ligne(nom: &str) -> LineParams {
    LineParams::preset(nom).unwrap()
}

#[test]
fn minitel_lignes_sans_clics() {
    let mut campagne_au_calme = ligne("Campagne");
    campagne_au_calme.impulses_per_s = 0.0;
    for (nom, p) in [("Parfaite", ligne("Parfaite")), ("Urbaine", ligne("Urbaine")), ("Longue distance", ligne("Longue distance")), ("Campagne sans clics", campagne_au_calme)] {
        let b = verifier(nom, &p);
        assert_eq!(b.ecran, page(), "{nom} : écran");
        assert_eq!(b.clavier, TOUCHES, "{nom} : clavier");
    }
}

/// Campagne : un clic toutes les cinq secondes. Il peut abîmer un caractère
/// ou deux, jamais faire perdre le fil.
#[test]
fn minitel_campagne() {
    let b = verifier("Campagne", &ligne("Campagne"));
    let e = ecart(&b.ecran, &page()) + ecart(&b.clavier, TOUCHES);
    assert!(e <= 6, "Campagne : {e} caractères abîmés : {:?}", String::from_utf8_lossy(&b.ecran));
}

/// Sous l'orage (trois clics par seconde, des coupures), la connexion tient
/// et l'essentiel passe.
#[test]
fn minitel_sous_l_orage() {
    let b = verifier("Orage", &ligne("Orage"));
    let attendu = page();
    let e = ecart(&b.ecran, &attendu);
    eprintln!("Orage : {e} écarts sur {} caractères ; clavier {:?}", attendu.len(), String::from_utf8_lossy(&b.clavier));
    assert!(e * 5 <= attendu.len(), "Orage : {e} écarts : {:?}", String::from_utf8_lossy(&b.ecran));
}

/// Statistiques sur vingt appels par ligne :
/// `cargo test --release --test minitel_appel -- --ignored --nocapture`
#[test]
#[ignore]
fn statistiques() {
    for nom in ["Urbaine", "Campagne", "Longue distance", "Orage"] {
        let mut total = 0;
        let mut connect = Vec::new();
        for graine in 1..=20 {
            let b = appel(&ligne(nom), graine);
            total += ecart(&b.ecran, &page()) + ecart(&b.clavier, TOUCHES);
            connect.push(b.connect[0].unwrap_or(99.0));
        }
        eprintln!("{nom} : {total} caractères abîmés en 20 appels ; CONNECT {connect:?}");
    }
}

/// Un appel V.34 ordinaire ne prend pas le chemin du Minitel.
#[test]
fn un_appel_v34_reste_en_v34() {
    let p = ligne("Urbaine");
    let mut call = Modem::new(Role::Call, ModemConfig::default());
    let mut ans = Modem::new(Role::Answer, ModemConfig::default());
    let (mut c2a, mut a2c) = (LineSim::new(p.clone()), LineSim::new(p));
    call.command(ModemCommand::Dial);
    ans.command(ModemCommand::Answer);
    let (mut call_rx, mut ans_rx) = ([0.0f32; B], [0.0f32; B]);
    let (mut call_tx, mut ans_tx) = ([0.0f32; B], [0.0f32; B]);
    let mut phases = Vec::new();
    for _ in 0..(6 * 8000 / B) {
        call.process(&call_rx, &mut call_tx);
        ans.process(&ans_rx, &mut ans_tx);
        c2a.process(&call_tx, &mut ans_rx);
        a2c.process(&ans_tx, &mut call_rx);
        for ev in call.drain_events() {
            if let ModemEvent::PhaseChanged(p) = ev {
                phases.push(p);
            }
        }
    }
    assert!(phases.contains(&Phase::Phase2) && !phases.contains(&Phase::Minitel), "{phases:?}");
}
