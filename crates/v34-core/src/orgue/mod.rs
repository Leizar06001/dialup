//! # L'orgue à données
//!
//! Une modulation qui sonne comme un orgue jouant une mélodie, les données
//! cachées là où l'oreille n'écoute pas. Prototype mesuré sur la ligne
//! simulée, pas encore branché au modem (ni V.8, ni l'appli).
//!
//! **L'idée.** Les harmoniques d'une note (k·f₀) sont régulièrement
//! espacées, exactement comme les sous-porteuses d'un MROF (OFDM). Si le
//! symbole dure une période entière de la note, la TFD d'une période tombe
//! pile sur les harmoniques : elles sont orthogonales. Le signal étant
//! périodique, le préfixe cyclique n'est rien d'autre que *la note qui
//! continue*. Et selon la loi acoustique d'Ohm, l'oreille entend
//! l'amplitude des harmoniques bien plus que leur phase : c'est là qu'on
//! cache les bits.
//!
//! **L'accord.** f₀ = 8000/W avec W entier : à 100 Hz, deux W voisins sont
//! à ~17 cents. Par recherche exhaustive, la tonique W = 108 (74,07 Hz, un
//! ré un peu haut) donne des intervalles à moins de 10 cents du tempérament
//! égal pour toute la mélodie et le motif (à moins de 22 cents du la 440
//! en absolu), et des périodes toutes paires : l'appelé, une octave
//! au-dessus (W/2), joue des octaves exactes. Pas de symboles sur deux
//! périodes : la moitié des cases de la TFD y seraient des
//! demi-harmoniques à laisser vides (sinon la note descend d'une octave),
//! débit divisé par deux — on n'en a pas eu besoin.
//!
//! | voix     | f₀ (Hz)     | W (éch.) | harmoniques, pleine bande | symboles/s |
//! |----------|-------------|----------|---------------------------|------------|
//! | appelant | 55,6 à 93,0 | 144 à 86 | 6..60 (55) à 4..36 (33)    | 45 à 68    |
//! | appelé   | 111 à 186   | 72 à 43  | 3..30 (28) à 2..18 (17)    | 77 à 107   |
//!
//! **Le temps.** Mélodie « Au clair de la lune » (64 temps, 38,4 s, en
//! boucle), noire = 0,6 s, connue des deux bouts. Chaque note commence pile
//! sur son temps : 12 ms d'articulation (l'ancienne s'éteint, la nouvelle
//! s'allume : les notes répétées se détachent), un symbole de référence
//! (phases connues, « l'attaque ») puis des symboles de données de W + 32
//! échantillons. Garde de 4 ms : la transition d'un symbole au suivant en
//! occupe le début (2 ms en joli, 1 ms en rapide), le reste absorbe la
//! dispersion de la ligne. Le reste de la division s'ajoute à la garde de
//! la référence.
//!
//! **Mode « joli ».** Amplitudes fixes (registration « principal »,
//! -3 dB/octave) : le timbre ne bouge pas. Les données sont dans le *pas*
//! de phase de chaque harmonique (1 à 4 bits), qui *glisse* pendant la
//! transition à amplitude constante. Les pas restent dans un arc de ±45°
//! (voir `constellation`) : sur tout le cercle, chaque harmonique
//! s'étalerait sur ±1/T ≈ l'écart entre harmoniques et l'on n'entendrait
//! plus qu'un souffle coloré ; dans l'arc, 77 à 86 % de la puissance reste
//! sur les raies (mesuré), la phase ne fait que diffuser : une « voix
//! céleste ». Le prix : ~12 dB de SNR à nombre de bits égal. Le signe du
//! pas alterne d'un symbole à l'autre : une ligne au repos (des zéros) est
//! une note tenue.
//!
//! **Mode « rapide ».** MAQ de 2 à 1024 points (rectangulaire pour les
//! nombres de bits impairs) sur chaque harmonique, une sur six en pilote.
//! Bien plus de bits ; les amplitudes bougent à chaque symbole, seuls les
//! pilotes restent sur les raies (22 à 31 % de la puissance) : un orgue
//! très soufflé.
//!
//! **Le motif d'appel** : les cinq notes des *Rencontres du troisième type*
//! (ré mi do do↓ sol↓) puis « la si », chacune tenue sa première moitié
//! (phases connues) puis *tremblante* sa seconde (symboles connus, découpés
//! exactement comme les données) ; enfin la mineur – sol majeur – la
//! mineur – sol majeur, accords « agités » (phases tirées à chaque symbole).
//! Le récepteur y trouve la note (autocorrélation à la période de la
//! première), l'instant exact (filtre adapté au passage de la 1ʳᵉ à la 2ᵉ
//! note, vérifié sur les suivants), le décalage de fréquence de la ligne
//! (rotation d'une période à la suivante sur les tenues : avec un décalage
//! Δ toutes les harmoniques glissent de Δ Hz, le son n'est plus
//! harmonique ; on le retire par un mélange complexe avant la TFD — pas
//! besoin de Hilbert, l'image négative tombe sur des cases inutilisées —
//! puis une boucle de phase suit le reste), le canal de chaque harmonique
//! de chaque note de la mélodie (toutes sont dans le motif), et sur le
//! tremblant le vrai rapport signal/(bruit + interférence) des données, que
//! des notes tenues ne montrent pas (la dispersion hors garde ne gêne que
//! les symboles). Mis en commun en fréquence, il décide du chargement en
//! bits (marge 3 dB). Les accords agités servent notre annuleur d'écho :
//! appris sur des notes tenues, il ne connaîtrait l'écho qu'aux fréquences
//! exactes des harmoniques ; il le lui faut sur toute la bande que les
//! données étalent, et pour toutes les notes à la fois.
//!
//! **Le duplex.** En *duo*, l'appelant joue dans le grave (320–1650 Hz),
//! l'appelé une octave au-dessus dans l'aigu (1950–3350 Hz). En *pleine
//! bande*, les deux voix occupent toute la bande et seul l'annuleur d'écho
//! (le NLMS de `dsp::echo`) les sépare : chacun se tait pendant le motif de
//! l'autre, l'annuleur apprend sur le sien (pas décroissant, et pas
//! d'apprentissage sur un clic), puis il est figé pendant le duplex (même un
//! pas minuscule y injecte le désajustement dû au signal d'en face). Les
//! deux voix commencent la mélodie ensemble au moment où elles s'entendent
//! côté appelé : le duo y est en mesure.
//!
//! **Ce qui limite le débit.** (1) La garde : 32 échantillons sur des
//! périodes de 43 à 144, soit 18 à 43 % du temps (l'appelé, une octave
//! au-dessus, en paie deux fois plus) ; plus l'articulation et la
//! référence de chaque note (~5 %). (2) Le SNR par harmonique : pente des
//! aigus, registration (joli) et, en joli, l'arc. (3) La dispersion de la
//! ligne aux bords de bande (passe-haut 300 Hz d'ordre 4) qui déborde de la
//! garde : ~30 dB de plafond sur les harmoniques graves. (4) En duplex,
//! l'écho résiduel (de l'ordre du bruit de ligne, l'annuleur n'ayant
//! appris que quelques secondes dans ce bruit) ; en duo, ce qui en déborde
//! sur les harmoniques voisines de la coupure. (5) Les clics et coupures
//! (Campagne, Orage) : sans eux le taux d'erreur brut tombe sous 1e-4 ;
//! avec, ~1e-3 (une salve par clic) que seuls un code correcteur entrelacé
//! ou la retransmission (LAPM) rattrapent.
//!
//! Prototype : le chargement mesuré par chaque récepteur est rendu à
//! l'émetteur d'en face *hors bande* (dans un vrai modem, ~200 bits à
//! faire passer pendant la poignée de main), le retard aller est supposé
//! sous 150 ms, et aucune dérive d'horloge d'échantillonnage n'est suivie
//! (le simulateur de ligne n'en a pas). Mesures : `examples/orgue_mesure.rs`.

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
    /// Petits pas de phase seulement : timbre stable, raies nettes
    Joli,
    /// MAQ : bien plus de bits, son soufflé
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
