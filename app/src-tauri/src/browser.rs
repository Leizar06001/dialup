//! Navigateur intégré : une deuxième fenêtre avec plusieurs vues web.
//! - en haut, la barre d'outils rétro (page locale `navigateur.html`) ;
//! - en dessous, la page, dont TOUT le trafic passe par le proxy SOCKS5 du
//!   tunnel, donc par le modem (aucun autre accès réseau) ;
//! - tout en bas, la barre d'état (page locale `nav-statut.html`) : jauge
//!   de chargement d'après le compteur du proxy ;
//! - ou bien, à la place de la page et de la barre d'état, la messagerie
//!   (page locale `messagerie.html`) : chat, visiophone, fichiers, contacts.
//!
//! Trois modes :
//! - « web 1998 » : on passe par l'hôte virtuel `web1998` du FAI, qui va
//!   chercher la page et la renvoie allégée (HTML simple, GIF 256 couleurs) ;
//! - « moderne » : le vrai web, tel quel (lent !) ;
//! - « messagerie » : pas de web, l'appel en cours (entre abonnés : chat,
//!   visio, fichiers ; avec le FAI : son salon), et les contacts du central
//!   (l'annuaire reste aussi dans la fenêtre du modem).
//!
//! La page web n'a jamais accès aux commandes de l'appli (elle affiche des
//! sites quelconques) ; la messagerie, page locale, y a droit.

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::webview::{PageLoadEvent, WebviewBuilder};
use tauri::window::WindowBuilder;
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Url, WebviewUrl};

pub const WINDOW: &str = "navigateur";
pub const CHROME: &str = "nav-chrome";
pub const PAGE: &str = "nav-page";
pub const STATUS: &str = "nav-status";
pub const MESSAGERIE: &str = "nav-messagerie";
/// La messagerie est affichée (sinon la page web)
static VUE_MESSAGERIE: AtomicBool = AtomicBool::new(false);
/// La page web existe (voir [`check_platform`])
static WEB_OK: AtomicBool = AtomicBool::new(false);
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

/// Vue affichée et disponibilité du web, pour la barre d'outils.
#[derive(Clone, serde::Serialize)]
pub struct Vue {
    /// "messagerie" ou "web"
    pub vue: &'static str,
    /// La page web existe (macOS 14 ou plus récent)
    pub web: bool,
}

pub fn vue() -> Vue {
    Vue { vue: if VUE_MESSAGERIE.load(Ordering::Relaxed) { "messagerie" } else { "web" }, web: WEB_OK.load(Ordering::Relaxed) }
}

/// Passe de la page web à la messagerie (ou l'inverse) dans la fenêtre ouverte.
pub fn set_vue(app: &AppHandle, messagerie: bool) -> Result<Vue, String> {
    if !messagerie {
        check_platform()?;
    }
    VUE_MESSAGERIE.store(messagerie, Ordering::Relaxed);
    if let Some(win) = app.get_window(WINDOW) {
        layout(&win);
        let _ = win.set_title(titre(messagerie));
    }
    let v = vue();
    let _ = app.emit_to(CHROME, "nav:vue", v.clone());
    // La fenêtre du modem éteint le clignotement de la LED correspondante
    let _ = app.emit("navigateur:vu", v.vue);
    Ok(v)
}

fn titre(messagerie: bool) -> &'static str {
    if messagerie { "Tinyverse Messagerie — chat, visiophone, fichiers" } else { "Tinyverse Navigator — Internet par modem" }
}

/// Ouvre (ou ramène au premier plan) la fenêtre du navigateur, sur la
/// messagerie ou sur la page web. Sans le web (macOS antérieur à 14), la
/// fenêtre n'a que la messagerie.
pub fn open(app: &AppHandle, proxy_port: u16, messagerie: bool) -> Result<(), String> {
    let web_ok = check_platform();
    if !messagerie {
        web_ok.clone()?;
    }
    if let Some(w) = app.get_window(WINDOW) {
        set_vue(app, messagerie)?;
        let _ = w.unminimize();
        return w.set_focus().map_err(|e| e.to_string());
    }
    VUE_MESSAGERIE.store(messagerie, Ordering::Relaxed);
    WEB_OK.store(web_ok.is_ok(), Ordering::Relaxed);
    build(app, proxy_port, web_ok.is_ok()).map_err(|e| e.to_string())?;
    // Le premier passage au premier plan arrive avant `on_window_event` :
    // on prévient la fenêtre du modem ici.
    let _ = app.emit("navigateur:vu", vue().vue);
    Ok(())
}

