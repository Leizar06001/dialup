//! # Navigateur « web 1998 » : l'hôte virtuel `http://web1998/`
//!
//! À 2-4 Ko/s, le web moderne est inutilisable par modem. En « mode 1998 »,
//! le navigateur de l'appli passe par le proxy du tunnel (SOCKS5 ou HTTP
//! CONNECT sur 127.0.0.1:1080) et demande l'hôte VIRTUEL `web1998`, port 80.
//! Le bout serveur du tunnel ne l'ouvre pas sur Internet : il le sert
//! lui-même avec un petit serveur HTTP/1.0 interne qui récupère les vraies
//! pages et les allège.
//!
//! | Chemin | Contenu |
//! |---|---|
//! | `/` | accueil « Tinyverse 1998 » : compteur de visites, sites conseillés, champ « Aller à » |
//! | `/p?u=URL` | la page `URL`, simplifiée en HTML 3.2 ([`html`]) avec un bandeau (poids avant/après, lien vers l'original) ; liens réécrits vers `/p`, images vers `/i` |
//! | `/f?u=ACTION&champs…` | envoi d'un formulaire GET d'une page simplifiée (moteurs de recherche) |
//! | `/i?u=URL` | l'image `URL` réduite (≤ 320 px de large, ≤ 256 couleurs) en GIF entrelacé d'environ 15 Ko ([`images`]) |
//! | `/construction.gif`, `/compteur.gif?n=…` | images de la page d'accueil |
//!
//! Le champ « Aller à » accepte une adresse complète, un nom de domaine
//! (`exemple.fr`, complété en `http://`) ou des mots-clés (recherche
//! FrogFind). Erreurs (adresse interdite, délai dépassé, site introuvable,
//! 404…) : pages rétro. Pages et images allégées sont gardées dans un cache
//! mémoire LRU borné (10 min).
//!
//! ## Utilisation
//!
//! Rien à faire de plus que de lancer le tunnel : [`TunnelPolicy::from_config`],
//! [`TunnelPolicy::from_configs`] et `TunnelPolicy::default()` créent le
//! service (section `[web1998]` de la configuration, active par défaut), et
//! [`tunnel::run`] / [`tunnel::connect`] interceptent l'hôte virtuel :
//!
//! ```no_run
//! use std::sync::{mpsc, Arc};
//! use v34_server::tunnel::{self, TunnelPolicy};
//!
//! let policy = Arc::new(TunnelPolicy::default()); // web 1998 compris
//! assert!(policy.web1998.is_some());
//! let (_to_server, from_modem) = mpsc::sync_channel::<Vec<u8>>(4);
//! let (to_modem, _from_server) = mpsc::sync_channel::<Vec<u8>>(4);
//! // bloquant, jusqu'à la fin de la liaison :
//! let fin = tunnel::run(policy, "FAI local".into(), "alice", from_modem, to_modem);
//! ```
//!
//! Hors tunnel : [`Web1998::open_stream`] rend une socket TCP reliée au
//! serveur interne (c'est ce que fait le tunnel), [`Web1998::serve`] sert
//! une connexion HTTP déjà ouverte, et [`Web1998::handle`] traite une
//! requête sans réseau local (pratique pour les tests).
//!
//! ## Sécurité
//!
//! L'hôte virtuel lui-même ne passe pas par le filtre du tunnel (rien ne
//! sort : c'est une paire de sockets sur la boucle locale, sans port en
//! écoute permanent). Les récupérations, elles, y passent toutes : `http`
//! et `https` seulement, ports 80 et 443 seulement, adresse vérifiée après
//! résolution DNS par [`crate::filter`] et `fai.reseaux_interdits`, chaque
//! redirection revérifiée (voir `fetch`). `fai.autoriser_prive` ne
//! s'applique PAS ici : le réseau local du serveur reste toujours fermé.
//!
//! [`TunnelPolicy::from_config`]: crate::tunnel::TunnelPolicy::from_config
//! [`TunnelPolicy::from_configs`]: crate::tunnel::TunnelPolicy::from_configs
//! [`tunnel::run`]: crate::tunnel::run
//! [`tunnel::connect`]: crate::tunnel::connect

mod cache;
mod fetch;
pub mod html;
pub mod images;
mod pages;

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use url::form_urlencoded;
use url::Url;

