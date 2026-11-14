//! L'écran des services du central qui parlent en trames de chat (3615
//! ORACLE, numéros cachés) : le texte part à la cadence d'un Minitel, et ce
//! que l'appelant tape arrive ligne par ligne.

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError};
use std::time::{Duration, Instant};

use v34_net::bbs::{self, Decoder, Frame, Item};

/// Cadence d'émission des morceaux de texte
const TIC: Duration = Duration::from_millis(100);
/// Caractères au plus par trame (bien moins que `bbs::MAX_PAYLOAD` octets)
const MORCEAU_MAX: usize = 400;

/// L'appelant a raccroché.
#[derive(Debug)]
pub struct Raccroche;

/// Écrit sur la ligne, en trames de chat, à la cadence d'un Minitel.
pub struct Ecran {
    sortie: SyncSender<Vec<u8>>,
    /// caractères par seconde
    cps: f64,
    credit: f64,
    prochain: Instant,
}

impl Ecran {
    /// `bit_s` : cadence d'affichage, 10 bits par caractère.
    pub fn new(sortie: SyncSender<Vec<u8>>, bit_s: u32) -> Ecran {
        Ecran { sortie, cps: bit_s.max(10) as f64 / 10.0, credit: 0.0, prochain: Instant::now() }
    }

    /// Nouvelles lignes (texte de une ou plusieurs lignes, séparées par
    /// `\n`), chacune précédée d'un retour à la ligne : elle ferme la ligne
    /// en cours chez l'appelant, même celle d'un appel précédent.
    pub fn lignes(&mut self, texte: &str) -> Result<(), Raccroche> {
        let mut s = String::with_capacity(texte.len() + 8);
        for l in texte.split('\n') {
            s.push('\n');
            s.push_str(l);
        }
        self.taper(&s)
    }

    /// Suite de la ligne en cours (points d'attente…).
    pub fn suite(&mut self, texte: &str) -> Result<(), Raccroche> {
        self.taper(texte)
    }

    /// Envoie le texte par petits morceaux, à la cadence choisie.
    fn taper(&mut self, s: &str) -> Result<(), Raccroche> {
        let cs: Vec<char> = s.chars().collect();
        let mut i = 0;
        let maintenant = Instant::now();
        if maintenant > self.prochain + TIC {
            // l'écran était au repos : on repart d'ici, sans rattrapage
            self.prochain = maintenant;
            self.credit = 0.0;
        }
        while i < cs.len() {
            let now = Instant::now();
            if self.prochain > now {
                std::thread::sleep(self.prochain - now);
            }
            self.prochain += TIC;
            self.credit += self.cps * TIC.as_secs_f64();
            let n = (self.credit.floor() as usize).min(cs.len() - i).min(MORCEAU_MAX);
            if n == 0 {
                continue;
            }
            self.credit -= n as f64;
            let morceau: String = cs[i..i + n].iter().collect();
            i += n;
            self.sortie.send(bbs::encode(&Frame::Chat(morceau))).map_err(|_| Raccroche)?;
        }
        Ok(())
    }
}

/// Ce que l'appelant envoie : questions (trames de chat, ou lignes de texte
/// brut d'un terminal à l'ancienne) et fichiers (refusés poliment).
#[derive(Default)]
pub struct Entree {
    dec: Decoder,
    brut: String,
    /// Lignes reçues, pas encore traitées
    pub file: VecDeque<String>,
    /// L'appelant a proposé un fichier
    pub fichier: bool,
}

impl Entree {
    pub fn pousser(&mut self, octets: &[u8]) {
        for it in self.dec.push(octets) {
            match it {
                Item::Frame(Frame::Chat(t)) => self.file.push_back(t),
                Item::Frame(Frame::FileOffer { .. }) => self.fichier = true,
                Item::Raw(r) => {
                    self.brut.push_str(&String::from_utf8_lossy(&r));
                    while let Some(i) = self.brut.find(['\r', '\n']) {
                        let l: String = self.brut.drain(..=i).collect();
                        if !l.trim().is_empty() {
                            self.file.push_back(l);
                        }
                    }
                    if self.brut.len() > 4096 {
                        self.brut.clear();
                    }
                }
                // visiophone, morceaux de fichiers… : ignorés
                _ => {}
            }
        }
    }

    /// Lit ce qui est arrivé, sans attendre. Faux si l'appelant a raccroché.
    pub fn relever(&mut self, rx: &Receiver<Vec<u8>>) -> bool {
        loop {
            match rx.try_recv() {
                Ok(b) => self.pousser(&b),
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => return false,
            }
        }
    }
}

