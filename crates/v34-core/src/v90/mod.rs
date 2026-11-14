//! # V.90 : 56 000 bit/s vers l'abonné
//!
//! Recommandation UIT-T V.90 (09/98), `docs/specs/ITU-T_V.90_1998-09.pdf`.
//!
//! Le FAI est raccordé au réseau en numérique : au lieu de moduler une
//! porteuse, son modem (« numérique ») choisit directement, 8000 fois par
//! seconde, l'octet G.711 que le central convertira en tension sur la ligne
//! de l'abonné. Le modem de l'abonné (« analogique ») n'a plus qu'à
//! reconnaître ces niveaux à travers sa boucle locale : jusqu'à 7 bits par
//! symbole, 56 000 bit/s. Dans l'autre sens, rien ne change : V.34.
//!
//! - [`codes`] : les 128 niveaux G.711 (Ucodes, tableau 1) ;
//! - [`frame`] : la trame de 6 symboles, codeur à modules, signes (§5.4) ;
//! - [`dfe`] : égaliseur à retour de décision et annuleur d'écho du
//!   récepteur PCM ;
//! - [`design`] : choix des niveaux selon le bruit mesuré (séquence CP) ;
//! - [`seq`] : séquences de la poignée de main (Ja/DIL, Jd, CP, MP) ;
//! - [`pcmtx`] : émetteur PCM du modem numérique (phases 3, 4, données) ;
//! - [`pcmrx`] : récepteur PCM du modem analogique.

pub mod codes;
pub mod design;
pub mod dfe;
pub mod frame;
pub mod pcmrx;
pub mod pcmtx;
pub mod seq;

pub use codes::Law;
pub use frame::{Decoder, Encoder, Mapping, Symbol};