fn build(app: &AppHandle, proxy_port: u16, web_ok: bool) -> tauri::Result<()> {
    let win = WindowBuilder::new(app, WINDOW)
        .title(titre(VUE_MESSAGERIE.load(Ordering::Relaxed)))
        .inner_size(1100.0, 820.0)
        .min_inner_size(640.0, 480.0)
        .build()?;
    let size = win.inner_size()?.to_logical::<f64>(win.scale_factor()?);
    let chrome = win.add_child(
        WebviewBuilder::new(CHROME, WebviewUrl::App("navigateur.html".into())),
        LogicalPosition::new(0.0, 0.0),
        LogicalSize::new(size.width, CHROME_H),
    )?;
    let messagerie = win.add_child(
        camera(WebviewBuilder::new(MESSAGERIE, WebviewUrl::App("messagerie.html".into()))),
        LogicalPosition::new(0.0, CHROME_H),
        LogicalSize::new(size.width, (size.height - CHROME_H).max(100.0)),
    )?;
    #[cfg(target_os = "linux")]
    {
        linux(&chrome, Some(CHROME_H));
        linux(&messagerie, None);
    }
    let _ = (chrome, messagerie);
    if web_ok {
        add_page(app, &win, proxy_port, size)?;
    }
    layout(&win);
    let (w2, a3) = (win.clone(), app.clone());
    win.on_window_event(move |ev| match ev {
        tauri::WindowEvent::Resized(_) | tauri::WindowEvent::ScaleFactorChanged { .. } => layout(&w2),
        // Au premier plan : ce qu'elle montre (web ou messagerie) a été vu,
        // les LED de la fenêtre du modem arrêtent de clignoter
        tauri::WindowEvent::Focused(true) => {
            let _ = a3.emit("navigateur:vu", vue().vue);
        }
        // Fenêtre fermée : la caméra du visiophone s'éteint avec elle
        tauri::WindowEvent::Destroyed => {
            if let Some(st) = a3.try_state::<crate::AppState>() {
                let _ = st.send(crate::engine::EngineCmd::VisioCamera(false));
            }
        }
        _ => {}
    });
    Ok(())
}

/// La page web (tout son trafic par le proxy du modem) et sa barre d'état.
fn add_page(app: &AppHandle, win: &tauri::Window, proxy_port: u16, size: LogicalSize<f64>) -> tauri::Result<()> {
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
    // Windows : WebView2 lance la page proxifiée avec une option
    // `--proxy-server`. Toutes les vues d'un même dossier de données doivent
    // avoir les mêmes options : sinon la création de celle-ci n'aboutit
    // jamais (fenêtre blanche, impossible à fermer, le reste de l'appli
    // tourne). La page a donc son propre dossier.
    #[cfg(windows)]
    let page = match app.path().app_local_data_dir() {
        Ok(d) => page.data_directory(d.join("navigateur")),
        Err(_) => page,
    };
    let page = win.add_child(page, LogicalPosition::new(0.0, CHROME_H), LogicalSize::new(size.width, (size.height - CHROME_H - STATUS_H).max(100.0)))?;
    let status = win.add_child(
        WebviewBuilder::new(STATUS, WebviewUrl::App("nav-statut.html".into())),
        LogicalPosition::new(0.0, (size.height - STATUS_H).max(0.0)),
        LogicalSize::new(size.width, STATUS_H),
    )?;
    #[cfg(target_os = "linux")]
    {
        linux(&page, None);
        linux(&status, Some(STATUS_H));
    }
    let _ = (page, status);
    Ok(())
}

