//! # v34-core
//!
//! Modem logiciel conforme à ITU-T V.8 (négociation d'appel) et V.34
//! (phases 2 à 4 + mode données). Tout le traitement se fait sur des
//! échantillons audio 8 kHz : le signal audio est le SEUL lien entre les
//! deux modems, exactement comme sur une ligne téléphonique.
//!
//! Utilisation :
//! ```no_run
//! use v34_core::{Modem, ModemCommand, ModemConfig, Role};
//! let mut m = Modem::new(Role::Call, ModemConfig::default());
//! m.command(ModemCommand::Dial);
//! let rx = [0.0f32; 160];
//! let mut tx = [0.0f32; 160];
//! m.process(&rx, &mut tx); // toutes les 20 ms
//! for ev in m.drain_events() { println!("{ev:?}"); }
//! ```

pub mod api;
pub mod datamode;
pub mod dsp;
pub mod info;
pub mod lapm;
pub mod line;
pub mod probe;
pub mod qamrx;
pub mod train;
mod tx;
mod modem;
mod phase2;
mod phase34;
pub mod v8;

pub use api::*;
pub use line::{EchoPath, LineParams, LineSim};
pub use modem::Modem;
