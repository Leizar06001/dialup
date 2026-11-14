//! Salon de discussion du FAI : tous les abonnés connectés au fournisseur
//! d'accès se retrouvent dans la même pièce (façon IRC / 3615).
//!
//! Comme `web1998`, c'est un hôte virtuel du tunnel : un OPEN vers
//! `salon:6667` n'ouvre rien sur Internet, il est branché ici. Le nom de
//! chacun est **celui de son abonnement** (authentifié par le central) : on
//! ne peut pas parler au nom d'un autre.
//!
//! Protocole texte, une ligne UTF-8 par message (`\n`) :
//! - client → salon : le texte à dire, ou une commande (`/qui`, `/aide`) ;
//! - salon → client : `<nom> texte` (message d'un autre), ou `* texte`
//!   (annonce : arrivée, départ, réponse à une commande).
//!
//! Ses propres messages ne sont pas renvoyés à l'auteur (il les affiche
//! déjà).

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::{debug, info};

pub const HOST: &str = "salon";
pub const PORT: u16 = 6667;
/// Longueur maximale d'une ligne (octets)
const MAX_LINE: usize = 500;
/// Messages par fenêtre de 10 s avant de faire taire un bavard
const FLOOD_MAX: u32 = 20;

/// `host:port` désigne-t-il le salon ?
pub fn is_virtual_host(host: &str, port: u16) -> bool {
    port == PORT && host.eq_ignore_ascii_case(HOST)
}

struct Member {
    id: u64,
    name: String,
    out: TcpStream,
}

#[derive(Default)]
pub struct Salon {
    members: Mutex<Vec<Member>>,
    next_id: Mutex<u64>,
}

impl std::fmt::Debug for Salon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Salon").field("present", &self.present()).finish()
    }
}

impl Salon {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Noms des présents (dans l'ordre d'arrivée, sans doublon).
    pub fn present(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for m in self.members.lock().unwrap().iter() {
            if !v.contains(&m.name) {
                v.push(m.name.clone());
            }
        }
        v
    }

    /// Ouvre une connexion au salon pour l'abonné `user` : renvoie le bout
    /// « client » (à relier au flux du tunnel) ; le bout « salon » est servi
    /// par un thread.
    pub fn open_stream(self: &Arc<Self>, user: &str) -> io::Result<TcpStream> {
        let (client, server) = tcp_pair()?;
        let me = self.clone();
        let user = user.to_string();
        std::thread::Builder::new().name("salon".into()).spawn(move || me.serve(&user, server))?;
        Ok(client)
    }

