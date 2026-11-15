//! Le morse entre deux modems complets : l'appelant le propose en V.8
//! (service non normalisé TINYVERSE), l'appelé l'accepte, puis chacun
//! envoie son message en même temps que l'autre, à travers le simulateur de
//! ligne (écho compris).

use v34_core::morse::MorseEvenement;
use v34_core::{EchoPath, LineParams, LineSim, Modem, ModemCommand, ModemConfig, ModemEvent, Phase, Role};

const B: usize = 160;

#[derive(Default, Debug)]
struct Appel {
    /// Instant (s) du CONNECT de chacun
    connect: [Option<f32>; 2],
    /// Débit annoncé par CONNECT
    debit: [Option<u32>; 2],
    phases: [Vec<Phase>; 2],
    /// Texte reconnu par chacun (événements Morse::Recu)
    lu: [String; 2],
    /// Même chose, par le flux d'octets (ModemEvent::Data)
    donnees: [Vec<u8>; 2],
    /// Signes partis (événements Morse::Emis)
    emis: [String; 2],
    journal: Vec<String>,
    raccroche: [bool; 2],
    /// Fin des deux messages (s)
    fin: Option<f32>,
}

/// Un appel : `cfg` de chacun, messages envoyés dès CONNECT ; s'arrête quand
/// les deux messages sont lus (ou au bout de `max_s`). `raccrocher` : l'appelant
/// raccroche ensuite.
fn appel(ligne: &str, graine: u64, cfg: [ModemConfig; 2], messages: [&str; 2], max_s: f32, raccrocher: bool) -> Appel {
    let p = LineParams::preset(ligne).unwrap();
    let mut m = [Modem::new(Role::Call, cfg[0].clone()), Modem::new(Role::Answer, cfg[1].clone())];
    let (mut c2a, mut a2c) = (LineSim::with_seed(p.clone(), graine), LineSim::with_seed(p.clone(), graine + 1));
    let (mut echo_c, mut echo_a) = (EchoPath::new(p.clone()), EchoPath::new(p));
    m[0].command(ModemCommand::Dial);
    m[1].command(ModemCommand::Answer);
    let (mut rx, mut tx) = ([[0.0f32; B]; 2], [[0.0f32; B]; 2]);
    let mut a = Appel::default();
    let mut envoyes = [false; 2];
    let mut raccroche_a = None;
    for blk in 0..((max_s * 8000.0) as usize / B) {
        let t = (blk * B) as f32 / 8000.0;
        for i in 0..2 {
            m[i].process(&rx[i], &mut tx[i]);
        }
        c2a.process(&tx[0], &mut rx[1]);
        echo_a.process(&tx[1], &mut rx[1]);
        a2c.process(&tx[1], &mut rx[0]);
        echo_c.process(&tx[0], &mut rx[0]);
        for i in 0..2 {
            for ev in m[i].drain_events() {
                match ev {
                    ModemEvent::Connected { tx_rate, .. } => {
                        a.connect[i] = Some(t);
                        a.debit[i] = Some(tx_rate);
                    }
                    ModemEvent::PhaseChanged(p) => a.phases[i].push(p),
                    ModemEvent::Morse(MorseEvenement::Recu { texte, .. }) => a.lu[i].push_str(&texte),
                    ModemEvent::Morse(MorseEvenement::Emis { texte, .. }) => a.emis[i].push_str(&texte),
                    ModemEvent::Data(d) => a.donnees[i].extend(d),
                    ModemEvent::Log { msg, .. } => a.journal.push(format!("{} {msg}", ["APPELANT", "APPELÉ"][i])),
                    ModemEvent::Disconnected { .. } => a.raccroche[i] = true,
                    _ => {}
                }
            }
            if a.connect[i].is_some() && !envoyes[i] && m[i].status().phase == Phase::Morse {
                envoyes[i] = true;
                m[i].command(ModemCommand::Send(messages[i].as_bytes().to_vec()));
            }
        }
        if a.fin.is_none() && a.lu[1].ends_with("<AR>") && a.lu[0].ends_with("<AR>") {
            a.fin = Some(t);
            if !raccrocher {
                break;
            }
            raccroche_a = Some(blk);
            m[0].command(ModemCommand::Hangup);
        }
        if raccroche_a.is_some() && a.raccroche == [true, true] {
            break;
        }
        // pas de morse : on s'arrête une fois connectés
        if !a.phases[0].contains(&Phase::Morse) && a.connect[0].zip(a.connect[1]).is_some_and(|(c0, c1)| t > c0.max(c1) + 1.0) {
            break;
        }
    }
    a
}

fn morse() -> ModemConfig {
    ModemConfig { morse: true, ..ModemConfig::default() }
}

/// Les signes d'un texte : une procédure (« <KA> ») compte pour un.
fn signes(t: &str) -> Vec<String> {
    let mut v = Vec::new();
    let mut proc: Option<String> = None;
    for c in t.chars() {
        match proc.as_mut() {
            Some(p) => {
                p.push(c);
                if c == '>' {
                    v.push(proc.take().unwrap());
                }
            }
            None if c == '<' => proc = Some("<".into()),
            None => v.push(c.to_string()),
        }
    }
    v
}

