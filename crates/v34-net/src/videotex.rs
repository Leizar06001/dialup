//! Vidéotex (Télétel, Minitel 1 : le « STUM1B ») : les codes, les accents
//! du jeu G2, les touches de fonction du clavier, et un écran de 40 × 25
//! qui interprète le flot reçu. L'appli s'en sert pour afficher le Minitel,
//! le central pour vérifier ses pages dans les tests.
//!
//! Rappels :
//! - rangée 0 : la ligne de service (le « F » / « C » de la connexion) ;
//!   rangées 1 à 24 : l'écran, 40 colonnes ;
//! - `US rangée colonne` positionne le curseur (0x40 + rangée, 0x40 + colonne) ;
//! - `ESC` + une lettre change les attributs : couleur des caractères
//!   (0x40 à 0x47), du fond (0x50 à 0x57, pris en compte au prochain espace),
//!   taille (0x4C à 0x4F), clignotement (0x48/0x49), inversion (0x5C/0x5D) ;
//! - `SO` passe aux mosaïques (G1, des pavés de 2 × 3), `SI` revient au texte ;
//! - `SS2` + accent + lettre : les lettres accentuées (é = SS2 0x42 e) ;
//! - `REP n` répète le dernier caractère n fois ;
//! - le clavier envoie ses touches de fonction précédées de `SEP` (0x13).
//!
//! Sur un Minitel 1, les couleurs sont des niveaux de gris : du plus sombre
//! au plus clair, noir, bleu, rouge, magenta, vert, cyan, jaune, blanc.

use serde::Serialize;

pub const BEL: u8 = 0x07;
pub const BS: u8 = 0x08;
pub const HT: u8 = 0x09;
pub const LF: u8 = 0x0A;
pub const VT: u8 = 0x0B;
pub const FF: u8 = 0x0C;
pub const CR: u8 = 0x0D;
pub const SO: u8 = 0x0E;
pub const SI: u8 = 0x0F;
/// Curseur visible
pub const CON: u8 = 0x11;
pub const REP: u8 = 0x12;
/// Touches de fonction du clavier
pub const SEP: u8 = 0x13;
/// Curseur invisible
pub const COFF: u8 = 0x14;
/// Efface jusqu'à la fin de la rangée
pub const CAN: u8 = 0x18;
pub const SS2: u8 = 0x19;
/// Caractère abîmé (parité fausse)
pub const SUB: u8 = 0x1A;
pub const ESC: u8 = 0x1B;
/// Retour en haut à gauche
pub const RS: u8 = 0x1E;
/// Positionnement du curseur
pub const US: u8 = 0x1F;

/// Couleurs, dans l'ordre du Vidéotex.
pub const NOIR: u8 = 0;
pub const ROUGE: u8 = 1;
pub const VERT: u8 = 2;
pub const JAUNE: u8 = 3;
pub const BLEU: u8 = 4;
pub const MAGENTA: u8 = 5;
pub const CYAN: u8 = 6;
pub const BLANC: u8 = 7;

/// `PRO2 START ROULEAU` : l'écran défile au lieu de revenir en haut.
pub const ROULEAU_DEBUT: [u8; 4] = [ESC, 0x3A, 0x69, 0x43];
pub const ROULEAU_FIN: [u8; 4] = [ESC, 0x3A, 0x6A, 0x43];

/// Les touches de fonction du Minitel (SEP puis 0x41 à 0x49).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, serde::Deserialize)]
pub enum Touche {
    Envoi,
    Retour,
    Repetition,
    Guide,
    Annulation,
    Sommaire,
    Correction,
    Suite,
    ConnexionFin,
}

impl Touche {
    pub const TOUTES: [Touche; 9] =
        [Touche::Envoi, Touche::Retour, Touche::Repetition, Touche::Guide, Touche::Annulation, Touche::Sommaire, Touche::Correction, Touche::Suite, Touche::ConnexionFin];

    pub fn code(self) -> u8 {
        0x41 + Touche::TOUTES.iter().position(|t| *t == self).unwrap_or(0) as u8
    }

    pub fn depuis_code(c: u8) -> Option<Touche> {
        Touche::TOUTES.get(c.checked_sub(0x41)? as usize).copied()
    }

