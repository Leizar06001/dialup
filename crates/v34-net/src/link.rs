//! `AudioLink` : une « ligne téléphonique » sur UDP entre deux modems.
//!
//! - `send_block` accepte des blocs de n'importe quelle taille et émet un
//!   paquet toutes les 160 échantillons (20 ms).
//! - `recv_block` vide la socket (non bloquante) dans le tampon de gigue et
//!   restitue des échantillons à cadence régulière.
//!
//! Si aucun pair n'est configuré, le premier paquet reçu fixe le pair
//! (mode « répondeur »). Le champ `call_id` est prévu pour le futur central
//! téléphonique sur VPS (0 = liaison directe, accepte tout).

use std::collections::VecDeque;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::Instant;

use serde::Serialize;

use crate::jitter::{Concealment, FrameKind, JitterBuffer, JitterStats};
use crate::packet::{Codec, Packet, FRAME_SAMPLES};

#[derive(Clone, Debug, PartialEq)]
pub struct LinkConfig {
    pub codec: Codec,
    /// Identifiant d'appel (0 = liaison directe)
    pub call_id: u32,
    /// Profondeur cible initiale du tampon de gigue (ms)
    pub jitter_target_ms: u32,
    pub concealment: Concealment,
}

impl Default for LinkConfig {
    fn default() -> Self {
        Self { codec: Codec::Alaw, call_id: 0, jitter_target_ms: 140, concealment: Concealment::Modem }
    }
}

/// Statistiques de la liaison, sérialisables pour l'IHM.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct NetStats {
    pub peer: Option<String>,
    pub codec: String,
    pub packets_sent: u64,
    pub packets_received: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    /// Datagrammes illisibles
    pub invalid: u64,
    /// Paquets d'une autre source ou d'un autre appel
    pub foreign: u64,
    pub jitter: JitterStats,
}

pub struct AudioLink {
    sock: UdpSocket,
    peer: Option<SocketAddr>,
    cfg: LinkConfig,
    tx_seq: u32,
    tx_ts: u32,
    tx_acc: Vec<f32>,
    jb: JitterBuffer,
    rx_fifo: VecDeque<(f32, bool)>,
    epoch: Instant,
    stats: NetStats,
    got_audio: bool,
}

impl AudioLink {
    /// Ouvre une socket UDP locale (ex. "0.0.0.0:3434", "0.0.0.0:0").
    pub fn bind(local: impl ToSocketAddrs, cfg: LinkConfig) -> io::Result<Self> {
        let sock = UdpSocket::bind(local)?;
        sock.set_nonblocking(true)?;
        let jb = JitterBuffer::new(cfg.jitter_target_ms, cfg.concealment);
        let stats = NetStats { codec: cfg.codec.name().to_string(), ..NetStats::default() };
        Ok(Self {
            sock,
            peer: None,
            cfg,
            tx_seq: 0,
            tx_ts: 0,
            tx_acc: Vec::with_capacity(FRAME_SAMPLES),
            jb,
            rx_fifo: VecDeque::new(),
            epoch: Instant::now(),
            stats,
            got_audio: false,
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.sock.local_addr()
    }

    /// Fixe l'adresse de l'autre modem.
    pub fn connect(&mut self, peer: impl ToSocketAddrs) -> io::Result<()> {
        let addr = peer
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "adresse introuvable"))?;
        self.peer = Some(addr);
        self.stats.peer = Some(addr.to_string());
        Ok(())
    }

    pub fn peer(&self) -> Option<SocketAddr> {
        self.peer
    }

    pub fn config(&self) -> &LinkConfig {
        &self.cfg
    }

    /// Vrai dès qu'au moins un paquet audio de l'autre côté a été reçu.
    pub fn has_peer_audio(&self) -> bool {
        self.got_audio
    }

    /// Envoie des échantillons (accumulés en paquets de 20 ms).
    /// Sans pair connu, les échantillons sont jetés.
    pub fn send_block(&mut self, samples: &[f32]) -> io::Result<()> {
        let mut rest = samples;
        while !rest.is_empty() {
            let take = (FRAME_SAMPLES - self.tx_acc.len()).min(rest.len());
            self.tx_acc.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            if self.tx_acc.len() == FRAME_SAMPLES {
                let pkt = Packet {
                    codec: self.cfg.codec,
                    call_id: self.cfg.call_id,
                    seq: self.tx_seq,
                    timestamp: self.tx_ts,
                    samples: std::mem::take(&mut self.tx_acc),
                };
                self.tx_acc.reserve(FRAME_SAMPLES);
                self.tx_seq = self.tx_seq.wrapping_add(1);
                self.tx_ts = self.tx_ts.wrapping_add(FRAME_SAMPLES as u32);
                if let Some(peer) = self.peer {
                    let bytes = pkt.encode();
                    match self.sock.send_to(&bytes, peer) {
                        Ok(n) => {
                            self.stats.packets_sent += 1;
                            self.stats.bytes_sent += n as u64;
                        }
                        // Socket pleine ou pair momentanément injoignable : le
                        // paquet est perdu, comme sur un vrai réseau.
                        Err(e) if is_transient(&e) => {}
                        Err(e) => return Err(e),
                    }
                }
            }
        }
        Ok(())
    }

