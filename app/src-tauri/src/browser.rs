//! Navigateur intégré : une deuxième fenêtre avec deux vues web.
//! - en haut, la barre d'outils rétro (page locale `navigateur.html`) ;
//! - en dessous, la page, dont TOUT le trafic passe par le proxy SOCKS5 du
//!   tunnel, donc par le modem (aucun autre accès réseau) ;
//! - tout en bas, la barre d'état (page locale `nav-statut.html`) : jauge
//!   de chargement d'après le compteur du proxy.
//!
//! Deux modes :
//! - « web 1998 » : on passe par l'hôte virtuel `web1998` du FAI, qui va
//!   chercher la page et la renvoie allégée (HTML simple, GIF 256 couleurs) ;
//! - « moderne » : le vrai web, tel quel (lent !).

use tauri::webview::{PageLoadEvent, WebviewBuilder};
use tauri::window::WindowBuilder;
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Url, WebviewUrl};

pub const WINDOW: &str = "navigateur";
pub const CHROME: &str = "nav-chrome";
pub const PAGE: &str = "nav-page";
pub const STATUS: &str = "nav-status";
/// Hauteur de la barre d'outils (pixels logiques)
const CHROME_H: f64 = 100.0;
/// Hauteur de la barre d'état
const STATUS_H: f64 = 26.0;
/// Accueil du mode 1998 (servi par le FAI)
pub const HOME_1998: &str = "http://web1998/";
/// Accueil du mode moderne (moteur de recherche léger)
pub const HOME_MODERN: &str = "https://lite.duckduckgo.com/lite/";

/// Le navigateur intégré peut-il passer par le modem sur ce système ?
/// Sous macOS, le proxy par vue web n'existe qu'à partir de macOS 14 : avant,
/// la page irait sur Internet sans passer par le modem (ou l'appli
/// planterait). On refuse plutôt avec une explication.
pub fn check_platform() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let major = std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|v| v.trim().split('.').next()?.parse::<u32>().ok());
        if major.is_some_and(|m| m < 14) {
            return Err("le navigateur intégré demande macOS 14 (Sonoma) ou plus récent : sur ce système, ses pages ne pourraient pas passer par le modem. Le chat, les fichiers et le salon fonctionnent.".into());
        }
    }
    Ok(())
}

/// Ouvre (ou ramène au premier plan) la fenêtre du navigateur.
pub fn open(app: &AppHandle, proxy_port: u16) -> tauri::Result<()> {
    if let Some(w) = app.get_window(WINDOW) {
        let _ = w.unminimize();
        return w.set_focus();
    }
    let win = WindowBuilder::new(app, WINDOW)
        .title("Tinyverse Navigator — Internet par modem")
        .inner_size(1100.0, 820.0)
        .min_inner_size(640.0, 480.0)
        .build()?;
    let size = win.inner_size()?.to_logical::<f64>(win.scale_factor()?);
    win.add_child(
        WebviewBuilder::new(CHROME, WebviewUrl::App("navigateur.html".into())),
        LogicalPosition::new(0.0, 0.0),
        LogicalSize::new(size.width, CHROME_H),
    )?;
    let proxy = Url::parse(&format!("socks5://127.0.0.1:{proxy_port}")).expect("adresse de proxy");
    let (a1, a2) = (app.clone(), app.clone());
    let page = WebviewBuilder::new(PAGE, WebviewUrl::App("hors-ligne.html".into()))
        .proxy_url(proxy)
        .on_navigation(move |url| {
            let _ = a1.emit_to(CHROME, "nav:url", url.to_string());
            true
        })
        .on_page_load(move |_wv, payload| {
            let done = matches!(payload.event(), PageLoadEvent::Finished);
            let ev = serde_json::json!({ "url": payload.url().to_string(), "done": done });
            let _ = a2.emit_to(CHROME, "nav:load", ev.clone());
            let _ = a2.emit_to(STATUS, "nav:load", ev);
        });
    win.add_child(page, LogicalPosition::new(0.0, CHROME_H), LogicalSize::new(size.width, (size.height - CHROME_H - STATUS_H).max(100.0)))?;
    win.add_child(
        WebviewBuilder::new(STATUS, WebviewUrl::App("nav-statut.html".into())),
        LogicalPosition::new(0.0, (size.height - STATUS_H).max(0.0)),
        LogicalSize::new(size.width, STATUS_H),
    )?;
    layout(&win);
    let w2 = win.clone();
    win.on_window_event(move |ev| {
        if matches!(ev, tauri::WindowEvent::Resized(_) | tauri::WindowEvent::ScaleFactorChanged { .. }) {
            layout(&w2);
        }
    });
    Ok(())
}

