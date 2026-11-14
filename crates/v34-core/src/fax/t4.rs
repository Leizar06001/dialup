//! Codage des pages de fax : T.4, codage unidimensionnel « Huffman modifié »
//! (MH), celui de tous les télécopieurs du groupe 3.
//!
//! Une ligne fait 1728 points (215 mm à 8 points par mm). Elle est codée
//! comme une suite de plages alternées blanches et noires, en commençant par
//! le blanc (une plage blanche de longueur nulle si la ligne commence par du
//! noir). Chaque plage : un mot « de complément » (multiple de 64) si besoin,
//! puis un mot « de terminaison » (0 à 63). Chaque ligne est suivie de EOL
//! (000000000001) ; la page commence par un EOL et finit par RTC (six EOL).
//!
//! Les mots sont émis dans l'ordre où ils sont écrits (premier bit à
//! gauche d'abord). Tables : T.4, tableaux 2 et 3.

/// Points par ligne (A4, 8 points/mm)
pub const LARGEUR: usize = 1728;
/// EOL : onze 0 puis un 1
const EOL: &str = "000000000001";
/// RTC : six EOL de suite
const RTC_EOL: usize = 6;

/// Mots de terminaison blancs, plages 0 à 63.
const BLANC_TERM: [&str; 64] = [
    "00110101", "000111", "0111", "1000", "1011", "1100", "1110", "1111", "10011", "10100", "00111", "01000", "001000", "000011", "110100", "110101", "101010",
    "101011", "0100111", "0001100", "0001000", "0010111", "0000011", "0000100", "0101000", "0101011", "0010011", "0100100", "0011000", "00000010", "00000011",
    "00011010", "00011011", "00010010", "00010011", "00010100", "00010101", "00010110", "00010111", "00101000", "00101001", "00101010", "00101011", "00101100",
    "00101101", "00000100", "00000101", "00001010", "00001011", "01010010", "01010011", "01010100", "01010101", "00100100", "00100101", "01011000", "01011001",
    "01011010", "01011011", "01001010", "01001011", "00110010", "00110011", "00110100",
];

/// Mots de terminaison noirs, plages 0 à 63.
const NOIR_TERM: [&str; 64] = [
    "0000110111", "010", "11", "10", "011", "0011", "0010", "00011", "000101", "000100", "0000100", "0000101", "0000111", "00000100", "00000111", "000011000",
    "0000010111", "0000011000", "0000001000", "00001100111", "00001101000", "00001101100", "00000110111", "00000101000", "00000010111", "00000011000",
    "000011001010", "000011001011", "000011001100", "000011001101", "000001101000", "000001101001", "000001101010", "000001101011", "000011010010",
    "000011010011", "000011010100", "000011010101", "000011010110", "000011010111", "000001101100", "000001101101", "000011011010", "000011011011",
    "000001010100", "000001010101", "000001010110", "000001010111", "000001100100", "000001100101", "000001010010", "000001010011", "000000100100",
    "000000110111", "000000111000", "000000100111", "000000101000", "000001011000", "000001011001", "000000101011", "000000101100", "000001011010",
    "000001100110", "000001100111",
];

/// Mots de complément blancs, plages 64, 128 … 1728.
const BLANC_COMPL: [&str; 27] = [
    "11011", "10010", "010111", "0110111", "00110110", "00110111", "01100100", "01100101", "01101000", "01100111", "011001100", "011001101", "011010010",
    "011010011", "011010100", "011010101", "011010110", "011010111", "011011000", "011011001", "011011010", "011011011", "010011000", "010011001", "010011010",
    "011000", "010011011",
];

/// Mots de complément noirs, plages 64, 128 … 1728.
const NOIR_COMPL: [&str; 27] = [
    "0000001111", "000011001000", "000011001001", "000001011011", "000000110011", "000000110100", "000000110101", "0000001101100", "0000001101101",
    "0000001001010", "0000001001011", "0000001001100", "0000001001101", "0000001110010", "0000001110011", "0000001110100", "0000001110101", "0000001110110",
    "0000001110111", "0000001010010", "0000001010011", "0000001010100", "0000001010101", "0000001011010", "0000001011011", "0000001100100", "0000001100101",
];