    fn serve(&self, user: &str, s: TcpStream) {
        let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
        let Ok(out) = s.try_clone() else { return };
        let id = {
            let mut n = self.next_id.lock().unwrap();
            *n += 1;
            *n
        };
        let others = self.present();
        self.members.lock().unwrap().push(Member { id, name: user.to_string(), out });
        self.send_to(id, &format!("* Bienvenue sur le salon du FAI, {user} ! /qui pour voir qui est là, /aide pour l'aide."));
        self.send_to(id, &if others.is_empty() {
            "* Personne d'autre pour l'instant… ça viendra.".to_string()
        } else {
            format!("* Présents : {}", others.join(", "))
        });
        self.broadcast(id, &format!("* {user} entre dans le salon."));
        info!("salon : {user} entre ({} présent(s))", self.present().len());

        let mut reader = BufReader::new(s);
        let mut line = Vec::new();
        let (mut window, mut count) = (Instant::now(), 0u32);
        loop {
            line.clear();
            match Read::take(&mut reader, MAX_LINE as u64 + 1).read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            if line.len() > MAX_LINE && !line.ends_with(b"\n") {
                // ligne trop longue : on jette jusqu'à la fin de ligne
                let mut skip = Vec::new();
                if reader.read_until(b'\n', &mut skip).is_err() {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&line);
            let text: String = text.trim_end_matches(['\r', '\n']).chars().filter(|c| !c.is_control()).collect();
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            if window.elapsed() > Duration::from_secs(10) {
                window = Instant::now();
                count = 0;
            }
            count += 1;
            if count > FLOOD_MAX {
                if count == FLOOD_MAX + 1 {
                    self.send_to(id, "* Doucement ! Attends quelques secondes avant de reparler.");
                }
                continue;
            }
            match text {
                "/qui" | "/who" | "/names" => {
                    let p = self.present();
                    self.send_to(id, &format!("* {} présent(s) : {}", p.len(), p.join(", ")));
                }
                "/aide" | "/help" => {
                    self.send_to(id, "* Tout ce que tu tapes est lu par tous les abonnés connectés au FAI.");
                    self.send_to(id, "* /qui : liste des présents.");
                }
                t if t.starts_with('/') => self.send_to(id, "* Commande inconnue (/aide pour l'aide)."),
                t => {
                    debug!("salon : <{user}> {} car.", t.chars().count());
                    self.broadcast(id, &format!("<{user}> {t}"));
                }
            }
        }
        self.leave(id);
        self.broadcast(id, &format!("* {user} quitte le salon."));
        info!("salon : {user} sort ({} présent(s))", self.present().len());
    }

    fn leave(&self, id: u64) {
        let mut m = self.members.lock().unwrap();
        if let Some(i) = m.iter().position(|x| x.id == id) {
            let gone = m.remove(i);
            let _ = gone.out.shutdown(Shutdown::Both);
        }
    }

    fn send_to(&self, id: u64, line: &str) {
        let mut failed = false;
        if let Some(m) = self.members.lock().unwrap().iter_mut().find(|m| m.id == id) {
            failed = writeln!(m.out, "{line}").is_err();
        }
        if failed {
            self.leave(id);
        }
    }

    /// Envoie à tout le monde sauf `except`. Un membre qui ne lit plus
    /// (délai d'écriture dépassé) est retiré.
    fn broadcast(&self, except: u64, line: &str) {
        let mut dead = Vec::new();
        for m in self.members.lock().unwrap().iter_mut().filter(|m| m.id != except) {
            if writeln!(m.out, "{line}").is_err() {
                dead.push(m.id);
            }
        }
        for id in dead {
            self.leave(id);
        }
    }
}

/// Deux sockets TCP reliées entre elles sur 127.0.0.1 (aucun port ne reste
/// ouvert).
fn tcp_pair() -> io::Result<(TcpStream, TcpStream)> {
    let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let a = TcpStream::connect(l.local_addr()?)?;
    let me = a.local_addr()?;
    l.set_nonblocking(true)?;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match l.accept() {
            Ok((b, peer)) if peer == me => {
                b.set_nonblocking(false)?;
                return Ok((a, b));
            }
            Ok(_) => continue,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() > deadline {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "paire de sockets locale"));
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_line(s: &mut BufReader<TcpStream>) -> String {
        let mut l = String::new();
        s.read_line(&mut l).unwrap();
        l.trim_end().to_string()
    }

    #[test]
    fn deux_abonnes_se_parlent() {
        let salon = Salon::new();
        let a = salon.open_stream("raziel").unwrap();
        a.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut ra = BufReader::new(a.try_clone().unwrap());
        assert!(read_line(&mut ra).contains("Bienvenue"));
        assert!(read_line(&mut ra).contains("Personne"));

        let b = salon.open_stream("gaston").unwrap();
        b.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut rb = BufReader::new(b.try_clone().unwrap());
        assert!(read_line(&mut rb).contains("Bienvenue"));
        assert_eq!(read_line(&mut rb), "* Présents : raziel");
        assert_eq!(read_line(&mut ra), "* gaston entre dans le salon.");

        // Le nom vient de l'abonnement : impossible d'usurper quelqu'un.
        (&b).write_all(b"<raziel> c'est moi raziel\n").unwrap();
        assert_eq!(read_line(&mut ra), "<gaston> <raziel> c'est moi raziel");
        (&a).write_all("salut gaston, ça va ?\r\n".as_bytes()).unwrap();
        assert_eq!(read_line(&mut rb), "<raziel> salut gaston, ça va ?");
        (&a).write_all(b"/qui\n").unwrap();
        assert_eq!(read_line(&mut ra), "* 2 présent(s) : raziel, gaston");

        drop(rb);
        b.shutdown(Shutdown::Both).unwrap();
        assert_eq!(read_line(&mut ra), "* gaston quitte le salon.");
        assert_eq!(salon.present(), vec!["raziel".to_string()]);

        // Une ligne énorme est tronquée, pas fatale.
        let big = vec![b'x'; 5000];
        (&a).write_all(&big).unwrap();
        (&a).write_all(b"\n/qui\n").unwrap();
        assert_eq!(read_line(&mut ra), "* 1 présent(s) : raziel");
        let mut rest = [0u8; 1];
        a.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
        assert!(ra.get_mut().read(&mut rest).is_err(), "rien d'autre");
    }
}