    /// Vide la socket dans le tampon de gigue. Renvoie le nombre de paquets acceptés.
    pub fn poll(&mut self) -> io::Result<usize> {
        let mut buf = [0u8; 2048];
        let mut accepted = 0;
        loop {
            match self.sock.recv_from(&mut buf) {
                Ok((n, from)) => {
                    let Some(pkt) = Packet::decode(&buf[..n]) else {
                        self.stats.invalid += 1;
                        continue;
                    };
                    if self.cfg.call_id != 0 && pkt.call_id != self.cfg.call_id {
                        self.stats.foreign += 1;
                        continue;
                    }
                    match self.peer {
                        None => {
                            self.peer = Some(from);
                            self.stats.peer = Some(from.to_string());
                        }
                        Some(p) if p != from => {
                            self.stats.foreign += 1;
                            continue;
                        }
                        _ => {}
                    }
                    self.stats.packets_received += 1;
                    self.stats.bytes_received += n as u64;
                    self.got_audio = true;
                    let arrival = self.epoch.elapsed().as_secs_f64();
                    self.jb.push(pkt.seq, pkt.timestamp, pkt.samples, arrival);
                    accepted += 1;
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                // Windows signale les ICMP « port injoignable » par ConnectionReset.
                Err(e) if is_transient(&e) => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(accepted)
    }

    /// Remplit `out` avec l'audio reçu. Renvoie `true` si tout `out` est du
    /// vrai audio (pas de silence de remplissage ni de masquage de perte).
    pub fn recv_block(&mut self, out: &mut [f32]) -> bool {
        // Les erreurs réseau fatales sont rares en UDP ; on joue du silence.
        let _ = self.poll();
        while self.rx_fifo.len() < out.len() {
            let (frame, kind) = self.jb.pop_frame();
            let real = kind == FrameKind::Audio;
            self.rx_fifo.extend(frame.into_iter().map(|s| (s, real)));
        }
        let mut all_real = true;
        for o in out.iter_mut() {
            let (s, real) = self.rx_fifo.pop_front().unwrap_or((0.0, false));
            *o = s;
            all_real &= real;
        }
        all_real
    }

    pub fn stats(&self) -> NetStats {
        let mut s = self.stats.clone();
        s.jitter = self.jb.stats().clone();
        s
    }
}

fn is_transient(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionRefused | io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn frame(v: f32) -> Vec<f32> {
        (0..FRAME_SAMPLES).map(|i| v + i as f32 * 1e-4).collect()
    }

    #[test]
    fn two_links_exchange_audio() {
        // Tampon court : le test n'envoie que 5 trames.
        let cfg = LinkConfig { codec: Codec::L16, jitter_target_ms: 60, ..LinkConfig::default() };
        let mut a = AudioLink::bind("127.0.0.1:0", cfg.clone()).unwrap();
        let mut b = AudioLink::bind("127.0.0.1:0", cfg).unwrap();
        a.connect(b.local_addr().unwrap()).unwrap();
        // b ne se connecte pas : il apprend l'adresse de a au premier paquet.

        // Envoi en blocs de taille irrégulière (100 + 60 = 1 paquet, etc.)
        let all: Vec<f32> = (0..5).flat_map(|k| frame(k as f32 * 0.1)).collect();
        for chunk in all.chunks(100) {
            a.send_block(chunk).unwrap();
        }
        std::thread::sleep(Duration::from_millis(50));

        let mut got = Vec::new();
        for _ in 0..5 {
            let mut out = vec![0.0f32; FRAME_SAMPLES];
            assert!(b.recv_block(&mut out));
            got.extend(out);
        }
        for (x, y) in all.iter().zip(&got) {
            assert!((x - y).abs() < 1e-4, "{x} {y}");
        }
        assert_eq!(b.peer(), Some(a.local_addr().unwrap()));
        assert_eq!(b.stats().packets_received, 5);

        // Retour : b répond à a
        b.send_block(&frame(0.3)).unwrap();
        for _ in 0..2 {
            b.send_block(&frame(0.4)).unwrap();
        }
        std::thread::sleep(Duration::from_millis(50));
        let mut out = vec![0.0f32; FRAME_SAMPLES];
        assert!(a.recv_block(&mut out));
        assert!((out[0] - 0.3).abs() < 1e-4);
    }

    #[test]
    fn g711_link_quality() {
        let cfg = LinkConfig { jitter_target_ms: 60, ..LinkConfig::default() };
        let mut a = AudioLink::bind("127.0.0.1:0", cfg.clone()).unwrap();
        let mut b = AudioLink::bind("127.0.0.1:0", cfg).unwrap();
        a.connect(b.local_addr().unwrap()).unwrap();
        let tone: Vec<f32> = (0..FRAME_SAMPLES * 4).map(|i| 0.3 * (i as f32 * 0.2).sin()).collect();
        a.send_block(&tone).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let mut out = vec![0.0f32; FRAME_SAMPLES * 4];
        assert!(b.recv_block(&mut out));
        let err: f32 = tone.iter().zip(&out).map(|(x, y)| (x - y).powi(2)).sum::<f32>() / tone.len() as f32;
        let sig: f32 = tone.iter().map(|x| x * x).sum::<f32>() / tone.len() as f32;
        let snr = 10.0 * (sig / err).log10();
        assert!(snr > 30.0, "SNR G.711 trop faible : {snr:.1} dB");
    }

    #[test]
    fn ignores_foreign_call_ids() {
        let mut a = AudioLink::bind("127.0.0.1:0", LinkConfig { call_id: 7, ..LinkConfig::default() }).unwrap();
        let mut b = AudioLink::bind("127.0.0.1:0", LinkConfig { call_id: 9, ..LinkConfig::default() }).unwrap();
        a.connect(b.local_addr().unwrap()).unwrap();
        a.send_block(&frame(0.1)).unwrap();
        std::thread::sleep(Duration::from_millis(30));
        b.poll().unwrap();
        assert_eq!(b.stats().foreign, 1);
        assert!(!b.has_peer_audio());
    }
}
