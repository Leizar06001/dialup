//! Le code morse international (UIT-R M.1677-1), lettres accentuées
//! françaises comprises, et la lecture d'un texte en signes.
//!
//! Les procédures (« prosignes ») s'écrivent entre chevrons : `<KA>` (début
//! de message), `<AR>` (fin de message), `<SK>` (fin de contact), `<HH>`
//! (erreur)… Dans un texte à émettre, `<AR>` part comme un seul signe, sans
//! silence entre ses lettres ; le texte reçu les écrit de la même façon.

/// Un signe : son texte et son code (`.` point, `-` trait). L'espace entre
/// deux mots est le signe [`ESPACE`], au code vide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signe {
    pub texte: &'static str,
    pub code: &'static str,
}

/// Le silence entre deux mots.
pub const ESPACE: Signe = Signe { texte: " ", code: "" };

impl Signe {
    pub fn est_espace(&self) -> bool {
        self.code.is_empty()
    }
}

/// La table, dans l'ordre de préférence de l'écoute : quand deux textes ont
/// le même code, le premier l'emporte à la réception (les doublons de la fin
/// ne servent qu'à l'émission).
pub const TABLE: &[(&str, &str)] = &[
    ("A", ".-"), ("B", "-..."), ("C", "-.-."), ("D", "-.."), ("E", "."), ("F", "..-."), ("G", "--."),
    ("H", "...."), ("I", ".."), ("J", ".---"), ("K", "-.-"), ("L", ".-.."), ("M", "--"), ("N", "-."),
    ("O", "---"), ("P", ".--."), ("Q", "--.-"), ("R", ".-."), ("S", "..."), ("T", "-"), ("U", "..-"),
    ("V", "...-"), ("W", ".--"), ("X", "-..-"), ("Y", "-.--"), ("Z", "--.."),
    ("0", "-----"), ("1", ".----"), ("2", "..---"), ("3", "...--"), ("4", "....-"),
    ("5", "....."), ("6", "-...."), ("7", "--..."), ("8", "---.."), ("9", "----."),
    // lettres accentuées (et quelques cousines européennes)
    ("É", "..-.."), ("È", ".-..-"), ("À", ".--.-"), ("Ç", "-.-.."),
    ("Ü", "..--"), ("Ä", ".-.-"), ("Ö", "---."), ("Ñ", "--.--"),
    // ponctuation (UIT-R M.1677, puis les usages courants : ! ; _ $ &)
    (".", ".-.-.-"), (",", "--..--"), ("?", "..--.."), ("'", ".----."), ("/", "-..-."),
    ("(", "-.--."), (")", "-.--.-"), (":", "---..."), ("=", "-...-"), ("-", "-....-"),
    ("\"", ".-..-."), ("@", ".--.-."), ("!", "-.-.--"), (";", "-.-.-."), ("_", "..--.-"),
    ("$", "...-..-"), ("&", ".-..."),
    // procédures
    ("<KA>", "-.-.-"), ("<AR>", ".-.-."), ("<SK>", "...-.-"), ("<HH>", "........"), ("<SN>", "...-."),
    // doublons : « + » est AR, BT est « = », AS est « & », KN est « ( »
    ("+", ".-.-."), ("<BT>", "-...-"), ("<AS>", ".-..."), ("<KN>", "-.--."),
];

/// Signe d'un texte de la table (« A », « É », « <AR> »…).
pub fn signe(texte: &str) -> Option<Signe> {
    TABLE.iter().find(|(t, _)| *t == texte).map(|&(texte, code)| Signe { texte, code })
}

/// Texte d'un code reçu ; sept points ou plus : `<HH>` (« erreur, je reprends »).
pub fn lire(code: &str) -> Option<&'static str> {
    if code.len() >= 7 && code.bytes().all(|b| b == b'.') {
        return Some("<HH>");
    }
    TABLE.iter().find(|(_, c)| *c == code).map(|&(t, _)| t)
}

/// « .-.- » → « ·−·− », pour l'affichage.
pub fn joli(code: &str) -> String {
    code.chars().map(|c| if c == '.' { '·' } else { '−' }).collect()
}