fn pousser(bits: &mut Vec<bool>, mot: &str) {
    bits.extend(mot.bytes().map(|b| b == b'1'));
}

fn coder_plage(bits: &mut Vec<bool>, noir: bool, mut n: usize) {
    let (term, compl) = if noir { (&NOIR_TERM, &NOIR_COMPL) } else { (&BLANC_TERM, &BLANC_COMPL) };
    while n >= 64 {
        let m = (n / 64).min(27);
        pousser(bits, compl[m - 1]);
        n -= m * 64;
    }
    pousser(bits, term[n]);
}

/// Code une ligne (vrai = noir), sans EOL. La ligne est tronquée ou complétée
/// de blanc à 1728 points.
pub fn coder_ligne(ligne: &[bool], bits: &mut Vec<bool>) {
    let mut noir = false;
    let mut i = 0;
    while i < LARGEUR {
        let mut n = 0;
        while i + n < LARGEUR && ligne.get(i + n).copied().unwrap_or(false) == noir {
            n += 1;
        }
        coder_plage(bits, noir, n);
        i += n;
        noir = !noir;
    }
}

/// Code une page entière : EOL, puis chaque ligne suivie d'un EOL, puis RTC.
pub fn coder_page(lignes: &[Vec<bool>]) -> Vec<bool> {
    let mut bits = Vec::new();
    pousser(&mut bits, EOL);
    for l in lignes {
        coder_ligne(l, &mut bits);
        pousser(&mut bits, EOL);
    }
    for _ in 0..RTC_EOL - 1 {
        pousser(&mut bits, EOL);
    }
    bits
}

// ───────────────────────── décodage ─────────────────────────

/// Arbre de décodage d'une couleur : nœuds (fils 0, fils 1) ; feuille = plage.
struct Arbre {
    noeuds: Vec<[i32; 2]>,
}

/// Valeur d'une feuille : plage (≥ 0) ; les nœuds internes sont des indices
/// positifs dans `noeuds`, les feuilles sont codées −(plage + 2), EOL = −1.
const FEUILLE_EOL: i32 = -1;

impl Arbre {
    fn new(term: &[&str; 64], compl: &[&str; 27]) -> Arbre {
        let mut a = Arbre { noeuds: vec![[0, 0]] };
        for (n, m) in term.iter().enumerate() {
            a.ajouter(m, -(n as i32 + 2));
        }
        for (k, m) in compl.iter().enumerate() {
            a.ajouter(m, -(((k + 1) * 64) as i32 + 2));
        }
        a.ajouter(EOL, FEUILLE_EOL);
        a
    }

    fn ajouter(&mut self, mot: &str, feuille: i32) {
        let mut n = 0usize;
        let b: Vec<usize> = mot.bytes().map(|c| (c - b'0') as usize).collect();
        for (i, &bit) in b.iter().enumerate() {
            if i + 1 == b.len() {
                assert_eq!(self.noeuds[n][bit], 0, "code T.4 ambigu : {mot}");
                self.noeuds[n][bit] = feuille;
            } else {
                let suivant = self.noeuds[n][bit];
                assert!(suivant >= 0, "code T.4 préfixe d'un autre : {mot}");
                if suivant == 0 {
                    self.noeuds.push([0, 0]);
                    let id = (self.noeuds.len() - 1) as i32;
                    self.noeuds[n][bit] = id;
                    n = id as usize;
                } else {
                    n = suivant as usize;
                }
            }
        }
    }
}

/// Ce que le décodeur rend.
#[derive(Clone, Debug, PartialEq)]
pub enum Sortie {
    /// Une ligne décodée (`abimee` : erreur de transmission, la ligne
    /// précédente a été recopiée à sa place, comme le font les télécopieurs)
    Ligne { pixels: Vec<bool>, abimee: bool },
    /// RTC : fin de page
    FinDePage,
}

