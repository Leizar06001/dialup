//! Récupérations HTTP(S) du service web 1998, FILTRÉES comme le tunnel :
//!
//! - protocoles `http` et `https` seulement, ports 80 et 443 seulement ;
//! - adresse vérifiée APRÈS résolution DNS par [`crate::filter`] (pas de
//!   127.0.0.1, de réseau privé, de métadonnées du cloud…) et par les
//!   réseaux interdits de la configuration ; la connexion se fait à
//!   l'adresse vérifiée (résolveur de `ureq` remplacé), donc pas de
//!   contournement par un nom qui pointe vers le réseau local ;
//! - redirections suivies à la main (≤ [`MAX_REDIRECTS`]), chacune
//!   revérifiée ; délai global et taille maximale bornés.
//!
//! TLS : rustls + racines de confiance embarquées (webpki-roots), pas
//! d'OpenSSL.

use std::io::{self, Read};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

use url::Url;

use crate::filter::{is_forbidden, Cidr};

/// Redirections suivies au plus.
pub const MAX_REDIRECTS: usize = 5;

const USER_AGENT: &str = "Mozilla/5.0 (compatible; Tinyverse1998/0.1; navigateur web 1998 par modem)";
const ACCEPT_LANGUAGE: &str = "fr-FR,fr;q=0.9,en;q=0.5";

/// Échec d'une récupération (affiché à l'abonné sous forme de page d'erreur).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchError {
    /// Adresse mal formée
    BadUrl,
    /// Destination refusée par le filtre (raison)
    Forbidden(String),
    /// Délai dépassé
    Timeout,
    /// Nom d'hôte inconnu
    UnknownHost(String),
    /// Connexion ou échange impossible (raison)
    Unreachable(String),
    /// Échec de la négociation TLS (certificat invalide…) d'un site https
    Tls,
    /// Le site a répondu par une erreur HTTP
    Status(u16),
    /// Document trop lourd (taille connue ou minimale)
    TooBig(usize),
    TooManyRedirects,
}

/// Document récupéré.
#[derive(Clone, Debug)]
pub struct Fetched {
    /// URL finale (après redirections)
    pub url: Url,
    /// Type MIME en minuscules, sans paramètres
    pub mime: String,
    /// Paramètre `charset` de l'en-tête Content-Type, s'il y en a un
    pub charset: Option<String>,
    pub body: Vec<u8>,
    /// Corps coupé à la taille maximale
    pub truncated: bool,
}

/// Marqueur d'erreur du résolveur : adresse interdite.
#[derive(Debug)]
struct Refus;

impl std::fmt::Display for Refus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("destination interdite (adresse privée ou locale)")
    }
}

impl std::error::Error for Refus {}

#[derive(Debug)]
struct Filter {
    denied: Vec<Cidr>,
    /// Tests uniquement : ports autorisés sur la boucle locale (vide en
    /// production, voir `Fetcher::new`).
    local_ports: Vec<u16>,
}

impl Filter {
    fn port_ok(&self, port: u16) -> bool {
        port == 80 || port == 443 || self.local_ports.contains(&port)
    }

    fn addr_ok(&self, a: &SocketAddr) -> bool {
        let ip = a.ip();
        if self.denied.iter().any(|c| c.contains(ip)) {
            return false;
        }
        if is_forbidden(ip) {
            return ip.is_loopback() && self.local_ports.contains(&a.port());
        }
        a.port() == 80 || a.port() == 443
    }

    /// Résolveur donné à `ureq` : seules les adresses autorisées sont
    /// rendues, et `ureq` se connecte à l'une d'elles.
    fn resolve(&self, netloc: &str) -> io::Result<Vec<SocketAddr>> {
        let addrs: Vec<SocketAddr> = netloc.to_socket_addrs()?.collect();
        let ok: Vec<SocketAddr> = addrs.iter().copied().filter(|a| self.addr_ok(a)).collect();
        if ok.is_empty() && !addrs.is_empty() {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, Refus));
        }
        Ok(ok)
    }
}

/// Client HTTP(S) filtré (connexions réutilisées entre récupérations).
pub struct Fetcher {
    agent: ureq::Agent,
    filter: Arc<Filter>,
}

impl Fetcher {
    /// `denied` : réseaux interdits en plus de [`is_forbidden`].
    /// `local_ports` : DOIT être vide hors des tests (seul
    /// `Web1998::for_tests`, compilé uniquement pour les tests, en passe).
    pub fn new(denied: Vec<Cidr>, local_ports: Vec<u16>) -> Fetcher {
        let filter = Arc::new(Filter { denied, local_ports });
        let f = filter.clone();
        let agent = ureq::AgentBuilder::new()
            .redirects(0)
            .user_agent(USER_AGENT)
            .timeout_connect(Duration::from_secs(10))
            .max_idle_connections(16)
            .max_idle_connections_per_host(2)
            .resolver(move |netloc: &str| f.resolve(netloc))
            .build();
        Fetcher { agent, filter }
    }

    /// Vérifications faisables sans réseau (protocole, port).
    pub fn check(&self, url: &Url) -> Result<(), FetchError> {
        if !matches!(url.scheme(), "http" | "https") {
            return Err(FetchError::Forbidden(format!("protocole « {} » non géré", url.scheme())));
        }
        if url.host_str().is_none_or(str::is_empty) {
            return Err(FetchError::BadUrl);
        }
        let port = url.port_or_known_default().ok_or(FetchError::BadUrl)?;
        if !self.filter.port_ok(port) {
            return Err(FetchError::Forbidden(format!("port {port} non autorisé (80 et 443 seulement)")));
        }
        Ok(())
    }

