//! Rejoue une capture `v34-cli … --wav` dans un modem neuf : la voie gauche
//! (ce que le modem avait reçu) est redonnée telle quelle, bloc par bloc ;
//! la voie droite (ce qu'il avait émis) sert à vérifier que le modem rejoué
//! se comporte comme l'original. Pour déboguer hors ligne une session avec
//! un vrai modem, sans le rappeler.
//!
//!     cargo run --release -p v34-core --example rejoue -- capture.wav [call|answer] [bloc]

use v34_core::{Modem, ModemCommand, ModemConfig, ModemEvent, Role};

fn read_wav(path: &str) -> (Vec<f32>, Vec<f32>) {
    let b = std::fs::read(path).expect("lecture du WAV");
    let channels = u16::from_le_bytes([b[22], b[23]]) as usize;
    assert_eq!(channels, 2, "capture stéréo attendue (gauche = reçu, droite = émis)");
    let data = &b[44..];
    let s = |f: &[u8], c: usize| i16::from_le_bytes([f[2 * c], f[2 * c + 1]]) as f32 / 32767.0;
    data.chunks_exact(4).map(|f| (s(f, 0), s(f, 1))).unzip()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).expect("chemin du WAV");
    let role = if args.get(2).map(String::as_str) == Some("answer") { Role::Answer } else { Role::Call };
    let block: usize = args.get(3).map_or(160, |s| s.parse().unwrap());
    let (rx, tx_ref) = read_wav(path);
    let mut modem = Modem::new(role, ModemConfig::default());
    modem.command(if role == Role::Call { ModemCommand::Dial } else { ModemCommand::Answer });
    let mut tx = vec![0.0f32; block];
    let mut diverged = false;
    let mut got = 0usize;
    for (i, chunk) in rx.chunks_exact(block).enumerate() {
        modem.process(chunk, &mut tx);
        let ms = (i * block) as f64 / 8.0;
        let r = &tx_ref[i * block..(i + 1) * block];
        let err: f32 = tx.iter().zip(r).map(|(a, b)| (a - b) * (a - b)).sum::<f32>() / block as f32;
        let pow: f32 = r.iter().map(|b| b * b).sum::<f32>() / block as f32;
        if !diverged && err > 1e-6 && err > 1e-3 * pow {
            diverged = true;
            println!("[{ms:8.0} ms] (l'émission rejouée s'écarte de l'originale : {:.1} dB)", 10.0 * (err / pow.max(1e-12)).log10());
        }
        for ev in modem.drain_events() {
            match ev {
                ModemEvent::Log { msg, .. } => println!("[{ms:8.0} ms] {msg}"),
                ModemEvent::Connected { tx_rate, rx_rate } => println!("[{ms:8.0} ms] CONNECT {tx_rate}/{rx_rate}"),
                ModemEvent::Data(d) => got += d.len(),
                ModemEvent::Disconnected { reason } => println!("[{ms:8.0} ms] NO CARRIER {reason}"),
                _ => {}
            }
        }
    }
    println!("octets reçus : {got}");
}