    /// Ce qu'envoie le clavier.
    pub fn octets(self) -> [u8; 2] {
        [SEP, self.code()]
    }

    /// Le nom gravé sur la touche.
    pub fn nom(self) -> &'static str {
        match self {
            Touche::Envoi => "ENVOI",
            Touche::Retour => "RETOUR",
            Touche::Repetition => "RÉPÉTITION",
            Touche::Guide => "GUIDE",
            Touche::Annulation => "ANNULATION",
            Touche::Sommaire => "SOMMAIRE",
            Touche::Correction => "CORRECTION",
            Touche::Suite => "SUITE",
            Touche::ConnexionFin => "CONNEXION/FIN",
        }
    }

    /// Depuis son nom (sans accents ni casse : « envoi », « connexion/fin »…).
    pub fn depuis_nom(n: &str) -> Option<Touche> {
        let n: String = n.chars().filter(|c| c.is_alphanumeric()).map(sans_accent).collect::<String>().to_lowercase();
        Touche::TOUTES.into_iter().find(|t| t.nom().chars().filter(|c| c.is_alphanumeric()).map(sans_accent).collect::<String>().to_lowercase() == n)
    }
}

// ───────────────────────── accents (G2) ─────────────────────────

/// Accents du jeu G2 (SS2 + accent + lettre).
const GRAVE: u8 = 0x41;
const AIGU: u8 = 0x42;
const CIRCONFLEXE: u8 = 0x43;
const TREMA: u8 = 0x48;
const CEDILLE: u8 = 0x4B;

/// Lettres accentuées que le Minitel sait afficher : (lettre, accent, base).
const COMPOSEES: &[(char, u8, char)] = &[
    ('à', GRAVE, 'a'),
    ('è', GRAVE, 'e'),
    ('ù', GRAVE, 'u'),
    ('é', AIGU, 'e'),
    ('â', CIRCONFLEXE, 'a'),
    ('ê', CIRCONFLEXE, 'e'),
    ('î', CIRCONFLEXE, 'i'),
    ('ô', CIRCONFLEXE, 'o'),
    ('û', CIRCONFLEXE, 'u'),
    ('ä', TREMA, 'a'),
    ('ë', TREMA, 'e'),
    ('ï', TREMA, 'i'),
    ('ö', TREMA, 'o'),
    ('ü', TREMA, 'u'),
    ('ç', CEDILLE, 'c'),
];

/// Signes du jeu G2 (SS2 + code).
const SIGNES: &[(char, u8)] = &[
    ('£', 0x23),
    ('§', 0x27),
    ('←', 0x2C),
    ('↑', 0x2D),
    ('→', 0x2E),
    ('↓', 0x2F),
    ('°', 0x30),
    ('±', 0x31),
    ('÷', 0x38),
    ('¼', 0x3C),
    ('½', 0x3D),
    ('¾', 0x3E),
    ('Œ', 0x6A),
    ('œ', 0x7A),
    ('ß', 0x7B),
];

/// La lettre sans son accent (les majuscules accentuées n'existent pas sur
/// le Minitel : « É » s'écrit « E »).
pub fn sans_accent(c: char) -> char {
    match c {
        'à' | 'â' | 'ä' | 'á' | 'ã' | 'å' => 'a',
        'À' | 'Â' | 'Ä' | 'Á' | 'Ã' | 'Å' => 'A',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'É' | 'È' | 'Ê' | 'Ë' => 'E',
        'î' | 'ï' | 'í' | 'ì' => 'i',
        'Î' | 'Ï' | 'Í' | 'Ì' => 'I',
        'ô' | 'ö' | 'ó' | 'ò' | 'õ' => 'o',
        'Ô' | 'Ö' | 'Ó' | 'Ò' | 'Õ' => 'O',
        'ù' | 'û' | 'ü' | 'ú' => 'u',
        'Ù' | 'Û' | 'Ü' | 'Ú' => 'U',
        'ç' => 'c',
        'Ç' => 'C',
        'ñ' => 'n',
        'Ñ' => 'N',
        'ÿ' | 'ý' => 'y',
        c => c,
    }
}