/// Décodeur MH, bit par bit. Il attend le premier EOL, puis rend les lignes
/// au fil de l'eau. Après une erreur, il se resynchronise sur l'EOL suivant.
pub struct Decodeur {
    blanc: Arbre,
    noir: Arbre,
    /// Synchronisé (un EOL a été vu)
    synchro: bool,
    noeud: usize,
    noir_courant: bool,
    ligne: Vec<bool>,
    /// Plage en cours (compléments accumulés)
    plage: usize,
    /// En erreur : on attend l'EOL suivant (onze 0 puis un 1)
    zeros: u32,
    erreur: bool,
    /// EOL consécutifs sans ligne entre eux (6 = RTC)
    eol_suite: usize,
    precedente: Vec<bool>,
    fini: bool,
}

impl Default for Decodeur {
    fn default() -> Self {
        Decodeur {
            blanc: Arbre::new(&BLANC_TERM, &BLANC_COMPL),
            noir: Arbre::new(&NOIR_TERM, &NOIR_COMPL),
            synchro: false,
            noeud: 0,
            noir_courant: false,
            ligne: Vec::with_capacity(LARGEUR),
            plage: 0,
            zeros: 0,
            erreur: false,
            eol_suite: 0,
            precedente: vec![false; LARGEUR],
            fini: false,
        }
    }
}

impl Decodeur {
    pub fn fini(&self) -> bool {
        self.fini
    }

    /// Un bit de plus ; rend une ligne (ou la fin de page) quand il y en a une.
    pub fn bit(&mut self, b: bool) -> Option<Sortie> {
        if self.fini {
            return None;
        }
        // Recherche d'EOL (démarrage, ou après une erreur) : onze 0 au moins puis un 1
        if !self.synchro || self.erreur {
            if !b {
                self.zeros += 1;
                return None;
            }
            let eol = self.zeros >= 11;
            self.zeros = 0;
            if !eol {
                return None;
            }
            let rendu = if self.erreur {
                // la ligne abîmée est remplacée par la précédente
                self.erreur = false;
                Some(Sortie::Ligne { pixels: self.precedente.clone(), abimee: true })
            } else {
                None
            };
            self.synchro = true;
            self.debut_ligne();
            self.eol_suite = 1;
            return rendu;
        }
        // EOL, éventuellement précédé de 0 de bourrage : onze 0 au moins puis
        // un 1 (jamais plus de dix 0 de suite dans des données valides)
        if !b {
            self.zeros += 1;
        }
        if self.zeros >= 11 {
            self.noeud = 0;
            if b {
                self.zeros = 0;
                return self.fin_ligne();
            }
            return None;
        }
        if b {
            self.zeros = 0;
        }
        let arbre = if self.noir_courant { &self.noir } else { &self.blanc };
        let v = arbre.noeuds[self.noeud][b as usize];
        if v > 0 {
            self.noeud = v as usize;
            return None;
        }
        self.noeud = 0;
        if v == 0 {
            // code inexistant
            return self.erreur_ligne();
        }
        if v == FEUILLE_EOL {
            // (intercepté plus haut, par le compte des 0)
            return self.fin_ligne();
        }
        let n = (-v - 2) as usize;
        if n >= 64 {
            self.plage += n;
            return None;
        }
        let total = self.plage + n;
        self.plage = 0;
        if self.ligne.len() + total > LARGEUR {
            return self.erreur_ligne();
        }
        let c = self.noir_courant;
        self.ligne.extend(std::iter::repeat_n(c, total));
        self.noir_courant = !c;
        None
    }

    fn debut_ligne(&mut self) {
        self.ligne.clear();
        self.noir_courant = false;
        self.plage = 0;
        self.noeud = 0;
    }

    fn erreur_ligne(&mut self) -> Option<Sortie> {
        self.erreur = true;
        self.zeros = 0;
        self.eol_suite = 0;
        None
    }

