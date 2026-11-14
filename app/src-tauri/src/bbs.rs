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

    /// 3615 ORACLE (code du central, hors ligne comme en boucle locale) parle
    /// le protocole de l'appli : ses réponses passent par le décodeur du
    /// moteur (`on_data`) et s'affichent ligne à ligne dans la messagerie.
    #[test]
    fn l_oracle_parle_le_protocole_de_l_appli() {
        use std::time::{Duration, Instant};
        let cfg = v34_server::config::Oracle { fichier_cle: String::new(), debit_affichage: 56000, ..Default::default() };
        let mut oracle = v34_server::oracle::Oracle::new(&cfg).unwrap();
        oracle.attente_cartes = Duration::ZERO;
        let (in_tx, in_rx) = std::sync::mpsc::channel();
        let (out_tx, out_rx) = std::sync::mpsc::sync_channel(4096);
        let fin = std::thread::spawn(move || v34_server::oracle::session(std::sync::Arc::new(oracle), "test", "visiteur", in_rx, out_tx));

        let mut dec = Decoder::default();
        let (mut lignes, mut ouverte) = (Vec::new(), false);
        let mut lire_jusqu_a = |motif: &str, lignes: &mut Vec<String>, ouverte: &mut bool| {
            let t0 = Instant::now();
            while !lignes.iter().any(|l: &String| l.contains(motif)) {
                assert!(t0.elapsed() < Duration::from_secs(5), "« {motif} » jamais affiché : {lignes:#?}");
                if let Ok(b) = out_rx.recv_timeout(Duration::from_millis(50)) {
                    for it in dec.push(&b) {
                        match it {
                            Item::Frame(Frame::Chat(t)) => messagerie(lignes, ouverte, "remote", &t),
                            autre => panic!("seulement des trames de chat : {autre:?}"),
                        }
                    }
                }
            }
        };
        lire_jusqu_a("/fin : raccrocher.", &mut lignes, &mut ouverte);
        // une question, envoyée comme le fait le moteur (EngineCmd::Chat)
        let q = "Qui va gagner la Coupe du monde ?";
        in_tx.send(encode(&Frame::Chat(q.into()))).unwrap();
        messagerie(&mut lignes, &mut ouverte, "local", q);
        lire_jusqu_a(" F ]", &mut lignes, &mut ouverte);
        in_tx.send(encode(&Frame::Chat("/fin".into()))).unwrap();
        lire_jusqu_a("Au revoir !", &mut lignes, &mut ouverte);
        assert_eq!(fin.join().unwrap(), "consultation terminée (/fin)");

        assert_eq!(lignes[0], "<distant>    3 6 1 5", "la bannière en tête, sans ligne vide");
        assert!(lignes.iter().any(|l| l.contains("2,23 F la minute")));
        assert!(lignes.iter().all(|l| l.trim_end() != "<distant>"), "aucune ligne vide : {lignes:#?}");
        let moi = lignes.iter().position(|l| l.starts_with("<moi>")).unwrap();
        let cartes = lignes.iter().position(|l| l.contains("bat les cartes")).unwrap_or(moi + 1);
        assert!(cartes > moi, "la réponse vient après la question");
        assert!(lignes[moi + 1..].iter().any(|l| l.contains("Brésil") || l.contains("Stade de France")), "{lignes:#?}");
    }
}
