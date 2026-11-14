//! V34 Modem — interface rétro (Tauri v2).
//!
//! Le moteur temps réel tourne dans son propre thread (`engine`) ; les
//! commandes de l'IHM lui sont transmises par un canal, et il renvoie
//! l'état par des événements Tauri (`v34:state`, `v34:event`,
//! `v34:console`, `v34:chat`, `v34:file`, `v34:visio`).
//!
//! Le chat, les fichiers et le visiophone vivent dans la
//! messagerie de la fenêtre du navigateur ([`browser`]) ; elle peut s'ouvrir
//! en cours d'appel : un journal garde les messages reçus depuis le lancement.

// Pas de console noire derrière la fenêtre sous Windows (en release).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod bbs;
mod browser;
mod engine;
mod fft;
mod link;
#[cfg(target_os = "linux")]
mod linux;
mod tones;
mod visio;

use std::collections::VecDeque;
use std::sync::mpsc::{channel, Sender};
use std::sync::Mutex;

use serde::Serialize;
use tauri::State;
use tauri::{Emitter, Listener, Manager};
use v34_core::{LineParams, ModemCommand};

use audio::Speaker;
use engine::{Engine, EngineCmd};

struct AppState {
    tx: Mutex<Sender<EngineCmd>>,
    speaker: Speaker,
}

impl AppState {
    fn send(&self, cmd: EngineCmd) -> Result<(), String> {
        self.tx
            .lock()
            .map_err(|_| "moteur indisponible".to_string())?
            .send(cmd)
            .map_err(|_| "le moteur s'est arrêté".to_string())
    }
}

/// Ligne de la console AT (ex. "ATDT0123456789", "ATM2L3", "ATH").
#[tauri::command]
fn at_command(state: State<AppState>, line: String) -> Result<(), String> {
    state.send(EngineCmd::At(line))
}

/// Commande directe (boutons) : {"type":"Retrain"}, {"type":"Hangup"}…
#[tauri::command]
fn modem_command(state: State<AppState>, cmd: ModemCommand) -> Result<(), String> {
    state.send(EngineCmd::Modem(cmd))
}

#[tauri::command]
fn set_line(state: State<AppState>, params: LineParams) -> Result<(), String> {
    state.send(EngineCmd::SetLine(params))
}

#[derive(Serialize)]
struct Preset {
    name: String,
    params: LineParams,
}

#[tauri::command]
fn line_presets() -> Vec<Preset> {
    LineParams::presets()
        .into_iter()
        .map(|(name, params)| Preset { name: name.to_string(), params })
        .collect()
}

#[tauri::command]
fn set_speaker(
    state: State<AppState>,
    mode: Option<u8>,
    level: Option<u8>,
    volume: Option<f32>,
) -> Result<(), String> {
    state.send(EngineCmd::Speaker { mode, level, volume })
}

#[tauri::command]
fn send_chat(state: State<AppState>, text: String) -> Result<(), String> {
    state.send(EngineCmd::Chat(text))
}

/// Mode réseau : connexion au central (dialup.tinyverse.fr par défaut).
#[tauri::command]
fn net_connect(state: State<AppState>, server: String, user: String, password: String) -> Result<(), String> {
    if user.trim().is_empty() || password.is_empty() {
        return Err("nom d'abonné et mot de passe requis".into());
    }
    state.send(EngineCmd::NetConnect { server: server.trim().to_string(), user: user.trim().to_string(), password })
}

/// Rafraîchir l'annuaire du central.
#[tauri::command]
fn net_directory(state: State<AppState>) -> Result<(), String> {
    state.send(EngineCmd::NetDirectory)
}

/// Retour en boucle locale.
#[tauri::command]
fn net_local(state: State<AppState>) -> Result<(), String> {
    state.send(EngineCmd::NetLocal)
}

