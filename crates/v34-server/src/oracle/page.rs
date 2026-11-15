//! Les pages Vidéotex de 3615 ORACLE : la boule de cristal en mosaïques, le
//! titre en double taille, la zone des réponses et le champ de la question.
//!
//! ```text
//!  0 │ 3615 ORACLE · 2,23 F/min                        C
//!  1 │ (boule)  3615 ORACLE      ← double taille
//!  2 │ (boule)
//!  3 │ (boule)  l'avenir en direct depuis 1998
//!  4 │ (boule)  2,23 F la minute · Télétel
//!  5 │ ────────────────────────────────────────
//!  6 │ zone des réponses
//!    │ …
//! 18 │
//! 19 │ ────────────────────────────────────────
//! 20 │ Votre question :
//! 21 │ ........................................   ← champ de 3 rangées
//! 23 │ ........................................
//! 24 │  ENVOI  questionner     GUIDE  aide
//! ```

use v34_net::videotex::{BLANC, BLEU, CYAN, JAUNE, MAGENTA, VERT};

use crate::ecran::{Ecran, Raccroche, Style, Taille};

/// La boule de cristal sur son socle : 16 × 12 points, 8 × 4 caractères.
const BOULE: &[&str] = &[
    ".....######.....",
    "...##########...",
    "..############..",
    ".###..#########.",
    ".##..##########.",
    ".##############.",
    "..############..",
    "...##########...",
    ".....######.....",
    "......####......",
    "....########....",
    "..############..",
];

pub const ZONE: (u8, u8) = (6, 18);
/// Le champ de la question : 3 rangées
pub const CHAMP: (u8, u8, usize) = (21, 1, 120);

/// Style des réponses de l'oracle, des questions, de l'attente, du compteur.
pub const REPONSE: Style = Style::fg(BLANC);
pub const QUESTION: Style = Style::fg(CYAN);
pub const ATTENTE: Style = Style::fg(JAUNE);
pub const COMPTEUR: Style = Style::fg(VERT);

/// Le haut de la page : logo, titre, prix.
pub fn entete(e: &mut Ecran) -> Result<(), Raccroche> {
    e.effacer()?;
    e.service("3615 ORACLE · 2,23 F/min")?;
    e.dessin(1, 1, MAGENTA, None, BOULE)?;
    e.ecrire(2, 11, Style::fg(BLANC).taille(Taille::Double), "3615")?;
    e.ecrire(2, 20, Style::fg(JAUNE).taille(Taille::Double), "ORACLE")?;
    e.ecrire(3, 11, Style::fg(CYAN), "l'avenir en direct depuis 1998")?;
    e.ecrire(4, 11, Style::fg(VERT), "2,23 F la minute · Télétel")?;
    e.filet(ZONE.0 - 1, BLEU)?;
    e.filet(ZONE.1 + 1, BLEU)?;
    e.ecrire(20, 1, Style::fg(CYAN), "Votre question :")?;
    e.ecrire(24, 1, Style::fg(BLANC).inverse(), " ENVOI ")?;
    e.ecrire(24, 8, Style::NORMAL, " questionner  ")?;
    e.ecrire(24, 22, Style::fg(BLANC).inverse(), " GUIDE ")?;
    e.ecrire(24, 29, Style::NORMAL, " aide")?;
    e.zone(ZONE.0, ZONE.1, REPONSE);
    e.champ(CHAMP.0, CHAMP.1, CHAMP.2, Style::fg(BLANC))
}

/// L'écran d'accueil.
pub fn accueil(e: &mut Ecran, abonne: &str, restantes: Option<u32>) -> Result<(), Raccroche> {
    entete(e)?;
    e.dire(&super::persona::accueil(abonne, restantes))
}

/// Une nouvelle consultation : la zone se vide, la question s'y inscrit.
pub fn question(e: &mut Ecran, q: &str) -> Result<(), Raccroche> {
    e.vider_zone()?;
    e.dire_style(&format!("> {q}"), QUESTION)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{channel, sync_channel};
    use v34_net::videotex as vt;

    #[test]
    fn la_page_d_accueil() {
        let (tx, rx) = sync_channel(1000);
        let (_clavier, entree) = channel();
        let mut e = Ecran::new(tx, entree, 10_000_000);
        accueil(&mut e, "gaston", Some(30)).unwrap();
        let mut m = vt::Ecran::new();
        m.recevoir(&rx.try_iter().flatten().collect::<Vec<u8>>());
        // le titre en double taille : deux colonnes par lettre, deux rangées
        assert_eq!(m.texte_rangee(2), "          3 6 1 5  O R A C L E");
        assert_ne!(m.cellule(2, 10).attr & vt::A_DOUBLE_HAUTEUR, 0);
        assert!(m.texte_rangee(4).contains("2,23 F la minute"));
        assert_eq!(m.texte_rangee(ZONE.0 as usize), "Bienvenue, gaston. Les astres vous");
        assert_ne!(m.cellule(1, 1).attr & vt::A_MOSAIQUE, 0, "la boule de cristal");
        assert_eq!(m.texte_rangee(21), "........................................");
        assert!(m.curseur_visible && (m.rangee, m.colonne) == (21, 0));
        assert!(m.texte_rangee(24).contains("ENVOI"));
        assert!(m.texte_rangee(0).starts_with("3615 ORACLE"));
    }
}