    fn fin_ligne(&mut self) -> Option<Sortie> {
        if self.ligne.is_empty() && self.plage == 0 {
            // EOL juste après un EOL : RTC en cours
            self.eol_suite += 1;
            self.debut_ligne();
            if self.eol_suite >= RTC_EOL {
                self.fini = true;
                return Some(Sortie::FinDePage);
            }
            return None;
        }
        self.eol_suite = 1;
        let complete = self.ligne.len() == LARGEUR;
        let pixels = if complete { std::mem::take(&mut self.ligne) } else { self.precedente.clone() };
        if complete {
            self.precedente = pixels.clone();
        }
        self.debut_ligne();
        Some(Sortie::Ligne { pixels, abimee: !complete })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ligne(motif: impl Fn(usize) -> bool) -> Vec<bool> {
        (0..LARGEUR).map(motif).collect()
    }

    fn decoder(bits: &[bool]) -> (Vec<Vec<bool>>, usize, bool) {
        let mut d = Decodeur::default();
        let (mut lignes, mut abimees, mut fin) = (Vec::new(), 0, false);
        for &b in bits {
            match d.bit(b) {
                Some(Sortie::Ligne { pixels, abimee }) => {
                    abimees += abimee as usize;
                    lignes.push(pixels);
                }
                Some(Sortie::FinDePage) => fin = true,
                None => {}
            }
        }
        (lignes, abimees, fin)
    }

    #[test]
    fn tables_sans_ambiguite() {
        // la construction des arbres vérifie qu'aucun mot n'est le préfixe d'un autre
        let _ = Decodeur::default();
    }

    #[test]
    fn aller_retour() {
        let page = vec![
            ligne(|_| false),
            ligne(|_| true),
            ligne(|i| i % 2 == 0),
            ligne(|i| (i / 100) % 3 == 1),
            ligne(|i| i >= 1700),
            ligne(|i| i < 5 || (800..864).contains(&i)),
        ];
        let bits = coder_page(&page);
        // ligne blanche : EOL + complément 1728 + terminaison 0 = 12 + 9 + 8 bits
        let mut b = Vec::new();
        coder_ligne(&page[0], &mut b);
        assert_eq!(b.len(), 9 + 8);
        let (lignes, abimees, fin) = decoder(&bits);
        assert_eq!((lignes, abimees, fin), (page, 0, true));
    }

    #[test]
    fn erreur_puis_resynchronisation() {
        let page: Vec<Vec<bool>> = (0..5).map(|k| ligne(move |i| (i + k * 37) % 50 < 10)).collect();
        let mut bits = coder_page(&page);
        // quelques bits faux au milieu de la troisième ligne
        let debut3 = 12 + {
            let mut b = Vec::new();
            for l in &page[..2] {
                coder_ligne(l, &mut b);
                b.extend([false; 11].iter().chain([true].iter()));
            }
            b.len()
        };
        for k in 0..6 {
            bits[debut3 + 20 + k] ^= true;
        }
        let (lignes, abimees, fin) = decoder(&bits);
        assert!(fin);
        assert_eq!(lignes.len(), 5);
        assert!(abimees >= 1);
        assert_eq!(lignes[0], page[0]);
        assert_eq!(lignes[4], page[4]);
    }

    #[test]
    fn bourrage_avant_eol() {
        let page = vec![ligne(|i| i % 300 < 7), ligne(|_| false)];
        let mut bits = vec![false, false, false, false, false, false, false, false, false, false, false, true];
        for l in &page {
            coder_ligne(l, &mut bits);
            bits.extend([false; 30]); // bourrage
            bits.extend([false; 11].iter().chain([true].iter()));
        }
        for _ in 0..5 {
            bits.extend([false; 11].iter().chain([true].iter()));
        }
        let (lignes, abimees, fin) = decoder(&bits);
        assert_eq!((lignes, abimees, fin), (page, 0, true));
    }

    #[test]
    fn ignore_ce_qui_precede_le_premier_eol() {
        let page = vec![ligne(|i| i == 3)];
        let mut bits = vec![true, false, true, true, false, true, true, true];
        bits.extend(coder_page(&page));
        assert_eq!(decoder(&bits).0, page);
    }
}