use crate::config;
use crate::filter::Cidr;
use crate::debug;
use cache::Lru;
use fetch::{FetchError, Fetched, Fetcher};
use images::GifOptions;

pub use fetch::MAX_REDIRECTS;

/// Nom de l'hôte virtuel.
pub const HOST: &str = "web1998";
/// Port de l'hôte virtuel.
pub const PORT: u16 = 80;

/// L'hôte et le port désignent-ils l'hôte virtuel ? (`web1998`, casse
/// indifférente, point final toléré ; port 80 seulement)
pub fn is_virtual_host(host: &str, port: u16) -> bool {
    port == PORT && host.strip_suffix('.').unwrap_or(host).eq_ignore_ascii_case(HOST)
}

const ACCEPT_PAGE: &str = "text/html,application/xhtml+xml;q=0.9,text/plain;q=0.8,*/*;q=0.5";
const ACCEPT_IMAGE: &str = "image/png,image/jpeg,image/gif,image/webp;q=0.9,image/bmp;q=0.5";
const CACHE_TTL: Duration = Duration::from_secs(600);
/// Taille maximale de l'en-tête d'une requête du navigateur.
const MAX_REQUEST: usize = 16 * 1024;

/// Limites du service (section `[web1998]` de la configuration).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Taille maximale d'une page HTML téléchargée (octets ; au-delà, tronquée)
    pub max_page: usize,
    /// Taille maximale d'une image téléchargée (octets ; au-delà, refusée)
    pub max_image: usize,
    /// Largeur maximale des images converties (px)
    pub image_width: u32,
    /// Poids visé d'une image convertie (octets)
    pub image_target: usize,
    /// Délai maximal d'une récupération, redirections comprises
    pub timeout: Duration,
    /// Taille du cache mémoire (octets, 0 = sans cache)
    pub cache: usize,
}

impl Limits {
    pub fn from_config(c: &config::Web1998) -> Limits {
        Limits {
            max_page: c.page_max_ko as usize * 1024,
            max_image: c.image_max_ko as usize * 1024,
            image_width: c.largeur_image_max,
            image_target: c.image_cible_ko as usize * 1024,
            timeout: Duration::from_secs(c.delai_s.max(1)),
            cache: c.cache_mo as usize * 1024 * 1024,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Limits::from_config(&config::Web1998::default())
    }
}

/// Réponse HTTP du service.
#[derive(Clone, Debug)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Arc<Vec<u8>>,
    /// Durée de cache annoncée au navigateur (s ; 0 = ne pas garder)
    pub max_age: u32,
    /// Redirection (en-tête Location)
    pub location: Option<String>,
}

const HTML: &str = "text/html; charset=utf-8";
const GIF: &str = "image/gif";
const TEXT: &str = "text/plain; charset=utf-8";

impl Response {
    fn new(status: u16, content_type: &'static str, body: Vec<u8>, max_age: u32) -> Response {
        Response { status, content_type, body: Arc::new(body), max_age, location: None }
    }

    fn html(status: u16, page: String, max_age: u32) -> Response {
        Response::new(status, HTML, page.into_bytes(), max_age)
    }

    fn redirect(to: String) -> Response {
        let mut r = Response::new(302, TEXT, b"redirection\n".to_vec(), 0);
        r.location = Some(to);
        r
    }

    fn error(code: u16, title: &str, message: &str, original: Option<&Url>) -> Response {
        Response::html(code, pages::error(code, title, message, original, None), 0)
    }

    /// Erreur pour une image : corps minimal (le navigateur affiche le
    /// texte alternatif), inutile de payer une page entière au modem.
    fn image_error(code: u16, message: &str) -> Response {
        Response::new(code, TEXT, format!("{message}\n").into_bytes(), 0)
    }
}

fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        302 => "Found",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        504 => "Gateway Timeout",
        _ => "Erreur",
    }
}

/// Le service web 1998 (un seul par serveur, partagé par tous les appels :
/// cache et compteur de visites communs).
pub struct Web1998 {
    limits: Limits,
    fetcher: Fetcher,
    cache: Mutex<Lru<Response>>,
    visits: AtomicU64,
}

impl std::fmt::Debug for Web1998 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Web1998").field("limits", &self.limits).field("visits", &self.visits()).finish_non_exhaustive()
    }
}

