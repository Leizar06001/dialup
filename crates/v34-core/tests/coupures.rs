//! Trous de 20 ms dans le signal reçu (paquet perdu sur Internet) : le
//! modem doit encaisser sans renégocier ni perdre de données (LAPM
//! retransmet ce qui a été abîmé).

use v34_core::{LineParams, LineSim, Modem, ModemCommand, ModemConfig, ModemEvent, Role};

struct Bilan {
    connecte: bool,
    problemes: Vec<String>,
    recu: [usize; 2],
}

/// `trou_tous_les` : un trou de 20 ms (160 échantillons à zéro) dans le sens
/// appelé → appelant, tous les `trou_tous_les` blocs de 20 ms une fois connectés.
fn essai(trou_tous_les: usize, secondes: usize) -> Bilan {
    essai_ligne(LineParams::perfect(), trou_tous_les, secondes)
}

/// `trou_tous_les` = 0 : pas de trou.
fn essai_ligne(p: LineParams, trou_tous_les: usize, secondes: usize) -> Bilan {
    let mut call = Modem::new(Role::Call, ModemConfig::default());
    let mut ans = Modem::new(Role::Answer, ModemConfig::default());
    let (mut c2a, mut a2c) = (LineSim::new(p.clone()), LineSim::new(p));
    call.command(ModemCommand::Dial);
    ans.command(ModemCommand::Answer);
    const B: usize = 160;
    let (mut call_rx, mut ans_rx, mut call_tx, mut ans_tx) = ([0.0f32; B], [0.0f32; B], [0.0f32; B], [0.0f32; B]);
    let mut b = Bilan { connecte: false, problemes: Vec::new(), recu: [0, 0] };
    let mut depuis: Option<usize> = None;
    for blk in 0..secondes * 50 {
        call.process(&call_rx, &mut call_tx);
        ans.process(&ans_rx, &mut ans_tx);
        c2a.process(&call_tx, &mut ans_rx);
        a2c.process(&ans_tx, &mut call_rx);
        if let Some(d) = depuis {
            if trou_tous_les > 0 && (blk - d) % trou_tous_les == trou_tous_les - 1 {
                call_rx = [0.0; B];
            }
            if (blk - d) % 25 == 0 {
                call.command(ModemCommand::Send(vec![b'a'; 200]));
                ans.command(ModemCommand::Send(vec![b'b'; 200]));
            }
        }
        for (i, m) in [&mut call, &mut ans].into_iter().enumerate() {
            for ev in m.drain_events() {
                match ev {
                    ModemEvent::Connected { .. } => {
                        b.connecte = true;
                        depuis.get_or_insert(blk);
                    }
                    ModemEvent::Data(d) => b.recu[i] += d.len(),
                    ModemEvent::Log { msg, .. } if msg.contains("renégociation") || msg.contains("éentraînement") => {
                        b.problemes.push(format!("[{} ms] {} {msg}", blk * 20, if i == 0 { "APPELANT" } else { "APPELÉ" }))
                    }
                    ModemEvent::Disconnected { reason } => b.problemes.push(format!("NO CARRIER {reason}")),
                    _ => {}
                }
            }
        }
    }
    b
}

#[test]
fn trou_toutes_les_demi_secondes() {
    // Pire que n'importe quel Wi-Fi : 4 % du temps en silence.
    let b = essai(25, 40);
    eprintln!("reçu {:?}, problèmes {:?}", b.recu, b.problemes);
    assert!(b.problemes.is_empty(), "{:?}", b.problemes);
    assert!(b.recu[0] > 3000 && b.recu[1] > 3000, "{:?}", b.recu);
}

#[test]
fn trous_de_20_ms_sans_renegociation() {
    let b = essai(75, 40);
    eprintln!("reçu {:?}, problèmes {:?}", b.recu, b.problemes);
    assert!(b.connecte);
    assert!(b.problemes.is_empty(), "{:?}", b.problemes);
    assert!(b.recu[0] > 5000 && b.recu[1] > 5000, "les données passent : {:?}", b.recu);
}

/// Stabilité sur chaque ligne simulée, avec et sans trous réseau (un trou
/// toutes les 3 s) : affiche les renégociations (diagnostic).
#[test]
#[ignore = "diagnostic"]
fn stabilite_par_ligne() {
    use v34_core::EchoPath;
    let _ = EchoPath::new;
    for nom in ["Parfaite", "Urbaine", "Campagne", "Longue distance", "Orage"] {
        for trous in [0, 150] {
            let b = essai_ligne(LineParams::preset(nom).unwrap(), trous, 70);
            eprintln!("{nom:16} trous={trous:3} : {} problème(s), reçu {:?}", b.problemes.len(), b.recu);
            for pb in b.problemes.iter().take(6) {
                eprintln!("      {pb}");
            }
        }
    }
}
