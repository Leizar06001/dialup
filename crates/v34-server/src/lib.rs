//! # v34-server
//!
//! Partie serveur du modem logiciel V.34, prévue pour un VPS :
//! - un **central téléphonique** sur UDP ([`central`]) : les PC s'y
//!   enregistrent avec un nom d'abonné et un mot de passe (défi-réponse
//!   façon SCRAM, le mot de passe ne circule jamais), reçoivent le numéro
//!   que leur attribue l'annuaire, s'appellent, et le central relaie l'audio ;
//! - des **numéros spéciaux** desservis par des modems du central
//!   ([`services`]) : le « FAI », qui donne accès à Internet par un tunnel
//!   TCP multiplexé dans le flux du modem ([`tunnel`], filtré par
//!   [`filter`]), une ligne de test en écho, et **3615 ORACLE**
//!   ([`oracle`]), une IA persuadée qu'on est en 1998 ;
//! - dans ce tunnel, l'**hôte virtuel `http://web1998/`** ([`web1998`]) :
//!   le serveur sert lui-même des versions allégées « 1998 » des pages web
//!   (HTML 3.2, images GIF 256 couleurs, ou « V.Hallucination » : redessinées
//!   en quelques dizaines de formes SVG), utilisables à 3 Ko/s ;
//! - une **passerelle SIP** ([`sip`]) : un adaptateur téléphonique (ATA)
//!   s'y enregistre comme un abonné, et on y branche un vrai modem, un vrai
//!   fax ou un Minitel.
//!
//! La configuration est un fichier TOML ([`config`]) ; [`admin`] gère
//! l'annuaire depuis la ligne de commande.

pub mod log;

pub mod admin;
pub mod caches;
pub mod central;
pub mod comptes;
pub mod config;
pub mod ecran;
pub mod facturation;
pub mod filter;
pub mod oracle;
pub mod portail;
pub mod salon;
pub mod services;
pub mod sip;
pub mod tinyblog;
pub mod tunnel;
pub mod verrou;
pub mod web1998;

pub use central::{Server, ServerHandle, Stats};
pub use config::Config;
