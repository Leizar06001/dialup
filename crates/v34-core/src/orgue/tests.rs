//! Essais de bout en bout : deux organistes reliés par la ligne simulée.

use super::banc::appel;
use super::*;
use crate::line::LineParams;

#[test]
fn duo_joli_sur_ligne_parfaite_sans_erreur() {
    let r = appel(&LineParams::perfect(), Mode::Joli, Duplex::Duo, 3.0, 1, 0);
    for s in &r.sens {
        assert!(s.cale);
        assert!(s.compares > 5000, "{s:?}");
        assert_eq!(s.erreurs, 0, "{s:?}");
    }
}

#[test]
fn pleine_bande_rapide_sur_ligne_urbaine() {
    // l'écho (-30 dB) est dans la bande d'en face : seul l'annuleur sépare
    let r = appel(&LineParams::preset("Urbaine").unwrap(), Mode::Rapide, Duplex::PleineBande, 3.0, 7, 0);
    for s in &r.sens {
        assert!(s.cale);
        assert!(s.compares > 20000, "{s:?}");
        assert!(s.teb() < 1e-3, "{s:?}");
    }
    for e in r.echo_sous_signal_db {
        assert!(e > 35.0, "écho résiduel à {e:.1} dB sous le signal");
    }
}

#[test]
fn decalage_de_frequence_retrouve_et_corrige() {
    // 7 Hz : le pire des vieux multiplex ; toutes les harmoniques glissent
    let p = LineParams { freq_offset_hz: 7.0, ..LineParams::preset("Urbaine").unwrap() };
    let r = appel(&p, Mode::Joli, Duplex::Duo, 3.0, 3, 0);
    for s in &r.sens {
        assert!(s.cale);
        assert!((s.decalage_hz - 7.0).abs() < 0.05, "estimé {:.3} Hz", s.decalage_hz);
        assert!(s.teb() < 1e-3, "{s:?}");
    }
}

/// Diagnostic : SNR par harmonique de la tonique (tenue du motif, tremblant,
/// valeur retenue pour le chargement, données) et bits portés.
/// `ORGUE_LIGNE=Campagne ORGUE_RAPIDE=1 ORGUE_DUO=1 ORGUE_SANS_CLICS=1
/// cargo test --release -p v34-core --lib diagnostic_par_harmonique -- --ignored --nocapture`
/// (`ORGUE_DEBUG=1` en plus : écho seconde par seconde et lieu des erreurs).
#[test]
#[ignore = "diagnostic"]
fn diagnostic_par_harmonique() {
    let nom = std::env::var("ORGUE_LIGNE").unwrap_or("Urbaine".into());
    let mode = if std::env::var_os("ORGUE_RAPIDE").is_some() { Mode::Rapide } else { Mode::Joli };
    let duplex = if std::env::var_os("ORGUE_DUO").is_some() { Duplex::Duo } else { Duplex::PleineBande };
    let mut p = LineParams::preset(&nom).unwrap();
    if std::env::var_os("ORGUE_SANS_CLICS").is_some() {
        p.impulses_per_s = 0.0;
        p.dropouts_per_s = 0.0;
    }
    let r = appel(&p, mode, duplex, 10.0, 100, 0);
    for s in &r.sens {
        eprintln!("calé {} TEB {:.1e} ({} / {}) débit {:.0} SNR {:.1}", s.cale, s.teb(), s.erreurs, s.compares, s.debit_net, s.snr_donnees_db);
        for (f, sm, st, sr, sd, b) in &s.tonique {
            eprintln!("   {f:6.0} Hz  tenue {sm:5.1}  tremblant {st:5.1}  retenu {sr:5.1}  données {sd:5.1} dB  {b} bits");
        }
    }
}

/// Diagnostic : atténuation de l'annuleur, par demi-seconde, appris sur
/// notre motif dans le bruit de ligne puis figé sur nos données. Montre
/// pourquoi le motif finit par des accords agités : appris sur des notes
/// tenues, l'annuleur ne connaît l'écho qu'aux fréquences exactes des
/// harmoniques, alors que les données étalent chaque harmonique sur ±1/T.
#[test]
#[ignore = "diagnostic"]
fn diagnostic_annuleur() {
    use crate::dsp::echo::EchoCanceller;
    use crate::line::{EchoPath, LineSim};
    for voix in [Voix::Appelant, Voix::Appele] {
        for nom in ["Urbaine", "Campagne", "Longue distance", "Orage"] {
            let p = LineParams::preset(nom).unwrap();
            let mut tx = Emetteur::new(voix, Duplex::PleineBande, Mode::Joli);
            tx.pousser(super::banc::prbs(5, 400000));
            tx.programmer_motif(0);
            let lp = partition::duree_motif();
            tx.programmer_donnees(lp + 864);
            let mut ec = EchoCanceller::new();
            let mut chemin = EchoPath::new(p.clone());
            let mut bruit = LineSim::with_seed(LineParams { impulses_per_s: 0.0, ..p }, 3);
            let mut prec = [0.0f32; 160];
            let mut acc = (0.0f64, 0.0f64);
            let mut ligne = String::new();
            for blk in 0..(lp + 8000 * 6) / 160 {
                let mut rx = [0.0f32; 160];
                chemin.process(&prec, &mut rx);
                let mut n = [0.0f32; 160];
                bruit.process(&[0.0; 160], &mut n);
                for i in 0..160 {
                    ec.mu = organiste::pas_annuleur(0.3, blk * 160 + i, lp);
                    let e = ec.process(rx[i] + n[i]) - n[i];
                    acc.0 += (rx[i] as f64).powi(2);
                    acc.1 += (e as f64).powi(2);
                    prec[i] = tx.echantillon();
                    ec.push_tx(prec[i]);
                }
                if blk % 25 == 24 {
                    ligne += &format!(" {:.0}", 10.0 * (acc.0 / acc.1.max(1e-30)).log10());
                    acc = (0.0, 0.0);
                }
            }
            eprintln!("{voix:?} {nom} : atténuation d'écho (dB) par 0,5 s :{ligne}");
        }
    }
}