/// Un caractère en octets Vidéotex (texte, jeu G0 ou G2). Ce que le
/// Minitel ne sait pas afficher est remplacé au mieux.
pub fn encoder_car(c: char, out: &mut Vec<u8>) {
    if let Some(&(_, accent, base)) = COMPOSEES.iter().find(|(l, _, _)| *l == c) {
        out.extend([SS2, accent, base as u8]);
        return;
    }
    if let Some(&(_, code)) = SIGNES.iter().find(|(s, _)| *s == c) {
        out.extend([SS2, code]);
        return;
    }
    let remplace: &str = match c {
        ' '..='~' => {
            out.push(c as u8);
            return;
        }
        '\u{A0}' | '\u{202F}' | '\u{2009}' | '\t' => " ",
        '«' | '»' | '“' | '”' | '„' => "\"",
        '‘' | '’' | '‚' | '′' => "'",
        '…' => "...",
        '–' | '—' | '−' | '‐' | '·' | '•' => "-",
        '★' | '☆' | '✶' => "*",
        '×' => "x",
        '€' => "EUR",
        'æ' => "ae",
        'Æ' => "AE",
        '©' => "(c)",
        '®' => "(R)",
        _ => {
            let s = sans_accent(c);
            if s != c && s.is_ascii() {
                out.push(s as u8);
            } else if !c.is_control() {
                out.push(b'?');
            }
            return;
        }
    };
    out.extend(remplace.bytes());
}

/// Du texte en octets Vidéotex.
pub fn encoder(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() + 8);
    for c in s.chars() {
        encoder_car(c, &mut out);
    }
    out
}

/// Largeur à l'écran d'un texte (une colonne par caractère, accent compris).
pub fn largeur(s: &str) -> usize {
    s.chars()
        .map(|c| match c {
            '…' | '€' => 3,
            'æ' | 'Æ' => 2,
            '©' | '®' => 3,
            _ => 1,
        })
        .sum()
}

fn compose(accent: u8, base: u8) -> char {
    COMPOSEES.iter().find(|(_, a, b)| *a == accent && *b as u8 == base).map_or(base as char, |(l, _, _)| *l)
}

/// Signe du jeu G2 (SS2 + code).
pub fn signe(code: u8) -> char {
    match code {
        0x24 => '$',
        0x26 => '#',
        _ => SIGNES.iter().find(|(_, c)| *c == code).map_or('?', |(s, _)| *s),
    }
}

/// Code G2 d'un accent (pour les claviers qui envoient SS2 + accent + lettre).
pub fn est_accent(code: u8) -> bool {
    (0x41..=0x4F).contains(&code)
}

/// Lettre accentuée composée (SS2 accent base), ou la base seule.
pub fn composer(accent: u8, base: u8) -> char {
    compose(accent, base)
}

// ───────────────────────── l'écran ─────────────────────────

pub const COLONNES: usize = 40;
pub const RANGEES: usize = 25;

/// Attributs d'une cellule (bits de `Cellule::attr`).
pub const A_MOSAIQUE: u16 = 1;
pub const A_CLIGNOTE: u16 = 2;
pub const A_INVERSE: u16 = 4;
/// Texte souligné ; mosaïque disjointe
pub const A_SOULIGNE: u16 = 8;
pub const A_DOUBLE_HAUTEUR: u16 = 16;
pub const A_DOUBLE_LARGEUR: u16 = 32;
/// Moitié haute d'un caractère en double hauteur
pub const A_HAUT: u16 = 64;
/// Moitié droite d'un caractère en double largeur
pub const A_DROITE: u16 = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Cellule {
    /// Caractère affiché ; pour une mosaïque, son code (0x20 à 0x7F)
    pub c: char,
    pub fg: u8,
    pub bg: u8,
    pub attr: u16,
}

impl Cellule {
    const VIDE: Cellule = Cellule { c: ' ', fg: BLANC, bg: NOIR, attr: 0 };

    /// Couleurs et attributs en un nombre : fg | bg << 3 | attr << 6.
    pub fn code_attributs(&self) -> u32 {
        self.fg as u32 | (self.bg as u32) << 3 | (self.attr as u32) << 6
    }
}

#[derive(Clone, Copy, Debug)]
enum Etat {
    Normal,
    Esc,
    /// Séquence de protocole : octets encore à avaler (et les deux premiers)
    Pro { reste: u8, a: u8 },
    /// ESC [ … : jusqu'à l'octet final
    Csi,
    Us { rangee: Option<u8> },
    Rep,
    Ss2,
    Accent(u8),
}

