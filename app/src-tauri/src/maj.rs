//! Mises à jour : y a-t-il une version plus récente du logiciel ?
//!
//! On demande à GitHub la dernière version publiée du dépôt (ni brouillon ni
//! préversion : `releases/latest`) et on la compare à la nôtre. Au
//! démarrage, puis une fois par jour tant que le logiciel tourne ; à la
//! demande avec `AT*V`. Rien ne s'ouvre tout seul : le badge MAJ clignote,
//! et c'est un clic dessus qui ouvre la page de la version dans le
//! navigateur du système (comme la disquette de mise à jour qu'on allait
//! chercher soi-même chez le revendeur).
//!
//! Seule l'adresse IP voit GitHub : la requête ne dit rien de l'abonné.

use std::io::Read;
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// Dépôt des versions publiées
pub const DEPOT: &str = "Leizar06001/dialup";
/// Vérification automatique : une fois par jour
const PERIODE: Duration = Duration::from_secs(24 * 3600);

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Maj {
    pub actuelle: String,
    pub derniere: String,
    /// La dernière version publiée est plus récente que la nôtre
    pub nouvelle: bool,
    /// Page de la version sur GitHub
    pub page: String,
    /// AAAA-MM-JJ
    pub date: String,
    /// Les premières lignes des notes de version
    pub notes: String,
    /// Vérification demandée (AT*V) : on répond même s'il n'y a rien de neuf
    pub manuelle: bool,
}

/// Page de la dernière version trouvée (seule adresse que `ouvrir` accepte)
static PAGE: Mutex<Option<String>> = Mutex::new(None);

/// « v0.1.7 », « 0.1.7 » → (0, 1, 7)
fn version(s: &str) -> Option<(u32, u32, u32)> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let s = s.split(['-', '+']).next()?;
    let mut it = s.split('.').map(|p| p.parse::<u32>().ok());
    let v = (it.next()??, it.next().unwrap_or(Some(0))?, it.next().unwrap_or(Some(0))?);
    Some(v)
}

/// Notre version (V34_MAJ_ACTUELLE la remplace, pour les essais)
fn actuelle() -> String {
    std::env::var("V34_MAJ_ACTUELLE").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string())
}

/// Lit la réponse de GitHub (`releases/latest`).
fn analyser(v: &serde_json::Value, actuelle: &str, manuelle: bool) -> Result<Maj, String> {
    let tag = v["tag_name"].as_str().ok_or("réponse inattendue de GitHub")?;
    let derniere = version(tag).ok_or_else(|| format!("version illisible : {tag}"))?;
    let nous = version(actuelle).unwrap_or((0, 0, 0));
    let prefixe = format!("https://github.com/{DEPOT}/releases/");
    let page = v["html_url"].as_str().filter(|u| u.starts_with(&prefixe)).map_or_else(|| format!("{prefixe}latest"), str::to_string);
    // Les notes de version, sans titres ni tableaux (celui des
    // téléchargements) : les lignes de texte et les puces, sans leur tiret
    let notes: String = v["body"]
        .as_str()
        .unwrap_or("")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with(['#', '|', '`', '>']))
        .map(|l| l.trim_start_matches(['-', '*', '+']).trim().replace("**", ""))
        .filter(|l| !l.is_empty())
        .take(3)
        .collect::<Vec<_>>()
        .join(" · ");
    Ok(Maj {
        actuelle: actuelle.to_string(),
        derniere: format!("{}.{}.{}", derniere.0, derniere.1, derniere.2),
        nouvelle: derniere > nous,
        page,
        date: v["published_at"].as_str().and_then(|d| d.get(..10)).unwrap_or("").to_string(),
        notes: notes.chars().take(300).collect(),
        manuelle,
    })
}

/// Interroge GitHub (bloquant : dans un thread à part).
pub fn verifier(manuelle: bool) -> Result<Maj, String> {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(10)).build();
    let rep = agent
        .get(&format!("https://api.github.com/repos/{DEPOT}/releases/latest"))
        .set("User-Agent", "v34-modem")
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(404, _) => "aucune version publiée".to_string(),
            ureq::Error::Status(c, _) => format!("GitHub répond {c}"),
            ureq::Error::Transport(t) => format!("GitHub injoignable ({})", t.kind()),
        })?;
    let v: serde_json::Value = serde_json::from_reader(rep.into_reader().take(1 << 20)).map_err(|e| e.to_string())?;
    let m = analyser(&v, &actuelle(), manuelle)?;
    *PAGE.lock().unwrap_or_else(|e| e.into_inner()) = Some(m.page.clone());
    Ok(m)
}

