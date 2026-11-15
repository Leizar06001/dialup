//! Protocole applicatif « BBS » transporté par le flux fiable du modem :
//! chat et transferts de fichiers multiplexés.
//!
//! Le codage des trames vit dans `v34_net::bbs` (le central le parle aussi,
//! pour 3615 ORACLE) ; ici, ce qui ne concerne que l'appli : suivi des
//! transferts et enregistrement des fichiers reçus.

use std::path::{Path, PathBuf};
use std::time::Instant;

pub use v34_net::bbs::{crc32, encode, file_frames, Decoder, Frame, Item};

/// Envoi de fichier en cours. La progression vient des octets réellement
/// acquittés par la couche de correction d'erreurs de l'autre modem.
pub struct Outgoing {
    pub name: String,
    pub size: u64,
    /// Position dans le flux émis : début et fin des trames de ce fichier
    pub stream_start: u64,
    pub stream_end: u64,
    pub started: Instant,
}

impl Outgoing {
    /// Fraction acquittée (0..1) d'après le nombre d'octets acquittés.
    pub fn progress(&self, acked: u64) -> f64 {
        let span = (self.stream_end - self.stream_start).max(1) as f64;
        ((acked.saturating_sub(self.stream_start)) as f64 / span).clamp(0.0, 1.0)
    }
}

/// Réception de fichier en cours.
pub struct Incoming {
    pub id: u32,
    pub name: String,
    pub size: u64,
    pub data: Vec<u8>,
    pub started: Instant,
}

/// Texte d'une trame de chat reçue, pour la messagerie (qui assemble le
/// texte distant jusqu'au retour à la ligne). Entre abonnés, chaque trame
/// est un message entier : il finit sa ligne, sinon le message suivant s'y
/// collerait (« Bonjourça marche ? »). 3615 ORACLE (`flux`), lui, écrit au
/// fil de l'eau : des morceaux de lignes, chaque ligne précédée d'un retour.
pub fn chat_recu(texte: String, flux: bool) -> String {
    if flux || texte.ends_with('\n') {
        texte
    } else {
        texte + "\n"
    }
}

/// Nom de fichier sûr (pas de chemin, pas de caractères spéciaux).
pub fn safe_name(name: &str) -> String {
    let base = Path::new(name).file_name().and_then(|s| s.to_str()).unwrap_or("fichier");
    let cleaned: String = base.chars().map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { '_' } else { c }).collect();
    let cleaned = cleaned.trim_start_matches('.').to_string();
    if cleaned.is_empty() { "fichier".into() } else { cleaned }
}

/// Enregistre un fichier reçu dans `dir` sans écraser un fichier existant.
pub fn save_unique(dir: &Path, name: &str, data: &[u8]) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let name = safe_name(name);
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (name[..i].to_string(), name[i..].to_string()),
        _ => (name.clone(), String::new()),
    };
    let mut path = dir.join(&name);
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    std::fs::write(&path, data)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_sanitized() {
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name("a:b?.txt"), "a_b_.txt");
        assert_eq!(safe_name(".bashrc"), "bashrc");
    }

    /// Ce que fait la MESSAGERIE (messagerie.js, `onChat`) du texte distant :
    /// assemblé jusqu'au retour à la ligne, une ligne créée au premier
    /// caractère, et close par nos propres messages.
    fn messagerie(lignes: &mut Vec<String>, ouverte: &mut bool, de: &str, texte: &str) {
        if de == "local" {
            *ouverte = false;
            lignes.push(format!("<moi> {texte}"));
            return;
        }
        let parts: Vec<&str> = texte.split('\n').collect();
        for (i, p) in parts.iter().enumerate() {
            if !p.is_empty() {
                if !*ouverte {
                    lignes.push("<distant> ".into());
                    *ouverte = true;
                }
                lignes.last_mut().unwrap().push_str(p);
            }
            if i < parts.len() - 1 {
                *ouverte = false;
            }
        }
    }

    /// Entre abonnés : chaque message distant sur sa ligne, même quand deux
    /// arrivent de suite (le second se collait au premier).
    #[test]
    fn messages_entre_abonnes_sur_leur_ligne() {
        let (mut lignes, mut ouverte) = (Vec::new(), false);
        for (de, t) in [("remote", "Bonjour"), ("remote", "ça marche ?"), ("local", "oui"), ("remote", "super")] {
            let t = if de == "remote" { chat_recu(t.into(), false) } else { t.into() };
            messagerie(&mut lignes, &mut ouverte, de, &t);
        }
        assert_eq!(lignes, ["<distant> Bonjour", "<distant> ça marche ?", "<moi> oui", "<distant> super"]);
        // l'oracle, lui, écrit par morceaux
        assert_eq!(chat_recu("La voy".into(), true), "La voy");
    }
}