/// Les signes d'un caractère : majuscules, accents français (É, È, À, Ç
/// ont leur code ; ê, â, î, ô, û perdent leur accent), ligatures dépliées,
/// guillemets et apostrophes typographiques ramenés au plus simple. Rien
/// pour ce que le morse ne sait pas dire.
pub fn signes_du_caractere(c: char) -> Vec<Signe> {
    let s: String = match c {
        'é' | 'É' => "É".into(),
        'è' | 'È' => "È".into(),
        'à' | 'À' => "À".into(),
        'ç' | 'Ç' => "Ç".into(),
        'ü' | 'Ü' => "Ü".into(),
        'ä' | 'Ä' => "Ä".into(),
        'ö' | 'Ö' => "Ö".into(),
        'ñ' | 'Ñ' => "Ñ".into(),
        'ê' | 'ë' | 'Ê' | 'Ë' => "E".into(),
        'â' | 'á' | 'Â' | 'Á' => "A".into(),
        'î' | 'ï' | 'Î' | 'Ï' | 'í' => "I".into(),
        'ô' | 'ó' | 'Ô' | 'Ó' => "O".into(),
        'û' | 'ù' | 'ú' | 'Û' | 'Ù' => "U".into(),
        'ÿ' | 'Ÿ' => "Y".into(),
        'œ' | 'Œ' => "OE".into(),
        'æ' | 'Æ' => "AE".into(),
        '’' | '‘' | '`' | '´' => "'".into(),
        '«' | '»' | '“' | '”' => "\"".into(),
        '–' | '—' => "-".into(),
        '…' => "...".into(),
        '×' => "X".into(),
        c if c.is_whitespace() => " ".into(),
        c => c.to_uppercase().collect(),
    };
    s.chars()
        .filter_map(|c| if c == ' ' { Some(ESPACE) } else { signe(c.encode_utf8(&mut [0; 4])) })
        .collect()
}

/// Lit un texte au fil de l'eau (octets UTF-8, éventuellement coupés au
/// milieu d'un caractère ou d'une procédure `<AR>`) et le découpe en signes.
/// Les espaces répétés n'en font qu'un.
#[derive(Clone, Debug, Default)]
pub struct Lecteur {
    utf8: Vec<u8>,
    /// Procédure en cours de lecture : « < » puis jusqu'à trois lettres
    jeton: Option<String>,
    dernier_espace: bool,
}

impl Lecteur {
    pub fn pousser(&mut self, octets: &[u8]) -> Vec<Signe> {
        self.utf8.extend_from_slice(octets);
        let mut out = Vec::new();
        let mut texte = String::new();
        loop {
            match std::str::from_utf8(&self.utf8) {
                Ok(t) => {
                    texte.push_str(t);
                    self.utf8.clear();
                    break;
                }
                Err(e) => {
                    let bon = e.valid_up_to();
                    texte.push_str(std::str::from_utf8(&self.utf8[..bon]).unwrap_or_default());
                    match e.error_len() {
                        // octet invalide : sauté
                        Some(n) => drop(self.utf8.drain(..bon + n)),
                        // caractère coupé : la suite viendra
                        None => {
                            self.utf8.drain(..bon);
                            break;
                        }
                    }
                }
            }
        }
        for c in texte.chars() {
            self.caractere(c, &mut out);
        }
        out
    }

    /// Plus rien ne vient : une procédure entamée (« <K ») part telle quelle.
    pub fn vider(&mut self) -> Vec<Signe> {
        let mut out = Vec::new();
        if let Some(j) = self.jeton.take() {
            for c in j.chars() {
                self.ajouter(signes_du_caractere(c), &mut out);
            }
        }
        out
    }

    /// Une procédure attend sa fin (« <A » sans « > »).
    pub fn en_attente(&self) -> bool {
        self.jeton.is_some() || !self.utf8.is_empty()
    }

