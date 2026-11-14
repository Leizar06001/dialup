//! Phases 3 et 4 du V.90 côté PCM, sans le sens montant : l'émetteur du
//! FAI et le récepteur de l'abonné, reliés par la ligne simulée ; un petit
//! chef d'orchestre joue le rôle des échanges V.34 (Ja, S, CP…).

use v34_core::line::{LineParams, LineSim};
use v34_core::v90::design::{self, Channel, Limits};
use v34_core::v90::frame::rate_for;
use v34_core::v90::pcmrx::{PcmRx, RxEvent};
use v34_core::v90::pcmtx::PcmTx;
use v34_core::v90::seq::{up_mask_upto, DilDescriptor, Jd, Mp90};
use v34_core::v90::Law;

fn dil() -> DilDescriptor {
    DilDescriptor {
        sp: (0..13).map(|i| (0b1_0110_0101_1101_u32 >> i) & 1 == 1).collect(),
        tp: vec![true],
        h: [7; 8],
        refs: [96; 8],
        train: (4..128).step_by(8).map(|u| u as u8).collect(),
    }
}

struct Bilan {
    log: Vec<String>,
    rate: u32,
    errors: usize,
    bits: usize,
}

fn essai(p: &LineParams) -> Bilan {
    let law = Law::A;
    let uinfo = 96;
    let mut tx = PcmTx::new(law, uinfo);
    let mut rx = PcmRx::new(law, uinfo, Some(dil()));
    let mut line = LineSim::with_seed(p.clone(), 7);
    let mut log = Vec::new();
    // silence initial, puis le FAI démarre (Ja reçu)
    let mut started = false;
    let mut data_tx_bits: Vec<bool> = Vec::new();
    let mut seed = 0x9e3779b97f4a7c15u64;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed & 1 == 1
    };
    let mut mapping = None;
    let mut sent_data = false;
    let mut got_mp = false;
    for n in 0..8000 * 12 {
        if n == 800 && !started {
            started = true;
            tx.start_phase3(&Jd::upto(56000), Some(dil()));
        }
        let mut bit = || {
            let b = rnd();
            data_tx_bits.push(b);
            b
        };
        let s = if tx.in_data() { tx.next_sample(&mut bit) } else { tx.next_sample(&mut || true) };
        if tx.in_data() && !sent_data {
            sent_data = true;
        }
        let mut y = [0f32];
        line.process(&[s], &mut y);
        rx.push(y[0]);
        for ev in tx.take_events() {
            log.push(format!("{n:6} FAI    {ev:?}"));
        }
        for ev in rx.take_events() {
            log.push(format!("{n:6} ABONNÉ {ev:?}"));
            match ev {
                RxEvent::Jd(_) => tx.finish_jd(),
                RxEvent::DilDone { .. } => {
                    let ch = Channel::nominal(law, rx.sigma);
                    let cpt = design::design_range(&ch, &Limits { margin: 14.0, ..Limits::default() }, 6, 18).expect("jeu de phase 4");
                    let cp = design::design(&ch, &Limits::default()).expect("jeu de données");
                    log.push(format!("       jeux : phase 4 {} bit/s, données {} bit/s", cpt.rate(), cp.rate()));
                    rx.set_train_map(cpt.clone());
                    rx.set_data_map(cp.clone());
                    mapping = Some((cpt, cp));
                    tx.stop_dil();
                }
                RxEvent::RiBar => {}
                RxEvent::Mp(mp) => {
                    if !got_mp {
                        got_mp = true;
                        // le CP arrive : MP′, puis Ed/B1d quand le CP′ est là
                        let (_, cp) = mapping.clone().unwrap();
                        tx.set_mp(Mp90 { ack: true, ..mp }.to_bits(), true);
                        tx.finish_mp(cp);
                    }
                }
                RxEvent::Lost(why) => panic!("perdu : {why}\n{}", log.join("\n")),
                _ => {}
            }
        }
        // le FAI a reçu le CPt pendant Ri : TRN2d
        if let Some((cpt, _)) = &mapping {
            if !tx_started_trn2d(&log) && log.iter().any(|l| l.contains("Signal(\"Ri\")")) {
                let mp = Mp90 { up_drn: 13, trellis: 0, nonlinear: false, shaping_expanded: false, ack: false, up_rates: up_mask_upto(31200) };
                tx.start_trn2d(cpt.clone(), mp.to_bits());
                log.push("       (CPt reçu par le FAI)".into());
            }
        }
        if rx.in_data() && rx.data.len() > 20000 {
            break;
        }
    }
    // les bits de données : le FAI les a émis à partir de B1d
    let got = &rx.data;
    let n = got.len().min(data_tx_bits.len());
    let errors = (0..n).filter(|&i| got[i] != data_tx_bits[i]).count();
    Bilan { log, rate: rx.rate(), errors, bits: n }
}

fn tx_started_trn2d(log: &[String]) -> bool {
    log.iter().any(|l| l.contains("CPt reçu par le FAI"))
}

#[test]
fn poignee_pcm_parfaite() {
    let b = essai(&LineParams::preset("Parfaite").unwrap());
    let dump = b.log.join("\n");
    assert_eq!(b.rate, rate_for(36, 6), "{dump}");
    assert!(b.bits > 10000, "{dump}");
    assert_eq!(b.errors, 0, "{dump}");
    eprintln!("{dump}");
}

#[test]
fn poignee_pcm_urbaine() {
    let b = essai(&LineParams::preset("Urbaine").unwrap());
    let dump = b.log.join("\n");
    assert!(b.rate > 33600, "{dump}");
    assert!(b.bits > 10000, "{dump}");
    assert!(b.errors * 10000 <= b.bits, "{} erreurs sur {}\n{dump}", b.errors, b.bits);
    eprintln!("{dump}");
}