/// Un écran de Minitel : ce qu'affiche le flot Vidéotex reçu.
#[derive(Clone, Debug)]
pub struct Ecran {
    cellules: Vec<Cellule>,
    /// Curseur : rangée 0..24, colonne 0..39
    pub rangee: usize,
    pub colonne: usize,
    pub curseur_visible: bool,
    // attributs courants
    fg: u8,
    bg: u8,
    bg_attente: Option<u8>,
    attr: u16,
    mosaique: bool,
    etat: Etat,
    dernier: Option<(char, bool)>,
    rouleau: bool,
    /// Où revenir en quittant la rangée 0
    avant_rangee0: Option<(usize, usize)>,
    /// Rangées modifiées (bits)
    sales: u32,
    /// BEL reçus (le Minitel bipe)
    pub bips: u32,
    connecte: bool,
}

impl Default for Ecran {
    fn default() -> Self {
        Self::new()
    }
}

impl Ecran {
    pub fn new() -> Ecran {
        let mut e = Ecran {
            cellules: vec![Cellule::VIDE; COLONNES * RANGEES],
            rangee: 1,
            colonne: 0,
            curseur_visible: false,
            fg: BLANC,
            bg: NOIR,
            bg_attente: None,
            attr: 0,
            mosaique: false,
            etat: Etat::Normal,
            dernier: None,
            rouleau: false,
            avant_rangee0: None,
            sales: u32::MAX,
            bips: 0,
            connecte: false,
        };
        e.indicateur();
        e
    }

    pub fn cellule(&self, rangee: usize, colonne: usize) -> Cellule {
        self.cellules[rangee * COLONNES + colonne]
    }

    /// Le « C » (connecté) ou le « F » (fin) en haut à droite.
    pub fn connexion(&mut self, connecte: bool) {
        self.connecte = connecte;
        self.indicateur();
    }

    fn indicateur(&mut self) {
        self.cellules[COLONNES - 1] = Cellule { c: if self.connecte { 'C' } else { 'F' }, fg: BLANC, bg: NOIR, attr: 0 };
        self.sales |= 1;
    }

    /// Texte d'une rangée (mosaïques en espaces), sans les espaces de la fin.
    pub fn texte_rangee(&self, r: usize) -> String {
        let mut s: String = (0..COLONNES)
            .map(|c| {
                let x = self.cellule(r, c);
                // (mosaïques et moitiés droites des doubles largeurs : des blancs)
                if x.attr & (A_MOSAIQUE | A_DROITE) != 0 {
                    ' '
                } else {
                    x.c
                }
            })
            .collect();
        s.truncate(s.trim_end().len());
        s
    }

    /// Tout l'écran en texte (rangées 1 à 24).
    pub fn texte(&self) -> String {
        (1..RANGEES).map(|r| self.texte_rangee(r)).collect::<Vec<_>>().join("\n")
    }

    /// Rangées modifiées depuis le dernier appel.
    pub fn prendre_sales(&mut self) -> Vec<usize> {
        let s = std::mem::take(&mut self.sales);
        (0..RANGEES).filter(|r| s & (1 << r) != 0).collect()
    }

    /// Une rangée pour l'affichage : ses 40 caractères, et les attributs de
    /// chacun ([`Cellule::code_attributs`]).
    pub fn rangee_compacte(&self, r: usize) -> (String, Vec<u32>) {
        let cs = &self.cellules[r * COLONNES..(r + 1) * COLONNES];
        (cs.iter().map(|c| c.c).collect(), cs.iter().map(Cellule::code_attributs).collect())
    }

    fn reinit_attributs(&mut self) {
        self.fg = BLANC;
        self.bg = NOIR;
        self.bg_attente = None;
        self.attr = 0;
        self.mosaique = false;
    }

    fn effacer(&mut self, de: usize, a: usize, bg: u8) {
        for c in &mut self.cellules[de..a] {
            *c = Cellule { bg, ..Cellule::VIDE };
        }
        for r in de / COLONNES..=(a.saturating_sub(1)) / COLONNES {
            self.sales |= 1 << r;
        }
    }