/// Distance d'édition, en signes : les erreurs de lecture.
fn erreurs(a: &str, b: &str) -> usize {
    let (a, b) = (signes(a), signes(b));
    let mut d: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut prev = d[0];
        d[0] = i;
        for j in 1..=b.len() {
            let tmp = d[j];
            d[j] = (d[j] + 1).min(d[j - 1] + 1).min(prev + usize::from(a[i - 1] != b[j - 1]));
            prev = tmp;
        }
    }
    d[b.len()]
}

const ALLER: &str = "<KA>CQ CQ DE TINYVERSE, ICI LE MODEM QUI APPELLE. ÇA VA ? 73<AR>";
const RETOUR: &str = "<KA>BIEN REÇU ! L'ÉTÉ 98 SERA CHAUD : 3-0<AR>";

#[test]
fn morse_negocie_et_duplex_sur_ligne_urbaine() {
    let a = appel("Urbaine", 7, [morse(), morse()], [ALLER, RETOUR], 120.0, true);
    assert!(a.journal.iter().any(|l| l.contains("CONNECT 20 MPM MORSE")), "{:#?}", a.journal);
    assert!(a.phases[0].contains(&Phase::Morse) && a.phases[1].contains(&Phase::Morse), "{:?}", a.phases);
    assert!(!a.phases[0].contains(&Phase::Phase2), "pas de phase 2 en morse : {:?}", a.phases[0]);
    assert_eq!(a.debit, [Some(16), Some(16)], "CONNECT à 16 bit/s (20 mots/minute)");
    let connect = a.connect[0].unwrap();
    assert!(connect < 8.0, "négociation V.8 trop longue : {connect} s");
    // (puis SK : l'appelant raccroche)
    assert_eq!(a.lu[1], format!("{ALLER}<SK>"), "l'appelé lit l'appelant");
    assert_eq!(a.lu[0], RETOUR, "l'appelant lit l'appelé, en même temps");
    assert_eq!(String::from_utf8_lossy(&a.donnees[1]), ALLER, "le texte lu est aussi le flux d'octets");
    assert_eq!(a.emis[0], format!("{ALLER}<SK>"), "ce qui est parti, signe par signe");
    // le raccroché : SK part, l'autre le lit et raccroche aussi
    assert_eq!(a.raccroche, [true, true]);
    assert!(a.journal.iter().any(|l| l.starts_with("APPELÉ") && l.contains("SK reçu")), "{:#?}", a.journal);
    let duree = a.fin.unwrap() - connect;
    let cps = ALLER.chars().count() as f32 / duree;
    eprintln!("Urbaine : CONNECT à {connect:.1} s, {} caractères en {duree:.1} s ({cps:.2} car/s)", ALLER.chars().count());
}

#[test]
fn morse_sur_toutes_les_lignes() {
    // (ligne, signes faux tolérés sur chaque message : l'orage a ses
    // coupures de 20 à 100 ms, qui coupent un trait en deux ou mangent un point)
    for (ligne, tolere) in [("Parfaite", 0), ("Urbaine", 0), ("Campagne", 0), ("Longue distance", 0), ("Orage", 4)] {
        for graine in [11u64, 23] {
            let a = appel(ligne, graine, [morse(), morse()], [ALLER, RETOUR], 150.0, false);
            assert!(a.phases[1].contains(&Phase::Morse), "{ligne} : pas de morse ({:?})\n{:#?}", a.phases, a.journal);
            let e = [erreurs(ALLER, &a.lu[1]), erreurs(RETOUR, &a.lu[0])];
            eprintln!(
                "{ligne:16} graine {graine} : CONNECT à {:.1} s, erreurs {e:?}, fin à {:?} s\n   appelé lit   : {}\n   appelant lit : {}",
                a.connect[0].unwrap_or(-1.0),
                a.fin,
                a.lu[1],
                a.lu[0]
            );
            assert!(e[0] <= tolere && e[1] <= tolere, "{ligne} graine {graine} : trop d'erreurs {e:?}\n{}\n{}", a.lu[1], a.lu[0]);
        }
    }
}

#[test]
fn sans_morse_en_face_l_appel_continue_en_v34() {
    // l'appelant propose le morse, l'appelé ne veut pas (DIP 4 sur OFF)
    let a = appel("Urbaine", 3, [morse(), ModemConfig::default()], ["", ""], 40.0, false);
    assert!(!a.phases[0].contains(&Phase::Morse) && !a.phases[1].contains(&Phase::Morse), "{:?}", a.phases);
    assert!(a.debit[0].is_some_and(|d| d >= 9600) && a.debit[1].is_some(), "V.34 connecté : {:?}\n{:#?}", a.debit, a.journal);
    assert!(a.journal.iter().any(|l| l.contains("refusé (DIP 4 sur OFF)")), "{:#?}", a.journal);
    assert!(a.journal.iter().any(|l| l.contains("pas de morse en face")), "{:#?}", a.journal);
    // l'appelé veut bien, mais l'appelant ne l'a pas demandé
    let a = appel("Urbaine", 4, [ModemConfig::default(), morse()], ["", ""], 40.0, false);
    assert!(!a.phases[1].contains(&Phase::Morse), "{:?}", a.phases);
    assert!(a.debit[0].is_some_and(|d| d >= 9600), "V.34 connecté : {:?}\n{:#?}", a.debit, a.journal);
}
