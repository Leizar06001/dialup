//! Traitement du texte : questions reçues, réponses de l'IA (texte brut,
//! jeu de caractères sûr, longueur bornée et coupée proprement).

/// Longueur maximale d'une question (caractères)
pub const QUESTION_MAX: usize = 300;
/// Longueur maximale d'une réponse affichée (caractères)
pub const REPONSE_MAX: usize = 500;

/// Question telle qu'elle partira vers l'IA : sans caractères de contrôle,
/// espaces resserrés, 300 caractères au plus.
pub fn assainir_question(s: &str) -> String {
    let propre: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let mut out = propre.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some((i, _)) = out.char_indices().nth(QUESTION_MAX) {
        out.truncate(i);
    }
    out
}

/// Clé de comparaison : minuscules, sans accents, ponctuation en espaces.
/// « Qui va gagner la Coupe du Monde ?! » = « qui va gagner la coupe du monde ».
pub fn normaliser(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        match c {
            'à' | 'â' | 'ä' | 'á' => out.push('a'),
            'ç' => out.push('c'),
            'é' | 'è' | 'ê' | 'ë' => out.push('e'),
            'î' | 'ï' | 'í' => out.push('i'),
            'ô' | 'ö' | 'ó' => out.push('o'),
            'ù' | 'û' | 'ü' | 'ú' => out.push('u'),
            'ÿ' => out.push('y'),
            'œ' => out.push_str("oe"),
            'æ' => out.push_str("ae"),
            c if c.is_alphanumeric() => out.push(c),
            _ => out.push(' '),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Caractère admis sur la ligne : ASCII imprimable, Latin-1 (les accents
/// du français), œ, Œ et €. Le reste (émoticônes, idéogrammes…) disparaît.
fn admis(c: char) -> bool {
    matches!(c, ' '..='~' | '\u{A1}'..='\u{FF}' | 'œ' | 'Œ' | '€')
}

/// Réponse de l'IA en texte brut d'un seul paragraphe : Markdown retiré,
/// typographie ramenée à des caractères simples, jeu de caractères sûr.
pub fn assainir_reponse(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ligne in s.lines() {
        // titres et puces Markdown
        let l = ligne.trim().trim_start_matches('#').trim_start();
        let l = l.strip_prefix("- ").or_else(|| l.strip_prefix("* ")).unwrap_or(l);
        if l.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        for c in l.chars() {
            match c {
                '*' | '`' | '#' => {}
                '\u{2018}' | '\u{2019}' | '\u{201B}' | '\u{2032}' => out.push('\''),
                '\u{201C}' | '\u{201D}' | '\u{201E}' => out.push('"'),
                '\u{2010}'..='\u{2015}' | '\u{2212}' => out.push('-'),
                '\u{2026}' => out.push_str("..."),
                '\u{A0}' | '\u{2000}'..='\u{200A}' | '\u{202F}' | '\t' => out.push(' '),
                c if admis(c) => out.push(c),
                _ => {}
            }
        }
    }
    out = out.replace("__", "");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Fin de phrase à la position `i` de `cs` : `.`, `!` ou `?` suivi d'un
/// espace, de la fin du texte ou d'un guillemet / d'une parenthèse fermants.
/// Renvoie la longueur à garder (guillemet compris).
fn fin_de_phrase(cs: &[char], i: usize) -> Option<usize> {
    if !matches!(cs[i], '.' | '!' | '?') {
        return None;
    }
    match cs.get(i + 1) {
        None => Some(i + 1),
        Some(c) if c.is_whitespace() => Some(i + 1),
        Some('»' | '"' | ')') => Some(i + 2),
        _ => None,
    }
}

/// Coupe la réponse à `max` caractères au plus, à la fin d'une phrase.
/// `coupee` : l'IA s'est arrêtée en route (limite de jetons), la dernière
/// phrase est sans doute inachevée.
pub fn couper(s: &str, max: usize, coupee: bool) -> String {
    let cs: Vec<char> = s.chars().collect();
    let point = |c: &char| matches!(c, '.' | '!' | '?');
    let termine = match cs.as_slice() {
        [.., p] if point(p) => true,
        [.., p, q] => point(p) && matches!(q, '»' | '"' | ')'),
        _ => false,
    };
    if cs.len() <= max && (!coupee || termine) {
        return s.to_string();
    }
    let zone = cs.len().min(max);
    // dernière fin de phrase dans la zone (pas trop tôt : pas un « M. » isolé)
    let mut garde = None;
    for i in 0..zone {
        if let Some(n) = fin_de_phrase(&cs, i) {
            if n <= zone {
                garde = Some(n);
            }
        }
    }
    if let Some(n) = garde.filter(|&n| n >= zone / 4) {
        return cs[..n].iter().collect::<String>().trim_end().to_string();
    }
    // pas de phrase complète : on coupe au dernier mot, avec des points de suspension
    let limite = zone.saturating_sub(3);
    let fin = cs[..limite].iter().rposition(|c| c.is_whitespace()).unwrap_or(limite);
    let mut out: String = cs[..fin].iter().collect::<String>().trim_end_matches([' ', ',', ';', ':']).to_string();
    out.push_str("...");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn questions() {
        assert_eq!(assainir_question("  Quel  avenir\tpour\r\nle Minitel ?\u{7} "), "Quel avenir pour le Minitel ?");
        let longue = "é".repeat(500);
        assert_eq!(assainir_question(&longue).chars().count(), QUESTION_MAX);
        assert_eq!(normaliser("Qui va gagner la Coupe du Monde ?!"), normaliser("qui va gagner la coupe du monde"));
        assert_eq!(normaliser("L'Été, ÇA BRÛLE"), "l ete ca brule");
    }

    #[test]
    fn reponses_en_texte_brut() {
        let md = "## Vision\n**Les astres** disent :\n- oui\n- `non`\n\n« Peut‑être » — c’est ça… 🔮✨ 東京";
        assert_eq!(assainir_reponse(md), "Vision Les astres disent : oui non « Peut-être » - c'est ça...");
        assert_eq!(assainir_reponse("A\u{202F}!\u{A0}B"), "A ! B");
        assert_eq!(assainir_reponse("Prix : 10 € ou 65,60 F, cœur"), "Prix : 10 € ou 65,60 F, cœur");
        // jamais de caractère de contrôle (ni de 0xA5 seul, ni d'échappement)
        assert!(!assainir_reponse("a\u{1b}[2Jb\u{0}c").chars().any(|c| c.is_control()));
    }

    #[test]
    fn coupe_propre() {
        let court = "Les astres sont formels. Revenez demain.";
        assert_eq!(couper(court, 500, false), court);
        assert_eq!(couper(court, 500, true), court, "terminée : rien à couper");
        // coupée en route par la limite de jetons : on revient à la dernière phrase entière
        assert_eq!(couper("Les astres sont formels. Revenez demain, après le", 500, true), "Les astres sont formels.");
        assert_eq!(couper("« Oui ! » dit l'oracle. Et puis", 500, true), "« Oui ! » dit l'oracle.");
        // trop longue : coupée à la dernière phrase qui tient
        let longue = format!("{}Fin.", "Une phrase de trente caractères. ".repeat(20));
        let c = couper(&longue, REPONSE_MAX, false);
        assert!(c.chars().count() <= REPONSE_MAX && c.ends_with('.'), "{c}");
        // aucune fin de phrase : au dernier mot, avec des points de suspension
        let mot = "abracadabra ".repeat(60);
        let c = couper(&mot, 100, true);
        assert!(c.chars().count() <= 100 && c.ends_with("abracadabra..."), "{c}");
        // fin de phrase trop tôt (« M. ») : on préfère couper au mot
        let c = couper(&format!("M. {}", "x ".repeat(80)), 60, true);
        assert!(c.ends_with("..."), "{c}");
    }
}