    fn defiler(&mut self, haut: bool) {
        let (a, b) = (COLONNES, COLONNES * RANGEES);
        if haut {
            self.cellules.copy_within(a + COLONNES..b, a);
            self.effacer(b - COLONNES, b, NOIR);
        } else {
            self.cellules.copy_within(a..b - COLONNES, a + COLONNES);
            self.effacer(a, a + COLONNES, NOIR);
        }
        self.sales |= !1;
    }

    fn descendre(&mut self) {
        if self.rangee == 0 {
            if let Some((r, c)) = self.avant_rangee0.take() {
                self.rangee = r;
                self.colonne = c;
                self.reinit_attributs();
            } else {
                self.rangee = 1;
            }
            return;
        }
        if self.rangee >= RANGEES - 1 {
            if self.rouleau {
                self.defiler(true);
            } else {
                self.rangee = 1;
            }
        } else {
            self.rangee += 1;
        }
    }

    fn monter(&mut self) {
        if self.rangee <= 1 {
            if self.rouleau {
                self.defiler(false);
            } else if self.rangee == 1 {
                self.rangee = RANGEES - 1;
            }
        } else {
            self.rangee -= 1;
        }
    }

    fn avancer(&mut self, n: usize) {
        self.colonne += n;
        if self.colonne >= COLONNES {
            self.colonne = 0;
            if self.rangee == 0 {
                // la ligne de service ne déborde pas
                self.colonne = COLONNES - 1;
            } else {
                self.descendre();
            }
        }
    }

    fn poser(&mut self, r: usize, c: usize, cel: Cellule) {
        if r < RANGEES && c < COLONNES {
            self.cellules[r * COLONNES + c] = cel;
            self.sales |= 1 << r;
        }
    }

    /// Affiche un caractère à la position du curseur.
    fn afficher(&mut self, ch: char, mosaique: bool) {
        // le fond attendu est validé par un délimiteur (espace ou mosaïque)
        if (ch == ' ' || mosaique) && self.bg_attente.is_some() {
            self.bg = self.bg_attente.take().unwrap_or(NOIR);
        }
        let mut attr = self.attr & (A_CLIGNOTE | A_INVERSE | A_SOULIGNE);
        if mosaique {
            attr |= A_MOSAIQUE;
        } else {
            attr |= self.attr & (A_DOUBLE_HAUTEUR | A_DOUBLE_LARGEUR);
        }
        let (r, c) = (self.rangee, self.colonne);
        let dh = attr & A_DOUBLE_HAUTEUR != 0 && r >= 2;
        let dl = attr & A_DOUBLE_LARGEUR != 0 && c + 1 < COLONNES;
        if !dh {
            attr &= !A_DOUBLE_HAUTEUR;
        }
        if !dl {
            attr &= !A_DOUBLE_LARGEUR;
        }
        let cel = Cellule { c: ch, fg: self.fg, bg: self.bg, attr };
        self.poser(r, c, cel);
        if dl {
            self.poser(r, c + 1, Cellule { attr: attr | A_DROITE, ..cel });
        }
        if dh {
            self.poser(r - 1, c, Cellule { attr: attr | A_HAUT, ..cel });
            if dl {
                self.poser(r - 1, c + 1, Cellule { attr: attr | A_HAUT | A_DROITE, ..cel });
            }
        }
        self.dernier = Some((ch, mosaique));
        self.avancer(if dl { 2 } else { 1 });
    }

    /// Un octet G0 ou G1 imprimable.
    fn imprimable(&mut self, b: u8) {
        if self.mosaique && !(0x40..=0x5F).contains(&b) {
            self.afficher(b as char, true);
            return;
        }
        let ch = match b {
            0x7F => '█',
            _ => b as char,
        };
        self.afficher(ch, false);
    }

    /// Interprète des octets reçus.
    pub fn recevoir(&mut self, octets: &[u8]) {
        for &b in octets {
            self.octet(b & 0x7F);
        }
    }

