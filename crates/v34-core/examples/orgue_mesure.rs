//! Mesures de l'orgue à données : pour chaque ligne × mode × duplex, débit
//! net par sens, taux d'erreur brut, calage, décalage de fréquence, SNR.
//!
//! ```text
//! cargo run --release -p v34-core --example orgue_mesure -- [--essais N] [--secondes S]
//!     [--ligne Urbaine] [--sans-clics] [--wav DOSSIER] [--v34]
//! ```
//! `--wav` écrit des fichiers WAV (8 kHz, 16 bits, mono) pour écouter :
//! appelant seul, appelé seul, et le mélange entendu sur la ligne, pour
//! chaque mode ; `--v34` ajoute la même chose en V.34 pour comparer.
//! `--sans-clics` retire clics et coupures des lignes (pour voir ce qui
//! limite le reste). Rien n'est joué : c'est muet.

use std::io::Write;

use v34_core::orgue::banc::{appel, prbs, Sens};
use v34_core::orgue::partition::{f0, justesse, periode, DEGRES};
use v34_core::orgue::{Duplex, Mode, Voix};
use v34_core::{EchoPath, LineParams, LineSim, Modem, ModemCommand, ModemConfig, ModemEvent, Role};

/// WAV 8 kHz 16 bits mono, écrit à la main.
fn ecrire_wav(chemin: &std::path::Path, x: &[f32], gain: f32) -> std::io::Result<()> {
    let n = x.len() as u32;
    let mut f = std::io::BufWriter::new(std::fs::File::create(chemin)?);
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + 2 * n).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?; // PCM
    f.write_all(&1u16.to_le_bytes())?; // mono
    f.write_all(&8000u32.to_le_bytes())?;
    f.write_all(&16000u32.to_le_bytes())?; // octets par seconde
    f.write_all(&2u16.to_le_bytes())?;
    f.write_all(&16u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&(2 * n).to_le_bytes())?;
    for v in x {
        let s = (v * gain * 32767.0).round().clamp(-32768.0, 32767.0) as i16;
        f.write_all(&s.to_le_bytes())?;
    }
    f.flush()
}

fn arg(nom: &str) -> Option<String> {
    let a: Vec<String> = std::env::args().collect();
    a.iter().position(|x| x == nom).and_then(|i| a.get(i + 1).cloned())
}

fn drapeau(nom: &str) -> bool {
    std::env::args().any(|x| x == nom)
}

fn nom_mode(m: Mode) -> &'static str {
    match m {
        Mode::Joli => "joli",
        Mode::Rapide => "rapide",
    }
}

fn nom_duplex(d: Duplex) -> &'static str {
    match d {
        Duplex::Duo => "duo",
        Duplex::PleineBande => "pleine bande",
    }
}

fn teb(s: &Sens) -> String {
    if s.compares == 0 {
        return "—".into();
    }
    if s.erreurs == 0 {
        return format!("0 (<{:.0e})", 1.0 / s.compares as f64);
    }
    format!("{:.1e}", s.teb())
}