    fn caractere(&mut self, c: char, out: &mut Vec<Signe>) {
        if let Some(mut j) = self.jeton.take() {
            if c == '>' {
                j.push('>');
                match signe(&j) {
                    Some(s) if j.len() > 2 => self.ajouter(vec![s], out),
                    _ => {
                        for c in j.chars() {
                            self.ajouter(signes_du_caractere(c), out);
                        }
                    }
                }
                return;
            }
            if c.is_ascii_alphabetic() && j.len() < 4 {
                j.push(c.to_ascii_uppercase());
                self.jeton = Some(j);
                return;
            }
            // pas une procédure : ce qui a été lu part comme du texte
            for c in j.chars() {
                self.ajouter(signes_du_caractere(c), out);
            }
        }
        if c == '<' {
            self.jeton = Some("<".into());
        } else {
            self.ajouter(signes_du_caractere(c), out);
        }
    }

    fn ajouter(&mut self, signes: Vec<Signe>, out: &mut Vec<Signe>) {
        for s in signes {
            if s.est_espace() {
                if self.dernier_espace {
                    continue;
                }
                self.dernier_espace = true;
            } else {
                self.dernier_espace = false;
            }
            out.push(s);
        }
    }
}

/// Tout un texte en signes (pour les essais).
pub fn signes(texte: &str) -> Vec<Signe> {
    let mut l = Lecteur::default();
    let mut v = l.pousser(texte.as_bytes());
    v.extend(l.vider());
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(t: &str) -> Vec<&'static str> {
        signes(t).iter().map(|s| if s.est_espace() { "/" } else { s.code }).collect()
    }

    #[test]
    fn la_table_se_relit() {
        // chaque code n'a qu'une lecture : la première entrée de la table
        for (i, (t, c)) in TABLE.iter().enumerate() {
            let premier = TABLE.iter().position(|(_, c2)| c2 == c).unwrap();
            assert_eq!(lire(c), Some(TABLE[premier].0), "{t} {c}");
            if premier == i {
                assert_eq!(lire(c), Some(*t));
            }
        }
        assert_eq!(lire(".-.-."), Some("<AR>"), "AR plutôt que « + »");
        assert_eq!(lire("-...-"), Some("="));
        assert_eq!(lire("..--..--"), None);
        assert_eq!(lire("........."), Some("<HH>"));
        assert_eq!(joli(".-"), "·−");
    }

    #[test]
    fn francais_accents_et_procedures() {
        assert_eq!(codes("Éè à Ç"), ["..-..", ".-..-", "/", ".--.-", "/", "-.-.."]);
        assert_eq!(codes("forêt"), ["..-.", "---", ".-.", ".", "-"], "ê perd son accent");
        assert_eq!(codes("cœur"), ["-.-.", "---", ".", "..-", ".-."]);
        assert_eq!(codes("<KA>sos<AR>"), ["-.-.-", "...", "---", "...", ".-.-."]);
        assert_eq!(codes("<sk>"), ["...-.-"]);
        // pas des procédures : le texte part tel quel (« < » et « > » sont muets)
        assert_eq!(codes("<ZZ> 1<2"), ["--..", "--..", "/", ".----", "..---"]);
        assert_eq!(codes("a  \n b"), [".-", "/", "-..."], "espaces fusionnés");
        assert_eq!(codes("l’été « 98 »"), [".-..", ".----.", "..-..", "-", "..-..", "/", ".-..-.", "/", "----.", "---..", "/", ".-..-."]);
        assert_eq!(codes("€#%"), Vec::<&str>::new());
    }

    #[test]
    fn lecture_au_fil_de_l_eau() {
        // procédure et caractère coupés entre deux envois
        let mut l = Lecteur::default();
        let mut v = l.pousser(b"<A");
        assert!(v.is_empty() && l.en_attente());
        v.extend(l.pousser(b"R>\xC3"));
        v.extend(l.pousser(b"\xA9"));
        assert_eq!(v.iter().map(|s| s.texte).collect::<Vec<_>>(), ["<AR>", "É"]);
        // une procédure entamée puis abandonnée
        assert!(l.pousser(b"<K").is_empty());
        assert_eq!(l.vider().iter().map(|s| s.texte).collect::<Vec<_>>(), ["K"]);
        assert!(!l.en_attente());
    }
}
