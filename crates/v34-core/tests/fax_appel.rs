//! Une télécopie entre deux modems complets : l'appelant a préparé une page
//! et compose ; l'appelé décroche comme pour une liaison de données (V.8),
//! entend le CNG et passe en réception de fax.

use v34_core::fax::session::FaxEvenement;
use v34_core::fax::t4::LARGEUR;
use v34_core::{EchoPath, LineParams, LineSim, Modem, ModemCommand, ModemConfig, ModemEvent, Phase, Role};

struct Fin {
    appelant: Option<(bool, String)>,
    appele: Option<(bool, String)>,
    lignes: Vec<Vec<bool>>,
    correspondant: Option<String>,
    phases: Vec<Phase>,
    /// Chacun a signalé son raccroché (le lien réseau libère alors l'appel)
    raccroche: [bool; 2],
}

fn telecopie(ligne: &str, page: &[Vec<bool>]) -> Fin {
    let p = LineParams::preset(ligne).unwrap();
    let mut call = Modem::new(Role::Call, ModemConfig { ident: "0100000001".into(), ..ModemConfig::default() });
    let mut ans = Modem::new(Role::Answer, ModemConfig { ident: "0100000002".into(), ..ModemConfig::default() });
    let (mut c2a, mut a2c) = (LineSim::new(p.clone()), LineSim::new(p.clone()));
    let (mut echo_c, mut echo_a) = (EchoPath::new(p.clone()), EchoPath::new(p));
    call.command(ModemCommand::PreparerFax { lignes: page.to_vec(), fine: true });
    call.command(ModemCommand::Dial);
    ans.command(ModemCommand::Answer);
    const B: usize = 160;
    let (mut call_rx, mut ans_rx) = ([0.0f32; B], [0.0f32; B]);
    let (mut call_tx, mut ans_tx) = ([0.0f32; B], [0.0f32; B]);
    let mut f = Fin { appelant: None, appele: None, lignes: Vec::new(), correspondant: None, phases: Vec::new(), raccroche: [false; 2] };
    for _ in 0..(90 * 8000 / B) {
        call.process(&call_rx, &mut call_tx);
        ans.process(&ans_rx, &mut ans_tx);
        c2a.process(&call_tx, &mut ans_rx);
        echo_a.process(&ans_tx, &mut ans_rx);
        a2c.process(&ans_tx, &mut call_rx);
        echo_c.process(&call_tx, &mut call_rx);
        for ev in call.drain_events() {
            match ev {
                ModemEvent::Fax(FaxEvenement::Fin { ok, raison }) => f.appelant = Some((ok, raison)),
                ModemEvent::Disconnected { .. } => f.raccroche[0] = true,
                _ => {}
            }
        }
        for ev in ans.drain_events() {
            match ev {
                ModemEvent::Fax(FaxEvenement::Fin { ok, raison }) => f.appele = Some((ok, raison)),
                ModemEvent::Fax(FaxEvenement::Ligne(l)) => f.lignes.push(l),
                ModemEvent::Fax(FaxEvenement::Correspondant(c)) => f.correspondant = Some(c),
                ModemEvent::PhaseChanged(p) => f.phases.push(p),
                ModemEvent::Disconnected { .. } => f.raccroche[1] = true,
                _ => {}
            }
        }
        if f.appelant.is_some() && f.appele.is_some() && f.raccroche == [true, true] {
            break;
        }
    }
    f
}

fn page(n: usize) -> Vec<Vec<bool>> {
    (0..n).map(|y| (0..LARGEUR).map(|x| ((x / 40) + (y / 40)) % 2 == 0 && x < 1600).collect()).collect()
}

#[test]
fn telecopie_entre_deux_modems() {
    let pg = page(160);
    let f = telecopie("Urbaine", &pg);
    assert_eq!(f.appelant, Some((true, "page transmise".into())), "phases de l'appelé : {:?}", f.phases);
    assert_eq!(f.appele, Some((true, "page reçue".into())));
    assert_eq!(f.correspondant.as_deref(), Some("0100000001"));
    assert!(f.phases.contains(&Phase::Fax), "l'appelé est passé en fax : {:?}", f.phases);
    assert_eq!(f.lignes, pg);
    assert_eq!(f.raccroche, [true, true], "fin d'appel signalée des deux côtés");
}