    fn octet(&mut self, b: u8) {
        match self.etat {
            Etat::Normal => self.normal(b),
            Etat::Esc => {
                self.etat = Etat::Normal;
                self.esc(b);
            }
            Etat::Pro { reste, a } => {
                let a = if a == 0 { b } else { a };
                if reste <= 1 {
                    self.etat = Etat::Normal;
                    // PRO2 START/STOP ROULEAU
                    if b == 0x43 && a == 0x69 {
                        self.rouleau = true;
                    } else if b == 0x43 && a == 0x6A {
                        self.rouleau = false;
                    }
                } else {
                    self.etat = Etat::Pro { reste: reste - 1, a };
                }
            }
            Etat::Csi => {
                if (0x40..=0x7E).contains(&b) {
                    self.etat = Etat::Normal;
                }
            }
            Etat::Us { rangee: None } => {
                if b < 0x20 {
                    // une commande au milieu : on abandonne le positionnement
                    self.etat = Etat::Normal;
                    self.normal(b);
                } else {
                    self.etat = Etat::Us { rangee: Some(b) };
                }
            }
            Etat::Us { rangee: Some(r) } => {
                self.etat = Etat::Normal;
                let r = (r as usize).wrapping_sub(0x40);
                let c = (b as usize).wrapping_sub(0x41);
                if r < RANGEES && c < COLONNES {
                    if r == 0 && self.rangee != 0 {
                        self.avant_rangee0 = Some((self.rangee, self.colonne));
                    }
                    self.rangee = r;
                    self.colonne = c;
                }
                self.reinit_attributs();
            }
            Etat::Rep => {
                self.etat = Etat::Normal;
                let n = b.saturating_sub(0x40) as usize;
                if let Some((ch, m)) = self.dernier {
                    for _ in 0..n {
                        self.afficher(ch, m);
                    }
                }
            }
            Etat::Ss2 => {
                if est_accent(b) {
                    self.etat = Etat::Accent(b);
                } else {
                    self.etat = Etat::Normal;
                    if b >= 0x20 {
                        self.afficher(signe(b), false);
                    }
                }
            }
            Etat::Accent(a) => {
                self.etat = Etat::Normal;
                if b >= 0x20 {
                    self.afficher(compose(a, b), false);
                }
            }
        }
    }

    fn normal(&mut self, b: u8) {
        match b {
            BEL => self.bips += 1,
            BS => {
                if self.colonne > 0 {
                    self.colonne -= 1;
                } else if self.rangee > 1 {
                    self.colonne = COLONNES - 1;
                    self.rangee -= 1;
                }
            }
            HT => self.avancer(1),
            LF => self.descendre(),
            VT => self.monter(),
            FF => {
                self.effacer(COLONNES, COLONNES * RANGEES, NOIR);
                self.rangee = 1;
                self.colonne = 0;
                self.reinit_attributs();
                self.dernier = None;
            }
            CR => self.colonne = 0,
            SO => {
                self.mosaique = true;
                self.attr &= !(A_DOUBLE_HAUTEUR | A_DOUBLE_LARGEUR | A_SOULIGNE);
            }
            SI => {
                self.mosaique = false;
                self.attr &= !A_SOULIGNE;
            }
            CON => self.curseur_visible = true,
            COFF => self.curseur_visible = false,
            REP => self.etat = Etat::Rep,
            CAN => {
                let i = self.rangee * COLONNES + self.colonne;
                let bg = self.bg;
                self.effacer(i, (self.rangee + 1) * COLONNES, bg);
                if self.rangee == 0 {
                    self.indicateur();
                }
            }
            SS2 => self.etat = Etat::Ss2,
            SUB => self.afficher('▒', false),
            ESC => self.etat = Etat::Esc,
            RS => {
                self.rangee = 1;
                self.colonne = 0;
                self.reinit_attributs();
            }
            US => self.etat = Etat::Us { rangee: None },
            0x20..=0x7F => self.imprimable(b),
            _ => {}
        }
    }