/// Ouvre la fenêtre du navigateur (trafic par le proxy du modem), sur la
/// page web ou, avec `mode` = "messagerie", sur le chat et le visiophone.
/// Asynchrone : sous Windows, créer une fenêtre WebView2 depuis une commande
/// synchrone (exécutée sur le fil principal) bloque l'appli — fenêtre blanche
/// impossible à fermer.
#[tauri::command]
async fn open_browser(app: tauri::AppHandle, mode: Option<String>) -> Result<(), String> {
    browser::open(&app, engine::PROXY_PORT, mode.as_deref() == Some("messagerie"))
}

/// Barre d'outils du navigateur : passer à la messagerie ("messagerie") ou
/// au web (autre chose) ; sans argument, la vue actuelle.
#[tauri::command]
fn nav_vue(app: tauri::AppHandle, vue: Option<String>) -> Result<browser::Vue, String> {
    match vue {
        Some(v) => browser::set_vue(&app, v == "messagerie"),
        None => Ok(browser::vue()),
    }
}

/// Journal de la messagerie : les derniers messages du chat (numérotés) et
/// le dernier état des transferts, pour une fenêtre ouverte en cours d'appel.
#[derive(Default, Serialize, Clone)]
struct Journal {
    chat: VecDeque<serde_json::Value>,
    file: Option<serde_json::Value>,
    #[serde(skip)]
    seq: u64,
}

/// Messages gardés dans le journal
const JOURNAL_MAX: usize = 400;

#[tauri::command]
fn messagerie_historique(journal: State<Mutex<Journal>>) -> Journal {
    journal.lock().map(|j| j.clone()).unwrap_or_default()
}

/// Tient le journal : chaque message du chat (`v34:chat`) y entre avec un
/// numéro, et repart vers la messagerie (`messagerie:chat`) avec ce numéro,
/// qui lui évite les doublons avec l'historique.
fn tenir_journal(app: &tauri::AppHandle) {
    let h = app.clone();
    app.listen_any("v34:chat", move |ev| {
        let Ok(mut msg) = serde_json::from_str::<serde_json::Value>(ev.payload()) else { return };
        {
            let st = h.state::<Mutex<Journal>>();
            let Ok(mut j) = st.lock() else { return };
            j.seq += 1;
            msg["seq"] = j.seq.into();
            j.chat.push_back(msg.clone());
            while j.chat.len() > JOURNAL_MAX {
                j.chat.pop_front();
            }
        }
        let r = h.emit_to(browser::MESSAGERIE, "messagerie:chat", &msg);
        if std::env::var_os("V34_TRACE").is_some() {
            let ouverte = h.get_webview(browser::MESSAGERIE).is_some();
            let seq = &msg["seq"];
            match r {
                Ok(()) => eprintln!("[{:>6}] chat n°{seq} → messagerie ({})", engine::trace_ms(), if ouverte { "fenêtre ouverte" } else { "fermée : gardé dans le journal" }),
                Err(e) => eprintln!("[{:>6}] chat n°{seq} → messagerie : ÉCHEC {e}", engine::trace_ms()),
            }
        }
    });
    let h = app.clone();
    app.listen_any("v34:file", move |ev| {
        if let (Ok(v), Ok(mut j)) = (serde_json::from_str::<serde_json::Value>(ev.payload()), h.state::<Mutex<Journal>>().lock()) {
            j.file = Some(v);
        }
    });
}

#[tauri::command]
fn nav_go(app: tauri::AppHandle, input: String, mode: String) -> Result<(), String> {
    browser::navigate(&app, &input, &mode)
}

#[tauri::command]
fn nav_cmd(app: tauri::AppHandle, cmd: String) -> Result<(), String> {
    browser::command(&app, &cmd)
}

#[tauri::command]
fn nav_display_url(url: String) -> String {
    browser::display_url(&url)
}

/// Démo : V34_AUTOBROWSER="moderne|example.com" ouvre le navigateur avec
/// ce mode et cette adresse.
#[tauri::command]
fn nav_initial() -> Option<(String, String)> {
    let v = std::env::var("V34_AUTOBROWSER").ok()?;
    let (m, u) = v.split_once('|').unwrap_or(("1998", v.as_str()));
    Some((m.to_string(), u.to_string()))
}