    /// GET avec redirections. `max` : taille maximale du corps ; au-delà,
    /// il est coupé si `truncate`, sinon c'est une erreur.
    pub fn fetch(&self, url: &Url, accept: &str, max: usize, truncate: bool, timeout: Duration) -> Result<Fetched, FetchError> {
        let deadline = Instant::now() + timeout;
        let mut url = url.clone();
        url.set_fragment(None);
        for _ in 0..=MAX_REDIRECTS {
            self.check(&url)?;
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(FetchError::Timeout);
            }
            let resp = self
                .agent
                .request_url("GET", &url)
                .timeout(left)
                .set("Accept", accept)
                .set("Accept-Language", ACCEPT_LANGUAGE)
                .call()
                .map_err(|e| classify(e, &url))?;
            let status = resp.status();
            if (300..400).contains(&status) {
                match resp.header("location").and_then(|l| url.join(l.trim()).ok()) {
                    Some(mut next) => {
                        next.set_fragment(None);
                        url = next;
                        continue;
                    }
                    None => return Err(FetchError::Status(status)),
                }
            }
            if let Some(len) = resp.header("content-length").and_then(|v| v.trim().parse::<usize>().ok()) {
                if len > max && !truncate {
                    return Err(FetchError::TooBig(len));
                }
            }
            let mime = resp.content_type().trim().to_ascii_lowercase();
            let charset = resp.header("content-type").and_then(charset_param);
            let mut body = Vec::new();
            resp.into_reader().take(max as u64 + 1).read_to_end(&mut body).map_err(read_error)?;
            let truncated = body.len() > max;
            if truncated {
                if !truncate {
                    return Err(FetchError::TooBig(body.len()));
                }
                body.truncate(max);
            }
            return Ok(Fetched { url, mime, charset, body, truncated });
        }
        Err(FetchError::TooManyRedirects)
    }
}

/// `text/html; charset=ISO-8859-1` → `ISO-8859-1`.
fn charset_param(content_type: &str) -> Option<String> {
    content_type.split(';').skip(1).find_map(|p| {
        let (k, v) = p.split_once('=')?;
        k.trim().eq_ignore_ascii_case("charset").then(|| v.trim().trim_matches(|c| c == '"' || c == '\'').to_string())
    })
}

fn read_error(e: io::Error) -> FetchError {
    match e.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => FetchError::Timeout,
        _ => FetchError::Unreachable(format!("lecture interrompue ({e})")),
    }
}

fn classify(e: ureq::Error, url: &Url) -> FetchError {
    let t = match e {
        ureq::Error::Status(code, _) => return FetchError::Status(code),
        ureq::Error::Transport(t) => t,
    };
    // cause première : refus du filtre, délai dépassé, erreur TLS
    let mut tls = false;
    let mut src: Option<&(dyn std::error::Error + 'static)> = Some(&t);
    while let Some(s) = src {
        if let Some(io) = s.downcast_ref::<io::Error>() {
            if io.get_ref().is_some_and(|inner| inner.is::<Refus>()) {
                return FetchError::Forbidden("adresse privée ou locale".into());
            }
            match io.kind() {
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => return FetchError::Timeout,
                // rustls signale ses erreurs (certificat…) en InvalidData
                io::ErrorKind::InvalidData => tls = true,
                _ => {}
            }
        }
        src = s.source();
    }
    if tls && url.scheme() == "https" {
        return FetchError::Tls;
    }
    use ureq::ErrorKind as K;
    match t.kind() {
        K::Dns => FetchError::UnknownHost(url.host_str().unwrap_or("?").to_string()),
        K::InvalidUrl | K::UnknownScheme => FetchError::BadUrl,
        K::TooManyRedirects => FetchError::TooManyRedirects,
        K::ConnectionFailed => FetchError::Unreachable("connexion impossible".into()),
        K::BadStatus | K::BadHeader => FetchError::Unreachable("réponse HTTP invalide".into()),
        _ => FetchError::Unreachable(format!("erreur réseau ({})", t.kind())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(f: &Fetcher, u: &str) -> Result<Fetched, FetchError> {
        f.fetch(&Url::parse(u).unwrap(), "*/*", 1000, false, Duration::from_secs(2))
    }

    #[test]
    fn destinations_refusees() {
        let f = Fetcher::new(vec![Cidr::parse("8.8.4.0/24").unwrap()], Vec::new());
        for u in [
            "http://127.0.0.1/",
            "http://127.0.0.1:80/admin",
            "https://localhost/",
            "http://10.0.0.1/",
            "http://192.168.1.1/",
            "http://169.254.169.254/latest/meta-data/",
            "http://[::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://8.8.4.4/", // réseau interdit par la configuration
        ] {
            assert!(matches!(get(&f, u), Err(FetchError::Forbidden(_))), "{u} doit être refusée");
        }
        for u in ["http://example.com:22/", "http://1.1.1.1:8080/", "ftp://example.com/", "file:///etc/passwd", "gopher://x/"] {
            assert!(matches!(get(&f, u), Err(FetchError::Forbidden(_))), "{u} doit être refusée");
        }
    }

    #[test]
    fn parametre_charset() {
        assert_eq!(charset_param("text/html; charset=ISO-8859-1").as_deref(), Some("ISO-8859-1"));
        assert_eq!(charset_param("text/html;Charset=\"utf-8\"").as_deref(), Some("utf-8"));
        assert_eq!(charset_param("text/html"), None);
    }
}