/// Visiophone : la messagerie (page locale, jamais la page web) peut ouvrir
/// la caméra. Sous Linux, WebKitGTK refuse sinon toute demande (il n'a pas
/// de fenêtre pour demander l'accord) ; ailleurs, rien ne change : le
/// système demande lui-même.
fn camera(b: WebviewBuilder<tauri::Wry>) -> WebviewBuilder<tauri::Wry> {
    #[cfg(target_os = "linux")]
    let b = b.on_permission_request(|_wv, kind| {
        use tauri::webview::{PermissionKind, PermissionResponse};
        if matches!(kind, PermissionKind::Camera) {
            PermissionResponse::Allow
        } else {
            PermissionResponse::Default
        }
    });
    b
}

/// Sous Linux : place de la vue dans la fenêtre (hauteur fixe, ou tout le
/// reste : voir [`crate::linux`]), et, pour les pages locales, réglages
/// qui évitent les plantages de WebKitGTK ; la messagerie a la caméra.
#[cfg(target_os = "linux")]
fn linux(wv: &tauri::Webview, hauteur: Option<f64>) {
    let label = wv.label().to_string();
    let _ = wv.with_webview(move |pw| {
        let w = pw.inner();
        crate::linux::caser(&w, hauteur);
        let nom = match label.as_str() {
            CHROME => CHROME,
            STATUS => STATUS,
            MESSAGERIE => MESSAGERIE,
            _ => return, // la page web : aucun réglage de plus
        };
        crate::linux::fiabiliser(&w, nom);
        if nom == MESSAGERIE {
            crate::linux::activer_camera(&w);
        }
    });
}

/// Hauteur de la barre de titre à compenser. Sur macOS, la zone de contenu
/// passe SOUS la barre de titre (plein cadre) et WebKit décale le contenu de
/// la barre d'outils d'autant : la page doit commencer 28 pt plus bas.
fn titlebar(win: &tauri::Window) -> f64 {
    let _ = win;
    if cfg!(target_os = "macos") { 28.0 } else { 0.0 }
}

/// Recalcule la place des vues quand la fenêtre change de taille ou de
/// mode : la messagerie prend la place de la page et de la barre d'état.
fn layout(win: &tauri::Window) {
    let (Ok(sz), Ok(sf)) = (win.inner_size(), win.scale_factor()) else { return };
    let s = sz.to_logical::<f64>(sf);
    let tb = titlebar(win);
    let msg = VUE_MESSAGERIE.load(Ordering::Relaxed);
    for wv in win.webviews() {
        let shown = match wv.label() {
            CHROME => {
                let _ = wv.set_position(LogicalPosition::new(0.0, 0.0));
                let _ = wv.set_size(LogicalSize::new(s.width, tb + CHROME_H));
                true
            }
            MESSAGERIE => {
                let _ = wv.set_position(LogicalPosition::new(0.0, tb + CHROME_H));
                let _ = wv.set_size(LogicalSize::new(s.width, (s.height - tb - CHROME_H).max(100.0)));
                msg
            }
            PAGE => {
                let _ = wv.set_position(LogicalPosition::new(0.0, tb + CHROME_H));
                let _ = wv.set_size(LogicalSize::new(s.width, (s.height - tb - CHROME_H - STATUS_H).max(100.0)));
                !msg
            }
            STATUS => {
                // la hauteur intérieure compte déjà la barre de titre (macOS)
                let _ = wv.set_position(LogicalPosition::new(0.0, (s.height - STATUS_H).max(tb + CHROME_H)));
                let _ = wv.set_size(LogicalSize::new(s.width, STATUS_H));
                !msg
            }
            _ => continue,
        };
        let _ = if shown { wv.show() } else { wv.hide() };
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
    // (web1998 et tinyblog sont des hôtes du FAI : servis tels quels)
    let interne = ["http://web1998", "http://tinyblog"].iter().any(|h| with_scheme.to_ascii_lowercase().starts_with(h));
    if !modern && !interne {
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
        assert_eq!(resolve("tinyblog", "1998").unwrap().as_str(), "http://tinyblog/");
        assert_eq!(resolve("tinyblog/alice", "moderne").unwrap().as_str(), "http://tinyblog/alice");
    }
}