#[tauri::command]
fn nav_offline(app: tauri::AppHandle) {
    browser::show_offline(&app);
}

/// Échelle de temps de la simulation : 0,1 = ralenti ×10, 5 = accéléré ×5.
#[tauri::command]
fn set_speed(state: State<AppState>, speed: f64) -> Result<(), String> {
    state.send(EngineCmd::Speed(speed))
}

/// Envoi d'un fichier (contenu lu par l'IHM). Limité à 8 Mo : à 33 600 bit/s
/// c'est déjà plus d'une demi-heure de transfert.
#[tauri::command]
fn send_file(state: State<AppState>, name: String, data: Vec<u8>) -> Result<(), String> {
    if data.len() > 8 << 20 {
        return Err(format!("« {name} » est trop gros ({:.1} Mo, max 8 Mo) — c'est un modem 33,6k !", data.len() as f64 / 1048576.0));
    }
    state.send(EngineCmd::SendFile(name, data))
}

/// Visiophone : caméra (ou mire) allumée ou éteinte.
#[tauri::command]
fn visio_camera(state: State<AppState>, on: bool) -> Result<(), String> {
    state.send(EngineCmd::VisioCamera(on))
}

/// Visiophone : une image de la caméra (RGBA, `w` × `h`), hallucinée ici
/// (quelques millisecondes, hors du fil principal) puis confiée au moteur
/// si `send` (il ne l'envoie que si la ligne le permet). Renvoie l'aperçu :
/// exactement ce que verra le correspondant.
#[tauri::command]
async fn visio_frame(state: State<'_, AppState>, rgba: Vec<u8>, w: u32, h: u32, send: bool) -> Result<visio::Encoded, String> {
    let enc = visio::encode_rgba(&rgba, w, h)?;
    if send {
        state.send(EngineCmd::VisioFrame(enc.wire.clone()))?;
    }
    Ok(enc)
}

/// Panneau CONFIG : interrupteurs DIP 1 (56K V.90) et 2 (réponse
/// automatique), molette DÉBIT MAX (bit/s). Pris en compte au prochain
/// appel ; renvoie la configuration obtenue, que le panneau affiche.
#[tauri::command]
fn set_modem_config(state: State<AppState>, v90: bool, max_rate: u32, auto_answer: Option<bool>, express: Option<bool>) -> Result<v34_core::ModemConfig, String> {
    if !(2400..=56000).contains(&max_rate) {
        return Err(format!("débit max hors limites : {max_rate} bit/s (2 400 à 56 000)"));
    }
    let cfg = engine::modem_config(v90, max_rate, express.unwrap_or(false));
    state.send(EngineCmd::SetConfig(cfg.clone(), auto_answer.unwrap_or(true)))?;
    Ok(cfg)
}

#[derive(Serialize)]
struct AppInfo {
    version: &'static str,
    audio_device: Option<String>,
    /// Connexion automatique au central au lancement (V34_NOAUTONET=1 :
    /// non, pour les tests ; V34_NET choisit déjà le mode)
    autonet: bool,
    /// Démo / tests : V34_VISIO=mire (ou cam) allume le visiophone à
    /// l'ouverture de la messagerie
    visio: Option<String>,
}

/// La liste des succès (titres, descriptions, cachés ou non).
#[tauri::command]
fn succes_liste() -> &'static [v34_net::succes::Succes] {
    v34_net::succes::LISTE
}

#[tauri::command]
fn app_info(webview: tauri::Webview, state: State<AppState>) -> AppInfo {
    // Une seule fois : si la fenêtre du modem est rechargée (vue web
    // relancée après un plantage), elle ne se reconnecte pas au central.
    static DEJA: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let premier = webview.label() == "main" && !DEJA.swap(true, std::sync::atomic::Ordering::Relaxed);
    let autonet = premier && std::env::var_os("V34_NOAUTONET").is_none() && std::env::var_os("V34_NET").is_none();
    AppInfo { version: env!("CARGO_PKG_VERSION"), audio_device: state.speaker.device_name(), autonet, visio: std::env::var("V34_VISIO").ok() }
}

