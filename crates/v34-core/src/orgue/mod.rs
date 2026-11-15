//! # L'orgue à données
//!
//! Une modulation qui sonne comme un orgue jouant une mélodie, les données
//! cachées là où l'oreille n'écoute pas.
//!
//! **L'idée.** Les harmoniques d'une note (k·f₀) sont régulièrement
//! espacées, exactement comme les sous-porteuses d'un MROF (OFDM). Si le
//! symbole dure une période entière de la note, la TFD d'une période tombe
//! pile sur les harmoniques : elles sont orthogonales. Le signal étant
//! périodique, le préfixe cyclique n'est rien d'autre que *la note qui
//! continue*. Et selon la loi acoustique d'Ohm, l'oreille entend
//! l'amplitude des harmoniques, pas leur phase : c'est là qu'on cache les
//! bits.
//!
//! **L'accord.** f₀ = 8000/W avec W entier. Par recherche exhaustive, la
//! tonique W = 108 (74,07 Hz, un ré un peu haut) donne des intervalles à
//! moins de 10 cents du tempérament égal pour toute la mélodie et le motif
//! (à moins de 22 cents du la 440 en absolu), et des périodes toutes
//! paires : l'appelé, une octave au-dessus (W/2), joue des octaves
//! exactes. Pas de symboles sur deux périodes (la moitié des cases de la
//! TFD seraient des demi-harmoniques à laisser vides : débit divisé par
//! deux) ; on n'en a pas eu besoin.
//!
//! | voix     | f₀ (Hz)       | W (éch.)  | harmoniques (pleine bande) |
//! |----------|---------------|-----------|----------------------------|
//! | appelant | 55,6 à 93,0   | 144 à 86  | 6..60 à 4..36              |
//! | appelé   | 111 à 186     | 72 à 43   | 3..30 à 2..18              |
//!
//! **Le temps.** Mélodie « Au clair de la lune » (64 temps, 38,4 s, en
//! boucle), noire = 0,6 s. Chaque note commence pile sur son temps : un
//! symbole de référence (phases connues, « l'attaque », après 12 ms
//! d'articulation qui détache les notes répétées) puis des symboles de
//! données de W + 32 échantillons (garde de 4 ms : la ligne y étale ses
//! échos et, dans les 2 ms (joli) ou 1 ms (rapide) du début, le symbole
//! précédent passe au suivant en douceur). Le reste de la division
//! s'ajoute à la garde de la référence.
//!
//! **Mode « joli ».** Amplitudes fixes (registration « principal »,
//! -3 dB/octave) : le timbre ne bouge pas. Les données sont dans le
//! *changement* de phase de chaque harmonique (MDP différentielle, 1 à 4
//! bits selon le bruit mesuré), et ce changement *glisse* pendant la
//! transition, à amplitude constante. Une suite de zéros laisse la note
//! parfaitement tenue.
//!
//! **Mode « rapide ».** MAQ (2 à 256 points, rectangulaire pour les
//! nombres de bits impairs) sur chaque harmonique, chargement en bits
//! d'après le SNR mesuré, une harmonique sur six en pilote. Plus de bits,
//! son plus granuleux (les amplitudes bougent à chaque symbole).
//!
//! **Le motif d'appel** : les cinq notes des *Rencontres du troisième type*
//! (ré mi do do↓ sol↓) puis « la si », chaque note tenue avec des phases
//! connues. Le récepteur y trouve la note (autocorrélation à la période de
//! la première), l'instant exact (filtre adapté au changement de note), le
//! décalage de fréquence de la ligne (rotation d'une période à la suivante
//! sur les notes tenues : avec un décalage Δ toutes les harmoniques
//! glissent de Δ Hz, le son n'est plus harmonique ; on le retire par un
//! mélange complexe avant la TFD, puis une boucle de phase suit le reste),
//! le canal et le bruit de chaque harmonique de chaque note de la mélodie
//! (elles sont toutes dans le motif) → le chargement en bits.
//!
//! **Le duplex.** En *duo*, l'appelant joue dans le grave (320–1650 Hz),
//! l'appelé une octave au-dessus dans l'aigu (1950–3350 Hz). En *pleine
//! bande*, les deux voix occupent toute la bande et seul l'annuleur d'écho
//! (le NLMS de `dsp::echo`, appris sur notre motif pendant que l'autre se
//! tait) les sépare. Dans les deux cas l'annuleur tourne.
//!
//! **Ce qui limite le débit.** (1) La garde : 32 échantillons sur des
//! périodes de 43 à 144, soit 18 à 43 % du temps (l'appelé, une octave
//! au-dessus, en paie deux fois plus) ; plus l'articulation et la
//! référence de chaque note (~5 %). (2) Le SNR par harmonique, que la
//! pente des aigus et la registration (joli) font chuter en haut de bande.
//! (3) La dispersion de la ligne aux bords de bande (passe-haut 300 Hz
//! d'ordre 4) qui déborde de la garde : plancher d'interférence. (4) En
//! duplex, l'écho résiduel ; (5) les clics et coupures (Orage), que seul
//! un code correcteur ou la retransmission (LAPM) rattrapent.
//!
//! Prototype : le chargement mesuré par chaque récepteur est rendu à
//! l'émetteur d'en face *hors bande* (dans un vrai modem, ~200 bits à
//! faire passer pendant la poignée de main), et aucune dérive d'horloge
//! d'échantillonnage n'est suivie (le simulateur de ligne n'en a pas).

pub mod banc;
pub mod chargement;
pub mod constellation;
pub mod emetteur;
pub mod organiste;
pub mod partition;
pub mod recepteur;

pub use chargement::Chargement;
pub use emetteur::Emetteur;
pub use organiste::Organiste;
pub use recepteur::Recepteur;

/// Mode de jeu.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    /// Phases seulement : timbre stable
    Joli,
    /// MAQ : plus de bits, son granuleux
    Rapide,
}

/// Partage de la ligne.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Duplex {
    /// Appelant grave, appelé aigu (bandes séparées)
    Duo,
    /// Tous deux sur toute la bande, séparés par l'annuleur d'écho
    PleineBande,
}

/// Qui joue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Voix {
    /// Grave (tonique 74 Hz)
    Appelant,
    /// Une octave au-dessus
    Appele,
}

impl Voix {
    pub fn autre(self) -> Voix {
        match self {
            Voix::Appelant => Voix::Appele,
            Voix::Appele => Voix::Appelant,
        }
    }
}

/// Retard aller maximal toléré par le protocole (150 ms).
pub const RETARD_MAX: usize = 1200;
/// Silence entre la fin du motif de l'appelant et celui de l'appelé (100 ms).
pub const ECART: usize = 800;

/// Entre la fin du motif d'une voix et le début de ses données
/// (échantillons). L'appelant attend que l'appelé ait joué son motif
/// (+ deux retards maximaux) ; l'appelé commence ses données au moment où
/// celles de l'appelant lui arrivent.
pub fn decalage_donnees(voix: Voix) -> usize {
    match voix {
        Voix::Appelant => ECART + partition::duree_motif() + 2 * RETARD_MAX,
        Voix::Appele => 2 * RETARD_MAX,
    }
}

#[cfg(test)]
mod tests;