/// Hauteur de la barre de titre à compenser. Sur macOS, la zone de contenu
/// passe SOUS la barre de titre (plein cadre) et WebKit décale le contenu de
/// la barre d'outils d'autant : la page doit commencer 28 pt plus bas.
fn titlebar(win: &tauri::Window) -> f64 {
    let _ = win;
    if cfg!(target_os = "macos") { 28.0 } else { 0.0 }
}

/// Recalcule la place des deux vues quand la fenêtre change de taille.
fn layout(win: &tauri::Window) {
    let (Ok(sz), Ok(sf)) = (win.inner_size(), win.scale_factor()) else { return };
    let s = sz.to_logical::<f64>(sf);
    let tb = titlebar(win);
    for wv in win.webviews() {
        if wv.label() == CHROME {
            let _ = wv.set_position(LogicalPosition::new(0.0, 0.0));
            let _ = wv.set_size(LogicalSize::new(s.width, tb + CHROME_H));
        } else if wv.label() == PAGE {
            let _ = wv.set_position(LogicalPosition::new(0.0, tb + CHROME_H));
            let _ = wv.set_size(LogicalSize::new(s.width, (s.height - tb - CHROME_H - STATUS_H).max(100.0)));
        } else if wv.label() == STATUS {
            // la hauteur intérieure compte déjà la barre de titre (macOS)
            let _ = wv.set_position(LogicalPosition::new(0.0, (s.height - STATUS_H).max(tb + CHROME_H)));
            let _ = wv.set_size(LogicalSize::new(s.width, STATUS_H));
        }
    }
}

/// Encodage « pourcent » d'une adresse placée en paramètre.
fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Adresse réellement chargée selon le mode (« 1998 » ou « moderne »).
pub fn resolve(input: &str, mode: &str) -> Option<Url> {
    let t = input.trim();
    let modern = mode == "moderne";
    if t.is_empty() || t.eq_ignore_ascii_case("accueil") {
        return Url::parse(if modern { HOME_MODERN } else { HOME_1998 }).ok();
    }
    let with_scheme = if t.contains("://") { t.to_string() } else { format!("http://{t}") };
    if !modern && !with_scheme.to_ascii_lowercase().starts_with("http://web1998") {
        return Url::parse(&format!("{HOME_1998}p?u={}", encode(&with_scheme))).ok();
    }
    Url::parse(&with_scheme).ok()
}

/// Adresse « réelle » à afficher : en mode 1998, on décode /p?u=…
pub fn display_url(loaded: &str) -> String {
    if let Ok(u) = Url::parse(loaded) {
        if u.host_str() == Some("web1998") {
            if let Some((_, v)) = u.query_pairs().find(|(k, _)| k == "u") {
                return v.into_owned();
            }
        }
    }
    loaded.to_string()
}

pub fn navigate(app: &AppHandle, input: &str, mode: &str) -> Result<(), String> {
    let url = resolve(input, mode).ok_or_else(|| format!("adresse invalide : {input}"))?;
    let page = app.get_webview(PAGE).ok_or("navigateur fermé")?;
    page.navigate(url).map_err(|e| e.to_string())
}

pub fn command(app: &AppHandle, cmd: &str) -> Result<(), String> {
    let page = app.get_webview(PAGE).ok_or("navigateur fermé")?;
    let js = match cmd {
        "back" => "history.back()",
        "forward" => "history.forward()",
        "reload" => "location.reload()",
        "stop" => "window.stop()",
        _ => return Err(format!("commande inconnue : {cmd}")),
    };
    page.eval(js).map_err(|e| e.to_string())
}

/// Page hors-ligne (quand le tunnel n'est pas établi).
pub fn show_offline(app: &AppHandle) {
    if let Some(page) = app.get_webview(PAGE) {
        if let Ok(u) = page.url() {
            if u.path().ends_with("hors-ligne.html") {
                return;
            }
        }
        let _ = page.eval("location.href = '/hors-ligne.html'");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_addresses_per_mode() {
        assert_eq!(resolve("", "1998").unwrap().as_str(), HOME_1998);
        assert_eq!(resolve("fr.wikipedia.org/wiki/Modem", "1998").unwrap().as_str(), "http://web1998/p?u=http%3A%2F%2Ffr.wikipedia.org%2Fwiki%2FModem");
        assert_eq!(resolve("example.com", "moderne").unwrap().as_str(), "http://example.com/");
        assert_eq!(resolve("https://a.b/c", "moderne").unwrap().as_str(), "https://a.b/c");
        assert_eq!(display_url("http://web1998/p?u=http%3A%2F%2Fexample.com%2F"), "http://example.com/");
    }
}