/// Taille initiale : la plus grande fenêtre 16:10 qui tient dans ~90 % de
/// l'écran, centrée.
fn fit_to_screen(win: &tauri::WebviewWindow) {
    let Ok(Some(mon)) = win.current_monitor() else { return };
    let sf = mon.scale_factor();
    let screen = mon.size().to_logical::<f64>(sf);
    let origin = mon.position().to_logical::<f64>(sf);
    let (mut w, mut h) = (screen.width * 0.94, screen.height * 0.86);
    if w / h > 1.6 {
        w = h * 1.6;
    } else {
        h = w / 1.6;
    }
    let (mut w, mut h) = (w.max(1024.0), h.max(700.0));
    // Tests : V34_TAILLE=1024x700 impose la taille (mise en page des petits écrans)
    if let Some((tw, th)) = std::env::var("V34_TAILLE").ok().and_then(|v| v.split_once('x').and_then(|(a, b)| Some((a.parse::<f64>().ok()?, b.parse::<f64>().ok()?)))) {
        (w, h) = (tw.max(1024.0), th.max(700.0));
    }
    let _ = win.set_size(tauri::LogicalSize::new(w, h));
    // Position calculée nous-mêmes : `center()` utiliserait l'ancienne taille
    // tant que le redimensionnement n'est pas appliqué.
    let x = origin.x + ((screen.width - w) / 2.0).max(0.0);
    let y = origin.y + ((screen.height - h) / 2.0).max(0.0);
    let _ = win.set_position(tauri::LogicalPosition::new(x, y));
}

fn main() {
    let speaker = Speaker::start();
    let (tx, rx) = channel::<EngineCmd>();
    let engine_speaker = speaker.clone();

    tauri::Builder::default()
        .manage(AppState { tx: Mutex::new(tx), speaker })
        .manage(Mutex::new(Journal::default()))
        .setup(move |app| {
            tenir_journal(app.handle());
            let handle = app.handle().clone();
            std::thread::Builder::new()
                .name("v34-engine".into())
                .spawn(move || Engine::new(handle, engine_speaker).run(rx))?;
            if let Some(win) = app.get_webview_window("main") {
                fit_to_screen(&win);
                // Linux : canevas sans la carte graphique, rechargement si
                // le processus web plante (voir `linux::fiabiliser`)
                #[cfg(target_os = "linux")]
                let _ = win.with_webview(|pw| linux::fiabiliser(&pw.inner(), "main"));
            }
            if std::env::var("V34_AUTOBROWSER").is_ok() {
                let h = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    let h2 = h.clone();
                    let _ = h.run_on_main_thread(move || {
                        let _ = browser::open(&h2, engine::PROXY_PORT, false);
                    });
                });
            }
            // Tests : V34_UIEVAL="<js>" exécute ce JavaScript dans la fenêtre
            // principale 3 s après le lancement (simuler un clic, etc.).
            if let Ok(js) = std::env::var("V34_UIEVAL") {
                let h = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    if let Some(w) = h.get_webview_window("main") {
                        let _ = w.eval(&js);
                    }
                });
            }
            Ok(())
        })
        // Fermer la fenêtre du modem quitte tout le programme : le navigateur
        // et la messagerie ne restent pas seuls à l'écran.
        .on_window_event(|win, ev| {
            if win.label() == "main" && matches!(ev, tauri::WindowEvent::Destroyed) {
                win.app_handle().exit(0);
            }
        })
        .invoke_handler(tauri::generate_handler![
            at_command,
            modem_command,
            set_line,
            line_presets,
            set_speaker,
            send_chat,
            send_file,
            set_speed,
            set_modem_config,
            visio_camera,
            visio_frame,
            nav_vue,
            messagerie_historique,
            open_browser,
            net_connect,
            net_local,
            net_directory,
            nav_go,
            nav_cmd,
            nav_display_url,
            nav_offline,
            nav_initial,
            succes_liste,
            app_info
        ])
        .run(tauri::generate_context!())
        .expect("erreur au lancement de l'application");
}