impl Web1998 {
    /// `denied` : réseaux interdits en plus des adresses privées et
    /// locales (normalement `fai.reseaux_interdits`).
    pub fn new(limits: Limits, denied: Vec<Cidr>) -> Web1998 {
        Self::build(limits, denied, Vec::new())
    }

    fn build(limits: Limits, denied: Vec<Cidr>, local_ports: Vec<u16>) -> Web1998 {
        Web1998 {
            limits,
            fetcher: Fetcher::new(denied, local_ports),
            cache: Mutex::new(Lru::new(limits.cache, CACHE_TTL)),
            visits: AtomicU64::new(0),
        }
    }

    /// TESTS UNIQUEMENT (compilé seulement pour les tests du crate) :
    /// autorise les récupérations vers 127.0.0.1 sur ces ports, pour servir
    /// des pages d'un petit serveur local sans dépendre d'Internet.
    #[cfg(test)]
    pub(crate) fn for_tests(limits: Limits, local_ports: Vec<u16>) -> Web1998 {
        Self::build(limits, Vec::new(), local_ports)
    }

    /// Service selon la configuration (`None` si `actif = false`).
    pub fn from_config(c: &config::Web1998, denied: Vec<Cidr>) -> Option<Arc<Web1998>> {
        c.actif.then(|| Arc::new(Web1998::new(Limits::from_config(c), denied)))
    }

    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// Visites de la page d'accueil depuis le démarrage.
    pub fn visits(&self) -> u64 {
        self.visits.load(Ordering::Relaxed)
    }

    /// Ouvre une connexion vers le serveur HTTP interne : renvoie le bout
    /// « navigateur » d'une paire de sockets TCP locales ; l'autre bout est
    /// servi dans un thread ([`Web1998::serve`]). C'est ce que fait le
    /// tunnel quand l'abonné ouvre `web1998:80`.
    pub fn open_stream(self: &Arc<Self>) -> io::Result<TcpStream> {
        let (client, server) = tcp_pair()?;
        let me = self.clone();
        std::thread::Builder::new().name("web1998".into()).spawn(move || me.serve(server))?;
        Ok(client)
    }

    /// Sert une connexion HTTP (une requête, puis fermeture, comme un
    /// serveur HTTP/1.0). Bloquant.
    pub fn serve(&self, mut s: TcpStream) {
        let _ = s.set_read_timeout(Some(Duration::from_secs(30)));
        let _ = s.set_write_timeout(Some(Duration::from_secs(600)));
        let resp = match read_request(&mut s) {
            Some((method, target)) if method == "GET" || method == "HEAD" => {
                let r = self.handle(&target);
                debug!("web1998 : {method} {target} → {} ({} o)", r.status, r.body.len());
                Some((r, method == "HEAD"))
            }
            Some((method, _)) => Some((Response::error(501, "Méthode non gérée", &format!("La méthode {method} n'est pas gérée par ce serveur."), None), false)),
            None => None,
        };
        if let Some((r, head_only)) = resp {
            let _ = write_response(&mut s, &r, head_only);
        }
        let _ = s.shutdown(Shutdown::Write);
        // vider ce que le navigateur aurait encore envoyé : fermer avec des
        // données non lues provoquerait un RST qui peut tronquer la réponse
        let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
        let mut buf = [0u8; 4096];
        while matches!(s.read(&mut buf), Ok(n) if n > 0) {}
    }