    fn esc(&mut self, b: u8) {
        match b {
            0x40..=0x47 => self.fg = b - 0x40,
            0x50..=0x57 => {
                if self.mosaique {
                    self.bg = b - 0x50;
                } else {
                    self.bg_attente = Some(b - 0x50);
                }
            }
            0x48 => self.attr |= A_CLIGNOTE,
            0x49 => self.attr &= !A_CLIGNOTE,
            0x4C..=0x4F if !self.mosaique => {
                self.attr &= !(A_DOUBLE_HAUTEUR | A_DOUBLE_LARGEUR);
                if b == 0x4D || b == 0x4F {
                    self.attr |= A_DOUBLE_HAUTEUR;
                }
                if b == 0x4E || b == 0x4F {
                    self.attr |= A_DOUBLE_LARGEUR;
                }
            }
            0x59 => self.attr &= !A_SOULIGNE,
            0x5A => self.attr |= A_SOULIGNE,
            0x5C => self.attr &= !A_INVERSE,
            0x5D => self.attr |= A_INVERSE,
            // PRO1, PRO2, PRO3 : 1, 2 ou 3 octets à suivre
            0x39 => self.etat = Etat::Pro { reste: 1, a: 0 },
            0x3A => self.etat = Etat::Pro { reste: 2, a: 0 },
            0x3B => self.etat = Etat::Pro { reste: 3, a: 0 },
            0x23 => self.etat = Etat::Pro { reste: 2, a: 0 },
            0x5B => self.etat = Etat::Csi,
            _ => {}
        }
    }
}

