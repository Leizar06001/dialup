//! Administration de l'annuaire depuis la ligne de commande :
//! `ajouter-abonne`, `retirer-abonne`, `liste-abonnes`.
//!
//! Le fichier de configuration est modifié « à la main » (texte), pour
//! conserver ses commentaires ; le résultat est relu et validé AVANT
//! d'être écrit. L'écriture se fait en place (même propriétaire, mêmes
//! droits). Un serveur en marche recharge l'annuaire tout seul.

use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use v34_net::auth::{valid_name, PasswordHash};
use v34_net::signal::valid_number;

use crate::config::Config;

const HEADER: &str = "[[abonnes]]";

/// Lignes d'en-tête de table (`[x]`, `[[x]]`), hors commentaires.
fn is_table_header(line: &str) -> bool {
    let t = line.trim();
    let Some(inner) = t.strip_prefix('[') else { return false };
    let inner = inner.split('#').next().unwrap_or_default().trim_end();
    let inner = inner.strip_prefix('[').unwrap_or(inner);
    let Some(name) = inner.strip_suffix(']') else { return false };
    let name = name.strip_suffix(']').unwrap_or(name).trim();
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || "_-. \"".contains(c))
}

fn is_abonnes_header(line: &str) -> bool {
    let t = line.trim();
    t.starts_with("[[") && t.split('#').next().unwrap_or_default().replace(' ', "") == HEADER
}

/// Plage de lignes du i-ème bloc `[[abonnes]]` (en-tête compris, sans les
/// lignes vides / commentaires qui le suivent).
fn block_range(lines: &[&str], index: usize) -> Option<(usize, usize)> {
    let start = lines.iter().enumerate().filter(|(_, l)| is_abonnes_header(l)).nth(index)?.0;
    let mut end = lines[start + 1..].iter().position(|l| is_table_header(l)).map(|p| start + 1 + p).unwrap_or(lines.len());
    while end > start + 1 && {
        let t = lines[end - 1].trim();
        t.is_empty() || t.starts_with('#')
    } {
        end -= 1;
    }
    Some((start, end))
}

fn block(nom: &str, numero: &str, hash: &str) -> Vec<String> {
    vec![HEADER.to_string(), format!("nom = \"{nom}\""), format!("numero = \"{numero}\""), format!("mot_de_passe_hash = \"{hash}\"")]
}

fn parse(text: &str) -> Result<Config, String> {
    let cfg: Config = toml::from_str(text).map_err(|e| e.to_string())?;
    cfg.validate()?;
    Ok(cfg)
}

/// Ajoute ou met à jour un abonné ; renvoie le nouveau texte.
pub fn upsert(text: &str, nom: &str, numero: &str, hash: &PasswordHash) -> Result<String, String> {
    if !valid_name(nom) {
        return Err(format!("nom invalide « {nom} » (1 à 32 caractères A-Z a-z 0-9 . _ -)"));
    }
    if !valid_number(numero) {
        return Err(format!("numéro invalide « {numero} » (1 à 20 chiffres)"));
    }
    let cfg = parse(text)?;
    if let Some(autre) = cfg.abonnes.iter().find(|a| a.numero == numero && a.nom != nom) {
        return Err(format!("le {numero} est déjà attribué à {}", autre.nom));
    }
    if cfg.service_for(numero).is_some() {
        return Err(format!("le {numero} est un numéro spécial"));
    }
    let lines: Vec<&str> = text.lines().collect();
    let headers = lines.iter().filter(|l| is_abonnes_header(l)).count();
    if headers != cfg.abonnes.len() {
        return Err("annuaire écrit sous une forme non gérée (tableau en ligne ?) : modifiez le fichier à la main".into());
    }
    let new_block = block(nom, numero, &hash.to_string());
    let mut out: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
    match cfg.abonnes.iter().position(|a| a.nom == nom) {
        Some(i) => {
            let (s, e) = block_range(&lines, i).ok_or("bloc introuvable")?;
            out.splice(s..e, new_block);
        }
        None => {
            // après le dernier abonné s'il y en a, sinon à la fin du fichier
            let at = match cfg.abonnes.len() {
                0 => out.len(),
                n => block_range(&lines, n - 1).ok_or("bloc introuvable")?.1,
            };
            let mut ins = vec![String::new()];
            ins.extend(new_block);
            out.splice(at..at, ins);
        }
    }
    let mut new_text = out.join("\n");
    new_text.push('\n');
    let check = parse(&new_text).map_err(|e| format!("modification refusée (fichier résultant invalide : {e})"))?;
    match check.abonnes.iter().find(|a| a.nom == nom) {
        Some(a) if a.numero == numero => Ok(new_text),
        _ => Err("modification non vérifiable : fichier inchangé".into()),
    }
}

/// Retire un abonné ; renvoie le nouveau texte.
pub fn remove(text: &str, nom: &str) -> Result<String, String> {
    let cfg = parse(text)?;
    let i = cfg.abonnes.iter().position(|a| a.nom == nom).ok_or_else(|| format!("abonné inconnu : {nom}"))?;
    let lines: Vec<&str> = text.lines().collect();
    if lines.iter().filter(|l| is_abonnes_header(l)).count() != cfg.abonnes.len() {
        return Err("annuaire écrit sous une forme non gérée : modifiez le fichier à la main".into());
    }
    let (s, mut e) = block_range(&lines, i).ok_or("bloc introuvable")?;
    // une ligne vide qui suivait le bloc part avec lui
    if e < lines.len() && lines[e].trim().is_empty() {
        e += 1;
    }
    let mut out: Vec<&str> = lines.clone();
    out.drain(s..e);
    let mut new_text = out.join("\n");
    new_text.push('\n');
    let check = parse(&new_text).map_err(|e| format!("modification refusée (fichier résultant invalide : {e})"))?;
    if check.abonnes.iter().any(|a| a.nom == nom) || check.abonnes.len() + 1 != cfg.abonnes.len() {
        return Err("modification non vérifiable : fichier inchangé".into());
    }
    Ok(new_text)
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| {
        format!("lecture de {} : {e}\n(créez-le d'abord : v34-server --example-config > {})", path.display(), path.display())
    })
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    // en place : le fichier garde son propriétaire et ses droits (640 root:v34)
    std::fs::write(path, text).map_err(|e| format!("écriture de {} : {e}", path.display()))
}