/// V.34 entre deux modems complets : (débits CONNECT, signaux enregistrés
/// sur `duree` échantillons après le CONNECT).
fn v34(preset: &str, duree: usize) -> (Option<(u32, u32)>, Vec<f32>, Vec<f32>, Vec<f32>) {
    let p = LineParams::preset(preset).unwrap();
    let mut call = Modem::new(Role::Call, ModemConfig::default());
    let mut ans = Modem::new(Role::Answer, ModemConfig::default());
    let (mut c2a, mut a2c) = (LineSim::with_seed(p.clone(), 11), LineSim::with_seed(p.clone(), 12));
    let (mut echo_c, mut echo_a) = (EchoPath::new(p.clone()), EchoPath::new(p));
    call.command(ModemCommand::Dial);
    ans.command(ModemCommand::Answer);
    const B: usize = 160;
    let (mut call_rx, mut ans_rx) = ([0.0f32; B], [0.0f32; B]);
    let (mut call_tx, mut ans_tx) = ([0.0f32; B], [0.0f32; B]);
    let mut connecte: Option<(u32, u32)> = None;
    let mut envoye = [false; 2];
    let (mut ec, mut ea, mut el) = (Vec::new(), Vec::new(), Vec::new());
    // les mêmes bits pseudo-aléatoires que l'orgue, en octets
    let octets: Vec<u8> = prbs(3, 8 * 200_000).chunks(8).map(|c| c.iter().fold(0u8, |a, b| (a << 1) | b)).collect();
    for _ in 0..(60 * 8000 / B) {
        call.process(&call_rx, &mut call_tx);
        ans.process(&ans_rx, &mut ans_tx);
        c2a.process(&call_tx, &mut ans_rx);
        echo_a.process(&ans_tx, &mut ans_rx);
        a2c.process(&ans_tx, &mut call_rx);
        if connecte.is_some() && ec.len() < duree {
            ec.extend_from_slice(&call_tx);
            ea.extend_from_slice(&ans_tx);
            el.extend(call_tx.iter().zip(&call_rx).map(|(x, y)| x + y));
        }
        echo_c.process(&call_tx, &mut call_rx);
        for (i, m) in [&mut call, &mut ans].into_iter().enumerate() {
            for ev in m.drain_events() {
                if let ModemEvent::Connected { tx_rate, rx_rate } = ev {
                    if i == 0 {
                        connecte = Some((tx_rate, rx_rate));
                    }
                    if !envoye[i] {
                        envoye[i] = true;
                        m.command(ModemCommand::Send(octets.clone()));
                    }
                }
            }
        }
        if ec.len() >= duree {
            break;
        }
    }
    (connecte, ec, ea, el)
}