    /// Traite une requête GET (`target` : chemin et paramètres, ou URL
    /// absolue `http://web1998/…`).
    pub fn handle(&self, target: &str) -> Response {
        let target = match target.strip_prefix("http://").or_else(|| target.strip_prefix("HTTP://")) {
            Some(rest) => rest.find('/').map_or("/", |i| &rest[i..]),
            None => target,
        };
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let param = |name: &str| form_urlencoded::parse(query.as_bytes()).find(|(k, _)| k == name).map(|(_, v)| v.into_owned());
        match path {
            "/" | "/index.html" | "/index.htm" => {
                let n = self.visits.fetch_add(1, Ordering::Relaxed) + 1;
                Response::html(200, pages::home(n), 0)
            }
            "/construction.gif" => {
                static GIF_CONSTRUCTION: OnceLock<Vec<u8>> = OnceLock::new();
                Response::new(200, GIF, GIF_CONSTRUCTION.get_or_init(images::construction_gif).clone(), 86_400)
            }
            "/compteur.gif" => Response::new(200, GIF, images::counter_gif(&param("n").unwrap_or_default()), 86_400),
            "/p" => match param("u").map(|u| user_url(&u)) {
                Some(Ok(url)) => self.page(url),
                Some(Err(())) | None => Response::error(400, "Adresse invalide", "Tapez une adresse comme « fr.wikipedia.org » ou « lite.cnn.com », ou des mots-clés à chercher.", None),
            },
            "/f" => match form_target(query) {
                Some(url) => self.page(url),
                None => Response::error(400, "Formulaire invalide", "Ce formulaire ne peut pas être envoyé.", None),
            },
            "/i" => match param("u").and_then(|u| Url::parse(u.trim()).ok()) {
                Some(url) => self.image(url),
                None => Response::image_error(400, "adresse d'image invalide"),
            },
            "/favicon.ico" => Response::image_error(404, "pas d'icône"),
            _ => Response::error(404, "Fichier introuvable", "L'URL demandée n'a pas été trouvée sur ce serveur. Le webmestre a peut-être déménagé la page (ou elle est encore en construction).", None),
        }
    }

    fn cached(&self, key: &str) -> Option<Response> {
        self.cache.lock().ok()?.get(key)
    }

    fn store(&self, key: String, r: &Response) {
        if r.status == 200 {
            if let Ok(mut c) = self.cache.lock() {
                c.put(key, r.clone(), r.body.len() + 256);
            }
        }
    }

    /// `/p` : page simplifiée.
    fn page(&self, url: Url) -> Response {
        if let Some(local) = virtual_path(&url) {
            return Response::redirect(local);
        }
        let key = format!("p {url}");
        if let Some(r) = self.cached(&key) {
            return r;
        }
        let t0 = Instant::now();
        let resp = match self.fetcher.fetch(&url, ACCEPT_PAGE, self.limits.max_page, true, self.limits.timeout) {
            Ok(f) => self.render(f),
            Err(e) => fetch_error(&e, &url, &self.limits),
        };
        debug!("web1998 : page {url} → {} ({} o, {} ms)", resp.status, resp.body.len(), t0.elapsed().as_millis());
        self.store(key, &resp);
        resp
    }

    fn render(&self, f: Fetched) -> Response {
        let mime = f.mime.as_str();
        if mime.starts_with("image/") {
            return Response::redirect(html::image_link(&f.url));
        }
        // certains serveurs servent du HTML en text/plain ou sans type
        let start: Vec<u8> = f.body.iter().take(512).map(u8::to_ascii_lowercase).collect();
        let sniffed = start.windows(5).any(|w| w == b"<html") || start.windows(9).any(|w| w == b"<!doctype");
        let is_html = matches!(mime, "text/html" | "application/xhtml+xml") || (matches!(mime, "text/plain" | "application/octet-stream" | "") && sniffed);
        let notice = f.truncated.then(|| format!("Page tronquée : seuls les {} premiers ont été lus.", pages::size(self.limits.max_page)));
        if is_html {
            let text = html::decode(&f.body, f.charset.as_deref());
            let s = html::simplify(&text, &f.url);
            let page = pages::simplified(&s.title, &s.body, &f.url, f.body.len(), notice.as_deref());
            debug!("web1998 : {} allégée de {} à {}", f.url, pages::size(f.body.len()), pages::size(page.len()));
            return Response::html(200, page, 300);
        }
        if mime.starts_with("text/") {
            let text = html::decode(&f.body, f.charset.as_deref());
            let mut body = String::from("<pre>");
            html::escape_text(&mut body, &text);
            body.push_str("</pre>");
            let title = f.url.path().rsplit('/').next().unwrap_or("").to_string();
            return Response::html(200, pages::simplified(&title, &body, &f.url, f.body.len(), notice.as_deref()), 300);
        }
        Response::error(
            415,
            "Type de document non géré",
            &format!("Ce document ({mime}) ne peut pas être affiché par un navigateur de 1998. Utilisez le lien « voir l'original » d'une page, ou un logiciel adapté."),
            Some(&f.url),
        )
    }

