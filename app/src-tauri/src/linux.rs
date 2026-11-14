//! Particularités de Linux : les vues web de Tauri y sont des widgets
//! WebKitGTK, rangés dans une boîte GTK. Rien de tout ceci ne sert ailleurs.
//!
//! - Mise en page du navigateur : Tauri ne place pas les vues web à des
//!   coordonnées, il les range l'une sous l'autre dans la boîte GTK verticale
//!   de la fenêtre (GtkBox), où chacune prend une part égale de la hauteur ;
//!   `set_position` / `set_size` n'y font rien. D'où la messagerie repoussée
//!   dans la moitié basse, sous une barre d'outils étirée sur l'autre moitié.
//!   On règle donc la boîte elle-même ([`caser`]) : barre d'outils et barre
//!   d'état à hauteur fixe, page ou messagerie qui prend tout le reste. Une
//!   vue cachée n'y occupe aucune place : `show` / `hide` suffisent pour
//!   changer de mode, et GTK suit seul les redimensionnements et l'échelle
//!   de l'écran (ses tailles sont en pixels logiques).
//! - Caméra du visiophone : voir [`activer_camera`].
//! - Plantages de WebKitGTK : voir [`fiabiliser`].

use std::sync::atomic::{AtomicU32, Ordering};

use gtk::prelude::*;
use webkit2gtk::{SettingsExt, WebProcessTerminationReason, WebView, WebViewExt};

/// `hauteur` : hauteur fixe en pixels logiques, ou `None` pour prendre la
/// place qui reste dans la fenêtre.
pub fn caser(w: &WebView, hauteur: Option<f64>) {
    w.set_size_request(-1, hauteur.map_or(-1, |h| h.round() as i32));
    if let Some(boite) = w.parent().and_then(|p| p.downcast::<gtk::Box>().ok()) {
        boite.set_child_packing(w, hauteur.is_none(), true, 0, gtk::PackType::Start);
    }
}

/// WebKitGTK n'offre `navigator.mediaDevices` (getUserMedia) que si le
/// réglage « media stream » est actif, ce que wry ne fait pas. La page est
/// rechargée s'il fallait l'activer : ce qu'elle expose est fixé à son
/// chargement. L'accord de la vue est donné par son gestionnaire de
/// permissions (`browser::camera`) ; WebKitGTK passe ensuite par le portail
/// « Caméra » du bureau, qui demande une fois à l'utilisateur (KDE :
/// « Allow Apps to Use the Camera? ») et retient sa réponse.
pub fn activer_camera(w: &WebView) {
    let Some(r) = WebViewExt::settings(w) else { return };
    // Tests : V34_CAMERA_FACTICE=1 remplace la webcam par la caméra de
    // démonstration de WebKit (une mire animée), sans filmer personne.
    if std::env::var_os("V34_CAMERA_FACTICE").is_some() {
        r.set_enable_mock_capture_devices(true);
    }
    if !r.enables_media_stream() {
        r.set_enable_media_stream(true);
        w.reload();
    }
}

/// Plantages du processus web de WebKitGTK (vu sous Ubuntu 26.04, Wayland,
/// avec l'AppImage 0.1.3) : le canevas accéléré par la carte graphique
/// (Skia « Ganesh ») corrompt son cache de textures quand la fenêtre du
/// modem redessine ses canevas (oscilloscope, constellation…) à 60 images/s
/// — assertion dans `GrResourceCache`, SIGILL. La fenêtre garde alors sa
/// dernière image pour toujours : elle paraît figée, alors que l'appli
/// continue de tourner.
///
/// Remède : canevas dessinés par le processeur (ce sont de petits
/// canevas ; la composition des calques reste accélérée), et, si le
/// processus web tombe malgré tout, la page est rechargée : l'état revient
/// avec le prochain `v34:state` (25 fois par seconde).
pub fn fiabiliser(w: &WebView, nom: &'static str) {
    if let Some(r) = WebViewExt::settings(w) {
        let fait = fonction_webkit(&r, "CanvasUsesAcceleratedDrawing", false);
        if std::env::var_os("V34_TRACE").is_some() {
            eprintln!("[v34] vue web « {nom} » : canevas accéléré coupé : {fait:?}");
        }
    }
    static RELANCES: AtomicU32 = AtomicU32::new(0);
    w.connect_web_process_terminated(move |w, raison| {
        eprintln!("[v34] vue web « {nom} » : processus web arrêté ({raison:?})");
        // Pas de boucle sans fin si la page plante à chaque chargement
        if raison != WebProcessTerminationReason::TerminatedByApi && RELANCES.fetch_add(1, Ordering::Relaxed) < 20 {
            eprintln!("[v34] vue web « {nom} » : rechargement");
            w.reload();
        }
    });
}

/// Active ou coupe une « fonction » de WebKit (préférence interne, liste de
/// `webkit_settings_get_all_features`, WebKitGTK 2.42 et plus). Ces
/// fonctions C sont cherchées à l'exécution : avec un WebKitGTK plus ancien,
/// l'appli démarre quand même (rien n'est changé). Renvoie l'état obtenu, ou
/// `None` si la fonction n'existe pas ici.
fn fonction_webkit(r: &webkit2gtk::Settings, identifiant: &str, actif: bool) -> Option<bool> {
    use webkit2gtk::glib::translate::ToGlibPtr;
    use std::ffi::{c_char, c_int, c_void, CStr};
    type Liste = unsafe extern "C" fn() -> *mut c_void;
    type Longueur = unsafe extern "C" fn(*mut c_void) -> usize;
    type Element = unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void;
    type Identifiant = unsafe extern "C" fn(*mut c_void) -> *const c_char;
    type Lire = unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int;
    type Ecrire = unsafe extern "C" fn(*mut c_void, *mut c_void, c_int);
    type Liberer = unsafe extern "C" fn(*mut c_void);
    // SAFETY : symboles de libwebkit2gtk-4.1 (déjà chargée), signatures de
    // l'API publique documentée ; la liste est libérée après usage, et les
    // éléments n'en sont lus que pendant qu'elle vit.
    unsafe {
        let sym = |nom: &CStr| {
            let p = libc::dlsym(libc::RTLD_DEFAULT, nom.as_ptr());
            (!p.is_null()).then_some(p)
        };
        let liste: Liste = std::mem::transmute(sym(c"webkit_settings_get_all_features")?);
        let longueur: Longueur = std::mem::transmute(sym(c"webkit_feature_list_get_length")?);
        let element: Element = std::mem::transmute(sym(c"webkit_feature_list_get")?);
        let ident: Identifiant = std::mem::transmute(sym(c"webkit_feature_get_identifier")?);
        let lire: Lire = std::mem::transmute(sym(c"webkit_settings_get_feature_enabled")?);
        let ecrire: Ecrire = std::mem::transmute(sym(c"webkit_settings_set_feature_enabled")?);
        let liberer: Liberer = std::mem::transmute(sym(c"webkit_feature_list_unref")?);
        let l = liste();
        if l.is_null() {
            return None;
        }
        let reglages: *mut webkit2gtk::ffi::WebKitSettings = r.to_glib_none().0;
        let mut etat = None;
        for i in 0..longueur(l) {
            let f = element(l, i);
            if f.is_null() {
                continue;
            }
            let id = ident(f);
            if !id.is_null() && CStr::from_ptr(id).to_bytes() == identifiant.as_bytes() {
                ecrire(reglages.cast(), f, actif as c_int);
                etat = Some(lire(reglages.cast(), f) != 0);
                break;
            }
        }
        liberer(l);
        etat
    }
}
