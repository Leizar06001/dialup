//! Tunnel TCP multiplexé dans le flux d'octets du modem (accès « FAI »).
//!
//! - [`proto`] : format des trames et phase de login texte (documentés là) ;
//! - [`mux`] : table des flux TCP et file vers le modem, commune aux deux bouts ;
//! - [`client`] : bout client avec proxy local SOCKS5 / HTTP CONNECT ;
//! - [`pipe`] : raccord modem ↔ canaux d'octets, au débit de la ligne.
//!
//! Le bout serveur (connexions sortantes vers Internet, filtrage) est dans
//! le crate `v34-server`.

pub mod client;
pub mod mux;
pub mod pipe;
pub mod proto;

pub use client::{ClientConfig, LoginClient, LoginStep, TunnelClient, TunnelEvent, DEFAULT_PROXY};
pub use pipe::ModemPipe;
pub use proto::{Decoded, Decoder, Encoder, FailCode, Frame};
