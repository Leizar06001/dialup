//! Interface commune des « lignes » audio : liaison directe ([`AudioLink`])
//! ou appel via le central ([`crate::central::CentralClient`]).
//!
//! Le code qui cadence le modem (CLI, appli) peut ainsi être écrit une
//! seule fois, générique sur `P: AudioPort`.

use std::io;

use crate::link::{AudioLink, NetStats};

pub trait AudioPort {
    /// Envoie des échantillons (accumulés en paquets de 20 ms).
    fn send_block(&mut self, samples: &[f32]) -> io::Result<()>;
    /// Remplit `out` avec l'audio reçu ; vrai si tout est du vrai audio.
    fn recv_block(&mut self, out: &mut [f32]) -> bool;
    /// Vrai dès qu'au moins un paquet audio de l'autre côté a été reçu.
    fn has_peer_audio(&self) -> bool;
    fn stats(&self) -> NetStats;
}

impl AudioPort for AudioLink {
    fn send_block(&mut self, samples: &[f32]) -> io::Result<()> {
        AudioLink::send_block(self, samples)
    }
    fn recv_block(&mut self, out: &mut [f32]) -> bool {
        AudioLink::recv_block(self, out)
    }
    fn has_peer_audio(&self) -> bool {
        AudioLink::has_peer_audio(self)
    }
    fn stats(&self) -> NetStats {
        AudioLink::stats(self)
    }
}