    /// `/i` : image convertie.
    fn image(&self, url: Url) -> Response {
        let key = format!("i {url}");
        if let Some(r) = self.cached(&key) {
            return r;
        }
        let resp = match self.fetcher.fetch(&url, ACCEPT_IMAGE, self.limits.max_image, false, self.limits.timeout) {
            Ok(f) => {
                let opts = GifOptions::new(self.limits.image_width, self.limits.image_target);
                match images::to_gif(&f.body, &opts) {
                    Ok(gif) => {
                        debug!("web1998 : image {url} : {} → {}", pages::size(f.body.len()), pages::size(gif.len()));
                        Response::new(200, GIF, gif, 86_400)
                    }
                    Err(e) => Response::image_error(415, &e),
                }
            }
            Err(FetchError::TooBig(n)) => Response::image_error(413, &format!("image trop lourde ({})", pages::size(n))),
            Err(e) => {
                let (code, title, _) = describe(&e, &self.limits);
                Response::image_error(code, title)
            }
        };
        self.store(key, &resp);
        resp
    }
}

/// Code HTTP, titre et explication d'un échec de récupération.
fn describe(e: &FetchError, limits: &Limits) -> (u16, &'static str, String) {
    match e {
        FetchError::BadUrl => (400, "Adresse invalide", "Cette adresse est mal formée.".into()),
        FetchError::Forbidden(why) => (
            403,
            "Adresse interdite",
            format!("Cette adresse n'est pas accessible depuis Tinyverse 1998 ({why}). Seuls les sites web publics sont permis : http ou https, ports 80 et 443."),
        ),
        FetchError::Timeout => (
            504,
            "Délai dépassé",
            format!("Le site n'a pas répondu en moins de {} secondes. Il est peut-être surchargé (ou trop moderne). Réessayez plus tard.", limits.timeout.as_secs()),
        ),
        FetchError::UnknownHost(h) => (502, "Serveur introuvable", format!("Le nom « {h} » n'existe pas (ou le DNS est en panne). Vérifiez l'adresse.")),
        FetchError::Unreachable(why) => (502, "Connexion impossible", format!("Impossible de joindre le site : {why}.")),
        FetchError::Tls => (
            502,
            "Connexion sécurisée impossible",
            "Le certificat de ce site est invalide (ou la négociation TLS a échoué). Certains sites pour vieux ordinateurs ne fonctionnent qu'en http://.".into(),
        ),
        FetchError::Status(404 | 410) => (404, "Fichier introuvable", "Le site répond que cette page n'existe pas (erreur 404).".into()),
        FetchError::Status(code) => (502, "Erreur du site", format!("Le site a répondu par une erreur {code}.")),
        FetchError::TooBig(n) => (413, "Document trop lourd", format!("Ce document pèse {} : bien trop pour un modem.", pages::size(*n))),
        FetchError::TooManyRedirects => (502, "Trop de redirections", format!("Le site redirige plus de {} fois de suite.", fetch::MAX_REDIRECTS)),
    }
}

fn fetch_error(e: &FetchError, url: &Url, limits: &Limits) -> Response {
    let (code, title, msg) = describe(e, limits);
    // https refusé : proposer la version http (sans chiffrement, comme en 1998)
    let plain = (*e == FetchError::Tls).then(|| {
        let mut u = url.clone();
        let _ = u.set_scheme("http");
        html::page_link(&u)
    });
    let alt = plain.as_deref().map(|l| ("Essayer sans chiffrement (http://)", l));
    Response::html(code, pages::error(code, title, &msg, Some(url), alt), 0)
}

/// Ce que l'abonné a tapé dans « Aller à » : adresse complète, nom de
/// domaine (complété en `http://`), ou mots-clés (recherche FrogFind).
fn user_url(input: &str) -> Result<Url, ()> {
    let s = input.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("http://") || s.eq_ignore_ascii_case("https://") {
        return Err(());
    }
    if s.contains("://") {
        return Url::parse(s).map_err(|_| ());
    }
    let looks_like_host = !s.contains(char::is_whitespace) && (s.contains('.') || s.contains(':')) && !s.starts_with('.');
    if looks_like_host {
        if let Ok(u) = Url::parse(&format!("http://{s}")) {
            return Ok(u);
        }
    }
    let mut u = Url::parse("https://frogfind.com/").expect("URL valide");
    u.query_pairs_mut().append_pair("q", s);
    Ok(u)
}

