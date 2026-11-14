//! Multiplexeur commun aux deux bouts du tunnel : trames vers le modem,
//! et table des flux TCP ouverts (sockets non bloquantes).
//!
//! Contrôle de flux : on ne lit les sockets que si la file vers le modem
//! est presque vide ([`OUT_HIGH`]) ; le modem impose donc son débit aux
//! connexions TCP (via les fenêtres TCP du noyau), sans mémoire illimitée.

use std::collections::{BTreeMap, VecDeque};
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::mpsc::{SyncSender, TrySendError};

use super::proto::{Decoded, Decoder, Encoder, Frame, DATA_CHUNK};

/// Au-delà, on arrête de lire les sockets (octets en attente vers le modem).
pub const OUT_HIGH: usize = 2048;
/// Octets reçus du modem en attente d'écriture sur une socket : au-delà, le
/// flux est coupé (l'application locale ne lit plus).
pub const WBUF_MAX: usize = 256 * 1024;

struct MuxStream {
    sock: TcpStream,
    wbuf: VecDeque<u8>,
    /// CLOSE reçu : fermer dès que `wbuf` est vidé
    closing: bool,
}

/// Fin d'un flux du côté local.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamEnd {
    /// La socket locale est fermée ou en erreur (CLOSE envoyé)
    Local(u16),
    /// CLOSE reçu et données restantes écrites
    Remote(u16),
}

/// Le canal vers le modem est fermé (liaison terminée).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModemGone;

#[derive(Default)]
pub struct Mux {
    enc: Encoder,
    pub dec: Decoder,
    out: VecDeque<Vec<u8>>,
    out_bytes: usize,
    streams: BTreeMap<u16, MuxStream>,
    rr: u16,
}

impl Mux {
    pub fn new() -> Self {
        Self::default()
    }

    /// Met une trame en file vers le modem.
    pub fn send(&mut self, frame: &Frame) {
        let b = self.enc.encode(frame);
        self.send_raw(b);
    }

    /// Octets bruts vers le modem (texte de la phase de login).
    pub fn send_raw(&mut self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        self.out_bytes += bytes.len();
        self.out.push_back(bytes);
    }

    pub fn out_len(&self) -> usize {
        self.out_bytes
    }

    /// Pousse la file vers le canal du modem ; renvoie vrai si quelque chose
    /// est parti.
    pub fn flush(&mut self, to_modem: &SyncSender<Vec<u8>>) -> Result<bool, ModemGone> {
        let mut progress = false;
        while let Some(front) = self.out.pop_front() {
            let n = front.len();
            match to_modem.try_send(front) {
                Ok(()) => {
                    self.out_bytes -= n;
                    progress = true;
                }
                Err(TrySendError::Full(back)) => {
                    self.out.push_front(back);
                    break;
                }
                Err(TrySendError::Disconnected(_)) => return Err(ModemGone),
            }
        }
        Ok(progress)
    }

    pub fn len(&self) -> usize {
        self.streams.len()
    }

    pub fn is_empty(&self) -> bool {
        self.streams.is_empty()
    }

    pub fn contains(&self, id: u16) -> bool {
        self.streams.contains_key(&id)
    }

    pub fn ids(&self) -> Vec<u16> {
        self.streams.keys().copied().collect()
    }

    /// Ajoute un flux ouvert. `first` : octets à écrire en premier (réponse
    /// SOCKS / HTTP du côté client).
    pub fn attach(&mut self, id: u16, sock: TcpStream, first: &[u8]) -> io::Result<()> {
        sock.set_nonblocking(true)?;
        let _ = sock.set_nodelay(true);
        self.streams.insert(id, MuxStream { sock, wbuf: first.iter().copied().collect(), closing: false });
        Ok(())
    }

    /// Données DATA reçues pour un flux. Faux si le flux est inconnu ou
    /// vient d'être coupé pour débordement.
    pub fn write(&mut self, id: u16, data: &[u8]) -> bool {
        let Some(s) = self.streams.get_mut(&id) else {
            return false;
        };
        if s.closing {
            return true;
        }
        if s.wbuf.len() + data.len() > WBUF_MAX {
            self.drop_stream(id);
            self.send(&Frame::Close { id });
            return false;
        }
        s.wbuf.extend(data);
        true
    }

    /// CLOSE reçu : on finit d'écrire puis on ferme.
    pub fn close_remote(&mut self, id: u16) {
        if let Some(s) = self.streams.get_mut(&id) {
            s.closing = true;
        }
    }

    /// Ferme un flux localement et prévient l'autre bout.
    pub fn close_local(&mut self, id: u16) {
        if self.streams.contains_key(&id) {
            self.drop_stream(id);
            self.send(&Frame::Close { id });
        }
    }

    /// Ferme tous les flux (perte détectée, fin du tunnel). Renvoie leurs ids.
    pub fn reset_all(&mut self, notify_peer: bool) -> Vec<u16> {
        let ids = self.ids();
        for &id in &ids {
            self.drop_stream(id);
            if notify_peer {
                self.send(&Frame::Close { id });
            }
        }
        ids
    }

    fn drop_stream(&mut self, id: u16) {
        if let Some(s) = self.streams.remove(&id) {
            let _ = s.sock.shutdown(Shutdown::Both);
        }
    }

    /// Prochaine trame (ou perte) reçue du modem.
    pub fn next_decoded(&mut self) -> Option<Decoded> {
        self.dec.pop()
    }

    /// Écrit ce qui est en attente vers les sockets, lit ce qui en arrive.
    /// Renvoie (activité, flux terminés).
    pub fn pump(&mut self) -> (bool, Vec<StreamEnd>) {
        let mut progress = false;
        let mut ended = Vec::new();
        // écritures
        let ids = self.ids();
        for id in &ids {
            let s = self.streams.get_mut(id).expect("flux présent");
            let mut failed = false;
            while !s.wbuf.is_empty() {
                let (a, _) = s.wbuf.as_slices();
                match s.sock.write(a) {
                    Ok(0) => {
                        failed = true;
                        break;
                    }
                    Ok(n) => {
                        s.wbuf.drain(..n);
                        progress = true;
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => {
                        failed = true;
                        break;
                    }
                }
            }
            if failed {
                self.close_local(*id);
                ended.push(StreamEnd::Local(*id));
            } else if s.closing && s.wbuf.is_empty() {
                self.drop_stream(*id);
                ended.push(StreamEnd::Remote(*id));
            }
        }
        // lectures, à tour de rôle, tant que la file vers le modem le permet
        let ids = self.ids();
        if ids.is_empty() {
            return (progress, ended);
        }
        let start = ids.iter().position(|&i| i > self.rr).unwrap_or(0);
        let mut buf = [0u8; DATA_CHUNK];
        for k in 0..ids.len() {
            if self.out_bytes >= OUT_HIGH {
                break;
            }
            let id = ids[(start + k) % ids.len()];
            let Some(s) = self.streams.get_mut(&id) else { continue };
            if s.closing {
                continue;
            }
            self.rr = id;
            match s.sock.read(&mut buf) {
                Ok(0) => {
                    self.close_local(id);
                    ended.push(StreamEnd::Local(id));
                    progress = true;
                }
                Ok(n) => {
                    self.send(&Frame::Data { id, data: buf[..n].to_vec() });
                    progress = true;
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => {
                    self.close_local(id);
                    ended.push(StreamEnd::Local(id));
                    progress = true;
                }
            }
        }
        (progress, ended)
    }
}
