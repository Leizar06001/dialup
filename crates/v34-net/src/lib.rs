//! Transport audio sur IP : la « ligne téléphonique » entre deux modems.
//!
//! Le modem ne transmet QUE du son : des échantillons 8 kHz codés en G.711
//! (comme le réseau téléphonique numérique depuis les années 80), envoyés
//! par paquets UDP de 20 ms, puis remis en ordre par un tampon de gigue.
//!
//! ```no_run
//! use v34_net::{AudioLink, LinkConfig};
//! let mut link = AudioLink::bind("0.0.0.0:3434", LinkConfig::default()).unwrap();
//! link.connect("192.168.1.20:3434").unwrap();
//! let tx = [0.0f32; 160];
//! let mut rx = [0.0f32; 160];
//! link.send_block(&tx).unwrap();
//! link.recv_block(&mut rx);
//! ```
//!
//! Pour passer par le central téléphonique (VPS) plutôt qu'en direct, voir
//! [`central::CentralClient`] (même interface audio, trait [`AudioPort`]) ;
//! pour l'accès Internet par le modem « FAI », voir [`tunnel`].

pub mod auth;
pub mod central;
pub mod crypto;
pub mod g711;
pub mod jitter;
pub mod link;
pub mod packet;
pub mod pace;
pub mod port;
pub mod signal;
pub mod tunnel;

pub use jitter::{Concealment, FrameKind, JitterBuffer, JitterStats};
pub use link::{AudioLink, LinkConfig, NetStats};
pub use pace::Pacer;
pub use packet::{Codec, Packet, FRAME_SAMPLES};
pub use port::AudioPort;

/// Port UDP par défaut (clin d'œil à V.34).
pub const DEFAULT_PORT: u16 = 3434;
