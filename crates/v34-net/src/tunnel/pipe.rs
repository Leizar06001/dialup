//! Raccord entre un modem (`ModemEvent::Data` / `ModemCommand::Send`) et un
//! bout de tunnel alimenté par une paire de canaux d'octets.
//!
//! Le modem n'est pas une socket : il accepte n'importe quelle quantité
//! d'octets et les émet au débit de la ligne. Le tuyau les lui donne donc
//! au compte-gouttes (seau à jetons au débit négocié), ce qui fait remonter
//! la contre-pression jusqu'aux connexions TCP au lieu de gonfler une file.

use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TryRecvError};
use std::time::Instant;

/// Fraction du débit nominal utilisée (marge pour le tramage LAPM).
const RATE_FACTOR: f64 = 0.85;
/// Réserve maximale de jetons, en secondes de ligne.
const BURST_S: f64 = 0.25;
/// Trames d'avance dans le canal tunnel → modem.
const CHANNEL_DEPTH: usize = 4;
/// Octets pris d'avance dans le canal.
const PENDING_MAX: usize = 1024;

pub struct ModemPipe {
    to_tunnel: Option<Sender<Vec<u8>>>,
    from_tunnel: Receiver<Vec<u8>>,
    tunnel_gone: bool,
    pending: VecDeque<u8>,
    bytes_per_s: f64,
    tokens: f64,
    last: Instant,
}

impl ModemPipe {
    /// Crée le tuyau pour un modem connecté à `tx_rate_bps` bit/s.
    /// Renvoie aussi les deux bouts à donner au tunnel :
    /// (octets reçus du modem, octets à envoyer au modem).
    pub fn new(tx_rate_bps: u32) -> (ModemPipe, Receiver<Vec<u8>>, SyncSender<Vec<u8>>) {
        let (to_tunnel, tunnel_in) = mpsc::channel();
        let (tunnel_out, from_tunnel) = mpsc::sync_channel(CHANNEL_DEPTH);
        let rate = if tx_rate_bps == 0 { 2400 } else { tx_rate_bps };
        let bytes_per_s = rate as f64 / 8.0 * RATE_FACTOR;
        let pipe = ModemPipe {
            to_tunnel: Some(to_tunnel),
            from_tunnel,
            tunnel_gone: false,
            pending: VecDeque::new(),
            bytes_per_s,
            tokens: (bytes_per_s * 0.05).max(16.0),
            last: Instant::now(),
        };
        (pipe, tunnel_in, tunnel_out)
    }

    /// Même chose pour un modem asynchrone (le V.23 du Minitel) :
    /// `bits_par_caractere` bits par octet sur la ligne (10 : départ, 7 bits,
    /// parité, arrêt), sans la marge du tramage LAPM. Le modem ne prend
    /// ainsi jamais d'avance : l'écho des touches ne fait pas la queue
    /// derrière des secondes de texte.
    pub fn asynchrone(tx_rate_bps: u32, bits_par_caractere: u32) -> (ModemPipe, Receiver<Vec<u8>>, SyncSender<Vec<u8>>) {
        let (mut pipe, a, b) = ModemPipe::new(tx_rate_bps);
        pipe.bytes_per_s = tx_rate_bps.max(10) as f64 / bits_par_caractere.max(1) as f64;
        pipe.tokens = (pipe.bytes_per_s * 0.05).max(4.0);
        (pipe, a, b)
    }

    /// Octets reçus par le modem (`ModemEvent::Data`).
    pub fn on_modem_data(&mut self, data: Vec<u8>) {
        if let Some(tx) = &self.to_tunnel {
            if tx.send(data).is_err() {
                self.to_tunnel = None;
            }
        }
    }

    /// Le modem ne reçoit plus rien (déconnexion) : le tunnel le verra.
    pub fn close_input(&mut self) {
        self.to_tunnel = None;
    }

    /// Octets à donner maintenant à `ModemCommand::Send` (à appeler à
    /// chaque bloc audio, typiquement toutes les 20 ms).
    pub fn take_for_modem(&mut self) -> Option<Vec<u8>> {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + dt * self.bytes_per_s).min(self.bytes_per_s * BURST_S + 16.0);
        while self.pending.len() < PENDING_MAX && !self.tunnel_gone {
            match self.from_tunnel.try_recv() {
                Ok(b) => self.pending.extend(b),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => self.tunnel_gone = true,
            }
        }
        let n = (self.tokens as usize).min(self.pending.len());
        if n == 0 {
            return None;
        }
        self.tokens -= n as f64;
        Some(self.pending.drain(..n).collect())
    }

    /// Vrai quand le tunnel s'est arrêté et que tout a été donné au modem.
    pub fn tunnel_finished(&self) -> bool {
        self.tunnel_gone && self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn debit_limite_et_fin() {
        let (mut pipe, tin, tout) = ModemPipe::new(33600);
        // modem → tunnel
        pipe.on_modem_data(b"abc".to_vec());
        assert_eq!(tin.recv().unwrap(), b"abc");
        // tunnel → modem : 4000 octets, débit ≈ 3570 o/s
        let feeder = std::thread::spawn(move || {
            for _ in 0..40 {
                tout.send(vec![7u8; 100]).unwrap();
            }
        });
        let t0 = Instant::now();
        let mut got = 0;
        while t0.elapsed() < Duration::from_millis(500) {
            if let Some(b) = pipe.take_for_modem() {
                got += b.len();
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(got > 1000 && got < 2600, "débit non respecté : {got} o en 0,5 s");
        while !pipe.tunnel_finished() {
            pipe.take_for_modem();
            std::thread::sleep(Duration::from_millis(20));
        }
        feeder.join().unwrap();
    }
}