/// Vérifie et annonce le résultat à l'IHM (`v34:maj`). Vérification
/// automatique : on ne dit rien s'il n'y a rien de neuf ou si GitHub ne
/// répond pas.
pub fn annoncer(app: &AppHandle, manuelle: bool) {
    let r = verifier(manuelle);
    if std::env::var_os("V34_TRACE").is_some() {
        eprintln!("[{:>6}] maj {r:?}", crate::engine::trace_ms());
    }
    match r {
        Ok(m) if m.nouvelle || manuelle => {
            let _ = app.emit("v34:maj", m);
        }
        Ok(_) => {}
        Err(e) if manuelle => {
            let _ = app.emit("v34:maj-erreur", e);
        }
        Err(_) => {}
    }
}

/// Vérification au démarrage (après quelques secondes), puis chaque jour.
/// V34_MAJ=non la coupe (essais).
pub fn surveiller(app: AppHandle) {
    if std::env::var("V34_MAJ").is_ok_and(|v| v == "non") {
        return;
    }
    let _ = std::thread::Builder::new().name("v34-maj".into()).spawn(move || {
        std::thread::sleep(Duration::from_secs(5));
        loop {
            annoncer(&app, false);
            std::thread::sleep(PERIODE);
        }
    });
}

/// Ouvre la page de la dernière version dans le navigateur du système.
pub fn ouvrir() -> Result<(), String> {
    let page = PAGE.lock().unwrap_or_else(|e| e.into_inner()).clone().ok_or("pas encore de version trouvée")?;
    if !page.starts_with(&format!("https://github.com/{DEPOT}/releases/")) {
        return Err("adresse inattendue".into());
    }
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(&page).spawn();
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", &page]).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(&page).spawn();
    r.map(|_| ()).map_err(|e| format!("navigateur du système introuvable ({e})"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_comparees() {
        assert_eq!(version("v0.1.7"), Some((0, 1, 7)));
        assert_eq!(version("0.2"), Some((0, 2, 0)));
        assert_eq!(version("v1.0.0-beta.2"), Some((1, 0, 0)));
        assert_eq!(version("pas une version"), None);
        assert!(version("v0.1.10") > version("0.1.9"), "10 > 9, pas l'ordre alphabétique");
    }

    #[test]
    fn reponse_de_github() {
        let v = serde_json::json!({
            "tag_name": "v0.1.8",
            "html_url": format!("https://github.com/{DEPOT}/releases/tag/v0.1.8"),
            "published_at": "2026-11-20T10:00:00Z",
            "body": "## Téléchargement\n| Système | Fichier |\n|---|---|\n| macOS | `V34.dmg` |\n\n## Nouveautés\n\n- Le **Minitel**\n- Le morse\n* L'orgue\n- Encore",
        });
        let m = analyser(&v, "0.1.7", false).unwrap();
        assert!(m.nouvelle && m.derniere == "0.1.8" && m.date == "2026-11-20");
        assert!(m.page.ends_with("/tag/v0.1.8"));
        assert_eq!(m.notes, "Le Minitel · Le morse · L'orgue", "ni titres ni tableau des téléchargements");
        // à jour, ou plus récent que la publication (version de développement)
        assert!(!analyser(&v, "0.1.8", false).unwrap().nouvelle);
        assert!(!analyser(&v, "0.2.0", false).unwrap().nouvelle);
        // page hors du dépôt : on ne la garde pas
        let v = serde_json::json!({"tag_name": "v0.1.8", "html_url": "https://exemple.org/piege"});
        assert_eq!(analyser(&v, "0.1.7", false).unwrap().page, format!("https://github.com/{DEPOT}/releases/latest"));
        assert!(analyser(&serde_json::json!({"message": "Not Found"}), "0.1.7", false).is_err());
    }
}