pub fn add_subscriber(path: &Path, nom: &str, numero: &str, password: &str, iterations: u32) -> Result<bool, String> {
    let text = read(path)?;
    let existed = parse(&text)?.abonnes.iter().any(|a| a.nom == nom);
    let hash = PasswordHash::new(password, iterations);
    let new_text = upsert(&text, nom, numero, &hash)?;
    write(path, &new_text)?;
    Ok(existed)
}

pub fn remove_subscriber(path: &Path, nom: &str) -> Result<(), String> {
    let text = read(path)?;
    let new_text = remove(&text, nom)?;
    write(path, &new_text)
}

pub fn list_subscribers(path: &Path) -> Result<Config, String> {
    Config::load(path)
}

/// Lit un mot de passe au clavier sans l'afficher (via `stty -echo`), ou
/// une ligne de l'entrée standard si ce n'est pas un terminal (scripts).
pub fn read_password(prompt: &str, confirm: bool) -> Result<String, String> {
    let stdin = io::stdin();
    if !stdin.is_terminal() {
        let mut line = String::new();
        stdin.lock().read_line(&mut line).map_err(|e| e.to_string())?;
        return Ok(line.trim_end_matches(['\r', '\n']).to_string());
    }
    let ask = |p: &str| -> Result<String, String> {
        eprint!("{p}");
        let _ = io::stderr().flush();
        let hidden = Command::new("stty").arg("-echo").stdin(Stdio::inherit()).status().is_ok_and(|s| s.success());
        let mut line = String::new();
        let res = stdin.lock().read_line(&mut line);
        if hidden {
            let _ = Command::new("stty").arg("echo").stdin(Stdio::inherit()).status();
            eprintln!();
        }
        res.map_err(|e| e.to_string())?;
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    };
    let pw = ask(prompt)?;
    if confirm && ask("Confirmez : ")? != pw {
        return Err("les deux saisies diffèrent".into());
    }
    Ok(pw)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "\
# configuration de test
[serveur]
ecoute = \"0.0.0.0:3434\"   # port UDP

# --- annuaire ---

[fai]
flux_max = 8
";

    fn h(pw: &str) -> PasswordHash {
        PasswordHash::new(pw, 1000)
    }

    #[test]
    fn ajout_modification_retrait_en_gardant_les_commentaires() {
        let t1 = upsert(BASE, "gaston", "0142424242", &h("a")).unwrap();
        let t2 = upsert(&t1, "alice", "0612345678", &h("b")).unwrap();
        assert!(t2.contains("# --- annuaire ---") && t2.contains("# port UDP"), "commentaires conservés");
        let c = parse(&t2).unwrap();
        assert_eq!(c.abonnes.len(), 2);
        assert_eq!(c.fai.flux_max, 8, "la section [fai] n'est pas avalée");
        // alice est insérée juste après gaston
        let pos = |s: &str| t2.find(s).unwrap();
        assert!(pos("nom = \"gaston\"") < pos("nom = \"alice\""));

        // mise à jour : nouveau numéro et nouveau mot de passe
        let hb = h("nouveau");
        let t3 = upsert(&t2, "gaston", "0143434343", &hb).unwrap();
        let c = parse(&t3).unwrap();
        let r = c.abonnes.iter().find(|a| a.nom == "gaston").unwrap();
        assert_eq!(r.numero, "0143434343");
        assert_eq!(r.mot_de_passe_hash, hb.to_string());
        assert_eq!(c.abonnes.len(), 2);

        // retrait
        let t4 = remove(&t3, "gaston").unwrap();
        let c = parse(&t4).unwrap();
        assert_eq!(c.abonnes.iter().map(|a| a.nom.as_str()).collect::<Vec<_>>(), vec!["alice"]);
        assert_eq!(c.fai.flux_max, 8);
        assert!(t4.contains("# --- annuaire ---"));
    }

    #[test]
    fn refus() {
        let t = upsert(BASE, "gaston", "0142424242", &h("a")).unwrap();
        assert!(upsert(&t, "alice", "0142424242", &h("b")).unwrap_err().contains("gaston"), "numéro déjà pris");
        assert!(upsert(&t, "alice", "0836656565", &h("b")).is_err(), "numéro spécial");
        assert!(upsert(&t, "pas valide", "01", &h("b")).is_err());
        assert!(upsert(&t, "bob", "01a", &h("b")).is_err());
        assert!(remove(&t, "inconnu").is_err());
    }

    #[test]
    fn en_tetes() {
        assert!(is_table_header("[fai]"));
        assert!(is_table_header("  [[abonnes]]  # commentaire"));
        assert!(!is_table_header("ports = [80, 443]"));
        assert!(!is_table_header("# [fai]"));
        assert!(is_abonnes_header("[[ abonnes ]]"));
        assert!(!is_abonnes_header("[[numeros_speciaux]]"));
    }
}