fn main() {
    let essais: u64 = arg("--essais").map(|s| s.parse().unwrap()).unwrap_or(3);
    let secondes: f32 = arg("--secondes").map(|s| s.parse().unwrap()).unwrap_or(20.0);
    let filtre = arg("--ligne");
    let wav = arg("--wav").map(std::path::PathBuf::from);

    let (iv, ab, oc) = justesse();
    println!("Accord : intervalles à {iv:.1} cents du tempérament égal au pire, notes à {ab:.1} cents du la 440 au pire, octaves appelant/appelé à {oc:.1} cent");
    for v in [Voix::Appelant, Voix::Appele] {
        let notes: Vec<String> = DEGRES.iter().map(|&d| format!("{} {:.1} Hz (W={})", d.nom(), f0(v, d), periode(v, d))).collect();
        println!("  {v:?} : {}", notes.join(", "));
    }
    println!();
    println!("{essais} essai(s) × {secondes} s de données par case ; débit net = moyenne sur la boucle mélodique (chargement mesuré, marge 3 dB)");
    println!();
    println!("| Ligne | Mode | Duplex | Sens | Débit net (bit/s) | TEB brut | Calage | Δf estimé (Hz) | SNR données (dB) | bits/harm. | écho résiduel sous le signal (dB) |");
    println!("|---|---|---|---|---|---|---|---|---|---|---|");
    for (nom, p) in LineParams::presets() {
        if let Some(f) = &filtre {
            if LineParams::preset(f) != Some(p.clone()) {
                continue;
            }
        }
        let mut p = p;
        let nom = if drapeau("--sans-clics") {
            p.impulses_per_s = 0.0;
            p.dropouts_per_s = 0.0;
            format!("{nom} sans clics")
        } else {
            nom.to_string()
        };
        for mode in [Mode::Joli, Mode::Rapide] {
            for duplex in [Duplex::Duo, Duplex::PleineBande] {
                let mut tot = [Sens::default(), Sens::default()];
                let mut cales = [0usize; 2];
                let mut debits = [Vec::new(), Vec::new()];
                let mut erle = [0.0f32; 2];
                for e in 0..essais {
                    let r = appel(&p, mode, duplex, secondes, 100 + e, 0);
                    for s in 0..2 {
                        let x = &r.sens[s];
                        if x.cale {
                            cales[s] += 1;
                        }
                        tot[s].erreurs += x.erreurs;
                        tot[s].compares += x.compares;
                        tot[s].effacements += x.effacements;
                        tot[s].symboles += x.symboles;
                        debits[s].push(x.debit_net);
                        tot[s].bits_moyens += x.bits_moyens / essais as f32;
                        tot[s].snr_donnees_db += x.snr_donnees_db / essais as f32;
                        tot[s].decalage_hz += x.decalage_hz / essais as f32;
                        erle[s] += r.echo_sous_signal_db[1 - s] / essais as f32;
                    }
                }
                for s in 0..2 {
                    let sens = if s == 0 { "appelant → appelé" } else { "appelé → appelant" };
                    let d = &debits[s];
                    let dmin = d.iter().cloned().fold(f32::INFINITY, f32::min);
                    let dmax = d.iter().cloned().fold(0.0f32, f32::max);
                    let debit = if dmax - dmin < 1.0 { format!("{dmin:.0}") } else { format!("{dmin:.0}–{dmax:.0}") };
                    println!(
                        "| {nom} | {} | {} | {sens} | {debit} | {} | {}/{essais} | {:+.2} | {:.1} | {:.2} | {:.1} |",
                        nom_mode(mode),
                        nom_duplex(duplex),
                        teb(&tot[s]),
                        cales[s],
                        tot[s].decalage_hz,
                        tot[s].snr_donnees_db,
                        tot[s].bits_moyens,
                        erle[s]
                    );
                }
            }
        }
    }

    if let Some(dir) = wav {
        std::fs::create_dir_all(&dir).unwrap();
        let p = LineParams::preset("Urbaine").unwrap();
        for mode in [Mode::Joli, Mode::Rapide] {
            for duplex in [Duplex::Duo, Duplex::PleineBande] {
                let r = appel(&p, mode, duplex, 10.5, 1, 0);
                let fin = r.debut_donnees + 10 * 8000;
                let r = appel(&p, mode, duplex, 10.5, 1, fin);
                let e = r.enregistrement.unwrap();
                let base = format!("orgue_{}_{}", nom_mode(mode), if duplex == Duplex::Duo { "duo" } else { "pleine_bande" });
                ecrire_wav(&dir.join(format!("{base}_appelant.wav")), &e.appelant, 1.0).unwrap();
                ecrire_wav(&dir.join(format!("{base}_appele.wav")), &e.appele, 1.0).unwrap();
                ecrire_wav(&dir.join(format!("{base}_ligne.wav")), &e.ligne, 0.7).unwrap();
                println!("WAV : {}/{base}_{{appelant,appele,ligne}}.wav ({:.1} s, données à {:.1} s)", dir.display(), e.appelant.len() as f32 / 8000.0, r.debut_donnees as f32 / 8000.0);
            }
        }
        if drapeau("--v34") {
            let (c, ec, ea, el) = v34("Urbaine", 10 * 8000);
            ecrire_wav(&dir.join("v34_appelant.wav"), &ec, 1.0).unwrap();
            ecrire_wav(&dir.join("v34_appele.wav"), &ea, 1.0).unwrap();
            ecrire_wav(&dir.join("v34_ligne.wav"), &el, 0.7).unwrap();
            println!("WAV : {}/v34_{{appelant,appele,ligne}}.wav (CONNECT {c:?}, {:.1} s)", dir.display(), ec.len() as f32 / 8000.0);
        }
    }
    if drapeau("--v34") {
        println!();
        println!("Référence V.34 (CONNECT appelant : émission/réception bit/s) :");
        for (nom, _) in LineParams::presets() {
            let (c, ..) = v34(nom, 8000);
            println!("  {nom} : {c:?}");
        }
    }
}
