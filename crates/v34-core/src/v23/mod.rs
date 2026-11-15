//! V.23 : le modem du Minitel, 1200 bit/s dans un sens, 75 dans l'autre.
//!
//! ```text
//! voie aller  (serveur → Minitel) : 1200 bit/s, 1 = 1300 Hz, 0 = 2100 Hz
//! voie retour (clavier → serveur) :   75 bit/s, 1 =  390 Hz, 0 =  450 Hz
//! ```
//!
//! Les caractères sont asynchrones, comme sur Télétel : un bit de départ
//! (0), sept bits de données (poids faible d'abord), un bit de parité paire,
//! un bit d'arrêt (1). Dix bits par caractère : 120 caractères par seconde
//! vers l'écran, 7,5 par seconde depuis le clavier.
//!
//! L'établissement de l'appel suit V.25, sans V.8 :
//!
//! ```text
//! serveur (appelé)                               Minitel (appelant)
//!   silence ………………………………………………………………
//!   ANS : 2100 Hz pur (2,6 s), sans la modulation à 15 Hz de l'ANSam
//!   75 ms de silence
//!   porteuse 1300 Hz — le célèbre sifflement →
//!                            ← porteuse de retour 390 Hz (le Minitel a reconnu un serveur)
//!   CONNECT 1200/75                              CONNECT 1200/75
//! ```
//!
//! [`fsk`] : modulation, démodulation, caractères ; [`session`] : l'appel,
//! côté serveur et côté Minitel, et l'oreille qui reconnaît un serveur
//! Minitel pendant que le modem attend un ANSam.

mod branchement;
pub mod fsk;
pub mod session;

pub use session::{EcouteMinitel, Minitel};