/// Le texte d'un flot Vidéotex, pour les tests et les journaux : ce qui
/// s'affiche, accents recomposés, mosaïques et attributs omis ; chaque
/// positionnement ailleurs qu'à la suite de ce qui vient de s'écrire,
/// effacement ou changement de rangée devient un saut de ligne (jamais deux
/// de suite).
pub fn transcrire(octets: &[u8]) -> String {
    let mut s = String::new();
    let mut i = 0;
    let mut mosaique = false;
    let mut dernier: Option<char> = None;
    // où en est le curseur (rangée, colonne), la largeur des caractères, et
    // où s'est arrêté le dernier texte écrit
    let (mut r, mut c, mut largeur) = (1u8, 1u8, 1u8);
    let mut fin_texte = (1u8, 1u8);
    let mut saut_attendu = false;
    let saut = |s: &mut String| {
        if !s.is_empty() && !s.ends_with('\n') {
            s.push('\n');
        }
    };
    let ecrire = |s: &mut String, ch: char, r: u8, c: &mut u8, largeur: u8, saut_attendu: &mut bool, fin_texte: &mut (u8, u8)| {
        if std::mem::take(saut_attendu) && !s.is_empty() && !s.ends_with('\n') {
            s.push('\n');
        }
        s.push(ch);
        *c = c.saturating_add(largeur);
        *fin_texte = (r, *c);
    };
    while i < octets.len() {
        let b = octets[i] & 0x7F;
        i += 1;
        match b {
            US => {
                let (r2, c2) = (octets.get(i).copied().unwrap_or(0x40).wrapping_sub(0x40), octets.get(i + 1).copied().unwrap_or(0x41).wrapping_sub(0x40));
                i += 2;
                // (un positionnement juste après le dernier texte écrit continue
                // la ligne, même si le curseur est passé ailleurs entre-temps)
                saut_attendu = (r2, c2) != fin_texte;
                (r, c, largeur) = (r2, c2, 1);
                mosaique = false;
            }
            FF | RS => {
                saut(&mut s);
                (r, c, largeur) = (1, 1, 1);
                mosaique = false;
            }
            LF | CR | VT => {
                saut(&mut s);
                r = r.wrapping_add(1);
                c = 1;
            }
            ESC => {
                let n = octets.get(i).copied().unwrap_or(0);
                match n {
                    0x4C | 0x4D => largeur = 1,
                    0x4E | 0x4F => largeur = 2,
                    _ => {}
                }
                i += match n {
                    0x39 => 2,
                    0x3A | 0x23 => 3,
                    0x3B => 4,
                    _ => 1,
                };
            }
            SO => mosaique = true,
            SI => mosaique = false,
            REP => {
                let n = octets.get(i).copied().unwrap_or(0x40).saturating_sub(0x40);
                i += 1;
                if let Some(ch) = dernier {
                    for _ in 0..n {
                        ecrire(&mut s, ch, r, &mut c, largeur, &mut saut_attendu, &mut fin_texte);
                    }
                }
            }
            SS2 => {
                let a = octets.get(i).copied().unwrap_or(0);
                let ch = if est_accent(a) {
                    let base = octets.get(i + 1).copied().unwrap_or(b' ');
                    i += 2;
                    compose(a, base)
                } else {
                    i += 1;
                    signe(a)
                };
                ecrire(&mut s, ch, r, &mut c, largeur, &mut saut_attendu, &mut fin_texte);
                dernier = Some(ch);
            }
            0x20..=0x7F => {
                let ch = if mosaique && !(0x40..=0x5F).contains(&b) {
                    ' '
                } else if b == 0x7F {
                    '█'
                } else {
                    b as char
                };
                ecrire(&mut s, ch, r, &mut c, if mosaique { 1 } else { largeur }, &mut saut_attendu, &mut fin_texte);
                dernier = Some(ch);
            }
            _ => {}
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accents_et_signes() {
        assert_eq!(encoder("Été à 2,23 F · œuf °"), b"Et\x19\x42e \x19\x41a 2,23 F - \x19\x7auf \x19\x30".to_vec());
        assert_eq!(encoder("ça déçoit"), b"\x19\x4bca d\x19\x42e\x19\x4bcoit".to_vec());
        assert_eq!(transcrire(&encoder("Noël, où êtes-vous ? œ « » …")), "Noël, où êtes-vous ? œ \" \" ...");
        assert_eq!(largeur("été…"), 6);
    }

    #[test]
    fn touches() {
        assert_eq!(Touche::Envoi.octets(), [SEP, 0x41]);
        assert_eq!(Touche::ConnexionFin.code(), 0x49);
        assert_eq!(Touche::depuis_code(0x48), Some(Touche::Suite));
        assert_eq!(Touche::depuis_nom("répétition"), Some(Touche::Repetition));
        assert_eq!(Touche::depuis_nom("Connexion/Fin"), Some(Touche::ConnexionFin));
        assert_eq!(Touche::depuis_code(0x4A), None);
    }

    #[test]
    fn page_positionnement_et_attributs() {
        let mut e = Ecran::new();
        let mut p = vec![FF, US, 0x43, 0x45, ESC, 0x4D, ESC, 0x46];
        p.extend(b"3615");
        p.extend([US, 0x45, 0x41, ESC, 0x54]);
        p.extend(b" fond bleu");
        p.extend([US, 0x46, 0x41, b'x', REP, 0x43]);
        e.recevoir(&p);
        assert_eq!(e.texte_rangee(3), "    3615");
        // double hauteur : le haut du caractère sur la rangée du dessus
        assert_eq!(e.cellule(2, 4).c, '3');
        assert_ne!(e.cellule(2, 4).attr & A_HAUT, 0);
        assert_eq!(e.cellule(3, 4).fg, CYAN);
        // le fond bleu n'arrive qu'avec l'espace (délimiteur)
        assert_eq!(e.cellule(5, 0).bg, BLEU);
        assert_eq!(e.texte_rangee(5), " fond bleu");
        assert_eq!(e.texte_rangee(6), "xxxx");
        assert_eq!(e.cellule(0, 39).c, 'F');
        e.connexion(true);
        assert_eq!(e.cellule(0, 39).c, 'C');
    }

    #[test]
    fn mosaiques_rouleau_et_correction() {
        let mut e = Ecran::new();
        e.recevoir(&[FF, SO, 0x7F, 0x21, SI, b'A']);
        assert_eq!(e.cellule(1, 0).attr & A_MOSAIQUE, A_MOSAIQUE);
        assert_eq!(e.cellule(1, 2).c, 'A');
        // CAN efface la fin de la rangée
        e.recevoir(&[US, 0x41, 0x42, CAN]);
        assert_eq!(e.texte_rangee(1), "");
        // en rouleau, le bas de l'écran fait défiler
        e.recevoir(&ROULEAU_DEBUT);
        e.recevoir(&[US, 0x58, 0x41]);
        e.recevoir(b"derniere\n\rencore");
        assert_eq!(e.texte_rangee(23), "derniere");
        assert_eq!(e.texte_rangee(24), "encore");
        // un caractère abîmé
        e.recevoir(&[SUB]);
        assert_eq!(e.texte_rangee(24), "encore▒");
    }

    #[test]
    fn transcription() {
        let mut p = vec![FF];
        p.extend(encoder("Bienvenue, gérard."));
        p.extend([US, 0x45, 0x41, ESC, 0x4D]);
        p.extend(encoder("Posez votre question"));
        p.extend([SO, 0x7F, 0x7F, SI]);
        assert_eq!(transcrire(&p), "Bienvenue, gérard.\nPosez votre question  ");
    }
}