/// `/f?u=ACTION&champ=valeur…` → ACTION dont les paramètres sont remplacés
/// par les champs (comme un navigateur envoie un formulaire GET).
fn form_target(query: &str) -> Option<Url> {
    let mut action = None;
    let mut fields = form_urlencoded::Serializer::new(String::new());
    let mut any = false;
    for (k, v) in form_urlencoded::parse(query.as_bytes()) {
        if k == "u" && action.is_none() {
            action = Some(Url::parse(v.trim()).ok()?);
        } else {
            fields.append_pair(&k, &v);
            any = true;
        }
    }
    let mut url = action?;
    let q = fields.finish();
    url.set_query(any.then_some(q.as_str()));
    Some(url)
}

/// Lien vers l'hôte virtuel lui-même : chemin local.
fn virtual_path(url: &Url) -> Option<String> {
    let host = url.host_str()?;
    if url.scheme() == "http" && is_virtual_host(host, url.port_or_known_default()?) {
        let mut p = url.path().to_string();
        if let Some(q) = url.query() {
            p.push('?');
            p.push_str(q);
        }
        return Some(p);
    }
    None
}

/// Lit l'en-tête d'une requête ; renvoie (méthode, cible).
fn read_request(s: &mut TcpStream) -> Option<(String, String)> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];
    while !(buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.windows(2).any(|w| w == b"\n\n")) {
        if buf.len() > MAX_REQUEST {
            return None;
        }
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => {
                if buf.is_empty() {
                    return None;
                }
                break;
            }
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let line = String::from_utf8_lossy(&buf).lines().next()?.to_string();
    let mut it = line.split_ascii_whitespace();
    let method = it.next()?.to_ascii_uppercase();
    let target = it.next()?.to_string();
    Some((method, target))
}

fn write_response(s: &mut TcpStream, r: &Response, head_only: bool) -> io::Result<()> {
    let mut h = format!(
        "HTTP/1.0 {} {}\r\nServer: Tinyverse/1998\r\nContent-Type: {}\r\nContent-Length: {}\r\n",
        r.status,
        reason(r.status),
        r.content_type,
        r.body.len()
    );
    if r.max_age > 0 {
        h.push_str(&format!("Cache-Control: max-age={}\r\n", r.max_age));
    } else {
        h.push_str("Cache-Control: no-cache\r\n");
    }
    if let Some(l) = &r.location {
        h.push_str(&format!("Location: {l}\r\n"));
    }
    if r.content_type == HTML {
        // pour la barre de chargement du navigateur de l'appli
        h.push_str(&format!("{}: {}\r\n", v34_net::tunnel::progress::ELEMENTS_HEADER, count_images(&r.body)));
    }
    h.push_str("Connection: close\r\n\r\n");
    s.write_all(h.as_bytes())?;
    if !head_only {
        s.write_all(&r.body)?;
    }
    s.flush()
}

/// Nombre d'images distinctes d'une page (le navigateur ne demande qu'une
/// fois une image répétée). Nos pages écrivent toujours `<img src="…"`.
fn count_images(html: &[u8]) -> usize {
    let html = String::from_utf8_lossy(html);
    let mut seen = std::collections::HashSet::new();
    for part in html.split("<img ").skip(1) {
        let tag = part.split('>').next().unwrap_or("");
        if let Some(src) = tag.split("src=\"").nth(1).and_then(|v| v.split('"').next()) {
            if !src.is_empty() && !src.starts_with("data:") {
                seen.insert(src.to_string());
            }
        }
    }
    seen.len()
}

/// Paire de sockets TCP reliées sur la boucle locale (équivalent de
/// `socketpair`, mais le multiplexeur du tunnel veut des `TcpStream`).
/// L'écouteur ne vit que le temps de la connexion ; on vérifie que c'est
/// bien NOTRE connexion qui est acceptée (adresse source).
fn tcp_pair() -> io::Result<(TcpStream, TcpStream)> {
    let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let a = TcpStream::connect(l.local_addr()?)?;
    let me = a.local_addr()?;
    l.set_nonblocking(true)?;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match l.accept() {
            Ok((b, peer)) if peer == me => {
                b.set_nonblocking(false)?;
                return Ok((a, b));
            }
            Ok(_) => continue, // intrus : fermé aussitôt
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() > deadline {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "paire de sockets locale"));
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests;
