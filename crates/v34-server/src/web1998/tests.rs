//! Tests du service web 1998 sans Internet : un petit serveur HTTP local
//! joue le « site », et le service est créé par `Web1998::for_tests`, seule
//! façon (compilée uniquement pour les tests) d'autoriser 127.0.0.1.

use std::io::{Cursor, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use image::{ImageBuffer, ImageFormat, Rgba};
use v34_net::tunnel::{ClientConfig, TunnelClient, TunnelEvent};

use super::*;
use crate::tunnel::{self, TunnelPolicy};

/// Page servie en ISO-8859-1 (avec son en-tête), comme un vieux site.
fn latin1_page(port: u16) -> Vec<u8> {
    let html = format!(
        "<html><head><title>Site d'essai</title><script>document.write('pub')</script></head><body>\
         <h1>Bienvenue \u{e9}t\u{e9}</h1><p>Un <a href=\"/autre?x=1\">lien</a> et une image :</p>\
         <img src=\"/img.png\" alt=\"photo\"><img src=\"http://127.0.0.1:{port}/img.png?v=2\">\
         <form action=\"/recherche\"><input name=\"q\"><input type=\"submit\" value=\"Chercher\"></form>\
         </body></html>"
    );
    html.chars().map(|c| c as u32 as u8).collect()
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let img = ImageBuffer::from_fn(w, h, |x, y| Rgba([(x % 256) as u8, (y % 256) as u8, ((x * y) % 256) as u8, 255]));
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png).unwrap();
    out
}

/// Puce rouge de 12 px sur fond blanc (une centaine d'octets en PNG).
fn puce() -> Vec<u8> {
    let img = ImageBuffer::from_fn(12, 12, |x, y| {
        let d = (x as i32 * 2 - 11).pow(2) + (y as i32 * 2 - 11).pow(2);
        if d < 100 { Rgba([200u8, 0, 0, 255]) } else { Rgba([255, 255, 255, 255]) }
    });
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png).unwrap();
    out
}

struct Site {
    port: u16,
    hits: Arc<AtomicUsize>,
}

/// Petit serveur HTTP local (un thread par connexion).
fn site() -> Site {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let h = h.clone();
            std::thread::spawn(move || serve_site(s, port, &h));
        }
    });
    Site { port, hits }
}

fn serve_site(mut s: TcpStream, port: u16, hits: &AtomicUsize) {
    let mut req = Vec::new();
    let mut b = [0u8; 1024];
    while !req.windows(4).any(|w| w == b"\r\n\r\n") {
        match s.read(&mut b) {
            Ok(0) | Err(_) => return,
            Ok(n) => req.extend_from_slice(&b[..n]),
        }
    }
    hits.fetch_add(1, Ordering::SeqCst);
    let line = String::from_utf8_lossy(&req).lines().next().unwrap_or("").to_string();
    let path = line.split(' ').nth(1).unwrap_or("/").to_string();
    let (status, ctype, body, extra): (&str, &str, Vec<u8>, String) = match path.split('?').next().unwrap() {
        "/page" => ("200 OK", "text/html; charset=ISO-8859-1", latin1_page(port), String::new()),
        "/img.png" => ("200 OK", "image/png", png(640, 480), String::new()),
        "/puce.png" => ("200 OK", "image/png", puce(), String::new()),
        "/gros.png" => ("200 OK", "image/png", vec![0u8; 1100 * 1024], String::new()),
        "/doc.pdf" => ("200 OK", "application/pdf", b"%PDF-1.4".to_vec(), String::new()),
        "/recherche" => ("200 OK", "text/html", format!("<p>Recherche : {path}</p>").into_bytes(), String::new()),
        "/vers-page" => ("301 Moved", "text/plain", Vec::new(), "Location: /page\r\n".into()),
        "/vers-prive" => ("302 Found", "text/plain", Vec::new(), "Location: http://192.168.1.1/admin\r\n".into()),
        "/vers-autre-port" => ("302 Found", "text/plain", Vec::new(), "Location: http://127.0.0.1:22/\r\n".into()),
        "/boucle" => ("302 Found", "text/plain", Vec::new(), "Location: /boucle\r\n".into()),
        "/lent" => {
            std::thread::sleep(Duration::from_secs(3));
            ("200 OK", "text/html", b"<p>enfin</p>".to_vec(), String::new())
        }
        _ => ("404 Not Found", "text/html", b"<h1>404</h1>".to_vec(), String::new()),
    };
    let head = format!("HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n", body.len());
    let _ = s.write_all(head.as_bytes());
    let _ = s.write_all(&body);
}

fn limits() -> Limits {
    Limits { max_image: 1024 * 1024, timeout: Duration::from_secs(1), ..Limits::default() }
}

fn body(r: &Response) -> String {
    String::from_utf8_lossy(&r.body).into_owned()
}

#[test]
fn hote_virtuel_reconnu() {
    assert!(is_virtual_host("web1998", 80));
    assert!(is_virtual_host("WEB1998", 80));
    assert!(is_virtual_host("Web1998.", 80));
    assert!(!is_virtual_host("web1998", 443));
    assert!(!is_virtual_host("web1998", 8080));
    assert!(!is_virtual_host("web1998.com", 80));
    assert!(!is_virtual_host("www.web1998", 80));
}

#[test]
fn accueil_compteur_et_images_generees() {
    let w = Web1998::new(Limits::default(), Vec::new());
    let r = w.handle("/");
    assert_eq!((r.status, r.content_type), (200, HTML));
    assert!(body(&r).contains("Tinyverse 1998") && body(&r).contains("compteur.gif?n=000001"));
    assert!(body(&w.handle("http://web1998/")).contains("compteur.gif?n=000002"), "forme absolue acceptée");
    assert_eq!(w.visits(), 2);
    let g = w.handle("/construction.gif");
    assert_eq!((g.status, g.content_type), (200, GIF));
    assert!(g.body.starts_with(b"GIF89a"));
    assert!(w.handle("/compteur.gif?n=000042").body.starts_with(b"GIF89a"));
    let e = w.handle("/nimporte/quoi");
    assert_eq!(e.status, 404);
    assert!(body(&e).contains("Fichier introuvable"));
    assert_eq!(w.handle("/p").status, 400, "adresse manquante");
    assert_eq!(w.handle("/p?u=http%3A%2F%2F").status, 400, "« http:// » seul");
}

#[test]
fn adresses_saisies() {
    assert_eq!(user_url("fr.wikipedia.org").unwrap().as_str(), "http://fr.wikipedia.org/");
    assert_eq!(user_url(" https://68k.news/ ").unwrap().as_str(), "https://68k.news/");
    assert_eq!(user_url("recette tarte tatin").unwrap().as_str(), "https://frogfind.com/?q=recette+tarte+tatin");
    assert!(user_url("").is_err());
    let f = form_target("u=https%3A%2F%2Fa.fr%2Fs%3Fold%3D1&q=caf%C3%A9+noir&u=x").unwrap();
    assert_eq!(f.as_str(), "https://a.fr/s?q=caf%C3%A9+noir&u=x", "paramètres de l'action remplacés par les champs");
    assert!(form_target("q=1").is_none());
    assert_eq!(virtual_path(&Url::parse("http://WEB1998/p?u=x").unwrap()).as_deref(), Some("/p?u=x"));
    assert_eq!(virtual_path(&Url::parse("http://web1998.fr/").unwrap()), None);
}

#[test]
fn reseau_local_interdit_en_production() {
    let site = site();
    let w = Web1998::new(limits(), Vec::new());
    for u in [
        format!("http://127.0.0.1:{}/page", site.port),
        "http://127.0.0.1/".to_string(),
        "http://localhost/".to_string(),
        "http://10.1.2.3/".to_string(),
        "http://169.254.169.254/latest/meta-data/".to_string(),
        "http://[::1]/".to_string(),
        "http://example.com:25/".to_string(),
        "file:///etc/passwd".to_string(),
    ] {
        let r = w.handle(&format!("/p?u={}", html::encode_param(&u)));
        assert_eq!(r.status, 403, "{u}");
        assert!(body(&r).contains("Adresse interdite"), "{u}");
        let i = w.handle(&format!("/i?u={}", html::encode_param(&u)));
        assert_eq!(i.status, 403, "{u} (image)");
        let h = w.handle(&format!("/h?u={}", html::encode_param(&u)));
        assert_eq!(h.status, 403, "{u} (image hallucinée)");
    }
    assert_eq!(site.hits.load(Ordering::SeqCst), 0, "le site local n'a jamais été contacté");
}

#[test]
fn page_simplifiee_depuis_un_site_local() {
    let site = site();
    let w = Web1998::for_tests(limits(), vec![site.port]);
    let base = format!("http://127.0.0.1:{}", site.port);
    let r = w.handle(&format!("/p?u={base}/page"));
    assert_eq!((r.status, r.content_type), (200, HTML));
    let b = body(&r);
    assert!(b.contains("<title>Site d'essai</title>"), "{b}");
    assert!(b.contains("<h1>Bienvenue été</h1>"), "ISO-8859-1 converti en UTF-8 :\n{b}");
    assert!(!b.contains("<script") && !b.contains("document.write"));
    assert!(b.contains(&format!("<a href=\"/p?u={base}/autre?x=1\">lien</a>")), "{b}");
    assert!(b.contains(&format!("<img src=\"/i?u={base}/img.png\" alt=\"photo\">")), "{b}");
    assert!(b.contains(&format!("<form action=\"/f\" method=\"get\"><input type=\"hidden\" name=\"u\" value=\"{base}/recherche\">")), "{b}");
    assert!(b.contains("Tinyverse 1998</b> &mdash; page allégée de"), "bandeau :\n{b}");
    assert!(b.contains(&format!("<a href=\"{base}/page\"><font color=\"#ffff00\">voir l'original</font></a>")), "{b}");

    // cache : la deuxième demande ne recontacte pas le site
    let before = site.hits.load(Ordering::SeqCst);
    assert_eq!(body(&w.handle(&format!("/p?u={base}/page"))), b);
    assert_eq!(site.hits.load(Ordering::SeqCst), before);

    // formulaire GET envoyé par /f
    let f = w.handle(&format!("/f?u={}&q=tarte+aux+pommes", html::encode_param(&format!("{base}/recherche"))));
    assert!(body(&f).contains("Recherche : /recherche?q=tarte+aux+pommes"), "{}", body(&f));

    // redirection suivie, l'original est l'URL finale
    let r = w.handle(&format!("/p?u={base}/vers-page"));
    assert_eq!(r.status, 200);
    assert!(body(&r).contains(&format!("<a href=\"{base}/page\"><font")));
    // image servie via /p : redirigée vers /i
    let r = w.handle(&format!("/p?u={base}/img.png"));
    assert_eq!((r.status, r.location.as_deref()), (302, Some(format!("/i?u={base}/img.png").as_str())));
    // type non géré, 404 du site
    assert_eq!(w.handle(&format!("/p?u={base}/doc.pdf")).status, 415);
    let r = w.handle(&format!("/p?u={base}/absente"));
    assert_eq!(r.status, 404);
    assert!(body(&r).contains("Fichier introuvable"));
}

#[test]
fn redirections_reverifiees_et_delais() {
    let site = site();
    let w = Web1998::for_tests(limits(), vec![site.port]);
    let base = format!("http://127.0.0.1:{}", site.port);
    let r = w.handle(&format!("/p?u={base}/vers-prive"));
    assert_eq!(r.status, 403, "redirection vers le réseau privé refusée");
    assert!(body(&r).contains("Adresse interdite"));
    assert_eq!(w.handle(&format!("/p?u={base}/vers-autre-port")).status, 403, "port 22 refusé après redirection");
    let r = w.handle(&format!("/p?u={base}/boucle"));
    assert!(body(&r).contains("Trop de redirections"), "{}", body(&r));
    let r = w.handle(&format!("/p?u={base}/lent"));
    assert_eq!(r.status, 504);
    assert!(body(&r).contains("Délai dépassé"));
}

#[test]
fn images_converties() {
    let site = site();
    let w = Web1998::for_tests(limits(), vec![site.port]);
    let base = format!("http://127.0.0.1:{}", site.port);
    let r = w.handle(&format!("/i?u={base}/img.png"));
    assert_eq!((r.status, r.content_type), (200, GIF));
    let img = image::load_from_memory(&r.body).unwrap();
    // motif bruité : réduit en couleurs, puis en taille, pour tenir le poids visé
    let (iw, ih) = (img.width() as i64, img.height() as i64);
    assert!(iw <= 320 && (iw * 3 - ih * 4).abs() <= 4, "{iw}×{ih} : proportions gardées");
    assert!(r.body.len() <= limits().image_target, "{} o", r.body.len());
    assert_eq!(w.handle(&format!("/i?u={base}/gros.png")).status, 413, "image plus lourde que la limite");
    assert_eq!(w.handle(&format!("/i?u={base}/doc.pdf")).status, 415);
}

#[test]
fn images_hallucinees() {
    let site = site();
    let w = Web1998::for_tests(limits(), vec![site.port]);
    let base = format!("http://127.0.0.1:{}", site.port);
    let r = w.handle(&format!("/h?u={base}/img.png"));
    assert_eq!((r.status, r.content_type, r.max_age), (200, SVG, 86_400));
    let svg = body(&r);
    assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 96 72\" width=\"320\" height=\"240\">"), "taille d'affichage du GIF : {svg}");
    assert!(svg.ends_with("</svg>") && svg.contains("<g fill-opacity="), "{svg}");
    assert!(r.body.len() <= hallu::HalluOptions::default().max_bytes, "{} o", r.body.len());
    let gif = w.handle(&format!("/i?u={base}/img.png"));
    assert!(r.body.len() * 4 < gif.body.len(), "SVG {} o, GIF {} o", r.body.len(), gif.body.len());
    // cache (même rêve, site pas recontacté)
    let hits = site.hits.load(Ordering::SeqCst);
    assert_eq!(body(&w.handle(&format!("/h?u={base}/img.png"))), svg);
    assert_eq!(site.hits.load(Ordering::SeqCst), hits);
    // erreurs comme /i
    assert_eq!(w.handle(&format!("/h?u={base}/gros.png")).status, 413, "image plus lourde que la limite");
    assert_eq!(w.handle(&format!("/h?u={base}/doc.pdf")).status, 415);
    assert_eq!(w.handle("/h?u=pas%20une%20adresse").status, 400);
    assert_eq!(w.handle(&format!("/h?u={base}/absente.png")).status, 404);
    // une puce de 12 px pèse moins que son rêve : GIF ordinaire
    let puce = w.handle(&format!("/h?u={base}/puce.png"));
    assert_eq!((puce.status, puce.content_type), (200, GIF));
    assert!(puce.body.len() < 300, "{} o", puce.body.len());
}

#[test]
fn choix_du_mode_d_images() {
    let site = site();
    let w = Web1998::for_tests(limits(), vec![site.port]);
    let base = format!("http://127.0.0.1:{}", site.port);
    let page = format!("/p?u={base}/page");
    let normal = w.handle(&page);
    let b = body(&normal);
    assert!(b.contains(&format!("<img src=\"/i?u={base}/img.png\" alt=\"photo\">")), "{b}");
    assert!(b.contains(&format!("Images : <b>normales</b> &middot; <a href=\"/p?u={base}/page&amp;images=hallu\">")), "{b}");
    assert!(normal.set_cookie.is_none());

    // cookie : même page en cache, autres liens d'images et autre bandeau
    let hits = site.hits.load(Ordering::SeqCst);
    let reve = w.handle_with_cookies(&page, "theme=sombre; images=hallu");
    let b = body(&reve);
    assert!(b.contains(&format!("<img src=\"/h?u={base}/img.png\" alt=\"photo\">")), "{b}");
    assert!(b.contains(&format!("<img src=\"/h?u={base}/img.png?v=2\" alt=\"\">")), "{b}");
    assert!(!b.contains("/i?u="), "{b}");
    assert!(b.contains(&format!("<a href=\"/p?u={base}/page&amp;images=normales\"><font color=\"#ffff00\">normales</font></a> &middot; <b>V.Hallucination</b>")), "{b}");
    assert!(b.contains("page allégée de"), "{b}");
    assert_eq!((reve.max_age, reve.set_cookie.as_deref()), (300, None));
    assert_eq!(site.hits.load(Ordering::SeqCst), hits, "changer de mode ne recharge pas la page");
    assert!(body(&w.handle_with_cookies(&page, "images=n'importe quoi")).contains("/i?u="), "cookie inconnu : images normales");

    // choix explicite (liens du bandeau et de l'accueil) : il l'emporte sur
    // le cookie, le remplace, et la réponse n'est pas gardée par le navigateur
    let choix = w.handle(&format!("{page}&images=hallu"));
    assert!(body(&choix).contains("<img src=\"/h?u="));
    assert_eq!(choix.set_cookie.as_deref(), Some("images=hallu; Path=/; Max-Age=31536000; SameSite=Lax"));
    assert_eq!(choix.max_age, 0);
    let retour = w.handle_with_cookies(&format!("{page}&images=normales"), "images=hallu");
    assert!(body(&retour).contains("<img src=\"/i?u=") && retour.set_cookie.as_deref().is_some_and(|c| c.starts_with("images=normales;")));
    let accueil = w.handle("/?images=hallu");
    assert!(body(&accueil).contains("<b>V.Hallucination</b>") && accueil.set_cookie.is_some());
    assert!(body(&w.handle_with_cookies("/", "images=hallu")).contains("<b>V.Hallucination</b>"));

    // /f : un champ « images » appartient au formulaire du site
    let f = w.handle(&format!("/f?u={}&images=hallu", html::encode_param(&format!("{base}/recherche"))));
    assert!(f.set_cookie.is_none());
    assert!(body(&f).contains("Recherche : /recherche?images=hallu"), "{}", body(&f));
    // image ouverte comme une page : redirigée selon le mode
    let r = w.handle_with_cookies(&format!("/p?u={base}/img.png"), "images=hallu");
    assert_eq!((r.status, r.location.as_deref()), (302, Some(format!("/h?u={base}/img.png").as_str())));
    // redirection : le bandeau vise l'adresse finale, déjà en cache
    let r = w.handle_with_cookies(&format!("/p?u={base}/vers-page"), "images=hallu");
    assert!(body(&r).contains(&format!("<a href=\"/p?u={base}/page&amp;images=normales\">")), "{}", body(&r));
}

/// Le petit serveur HTTP lit le cookie et pose celui du choix.
#[test]
fn cookie_du_mode_d_images_en_http() {
    let site = site();
    let web = Arc::new(Web1998::for_tests(limits(), vec![site.port]));
    let get = |req: String| -> String {
        let mut s = web.open_stream().unwrap();
        s.write_all(req.as_bytes()).unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).unwrap();
        String::from_utf8_lossy(&out).into_owned()
    };
    let page = format!("/p?u=http://127.0.0.1:{}/page", site.port);
    let r = get(format!("GET {page} HTTP/1.1\r\nHost: web1998\r\ncookie: a=1\r\nCookie: images=hallu\r\n\r\n"));
    assert!(r.starts_with("HTTP/1.0 200 OK\r\n") && r.contains("\r\nVary: Cookie\r\n"), "{r}");
    assert!(r.contains("<img src=\"/h?u=") && !r.contains("Set-Cookie"), "{r}");
    let r = get(format!("GET {page} HTTP/1.0\r\n\r\n"));
    assert!(r.contains("<img src=\"/i?u="), "sans cookie : {r}");
    let r = get("GET /?images=hallu HTTP/1.0\r\n\r\n".into());
    assert!(r.contains("\r\nSet-Cookie: images=hallu; Path=/; Max-Age=31536000; SameSite=Lax\r\n") && r.contains("Cache-Control: no-cache"), "{r}");
    let r = get(format!("GET /h?u=http://127.0.0.1:{}/img.png HTTP/1.0\r\n\r\n", site.port));
    assert!(r.contains("\r\nContent-Type: image/svg+xml\r\n") && r.contains("max-age=86400") && !r.contains("Vary"), "{}", &r[..200]);
    assert!(r.contains("\r\n\r\n<svg "));
}

/// Bout en bout : navigateur → proxy SOCKS5 du client de tunnel → canaux
/// (à la place du modem) → bout serveur du tunnel → hôte virtuel → site local.
#[test]
fn hote_virtuel_a_travers_le_tunnel() {
    crate::log::set_level(crate::log::Level::Error);
    let site = site();
    let web = Arc::new(Web1998::for_tests(limits(), vec![site.port]));
    // port 80 NON autorisé et réseau privé fermé : l'hôte virtuel passe quand même
    let policy = TunnelPolicy { allowed_ports: vec![443], allow_private: false, web1998: Some(web.clone()), ..TunnelPolicy::default() };
    let (c2s_tx, c2s_rx) = mpsc::sync_channel(4);
    let (s2c_tx, s2c_rx) = mpsc::sync_channel(4);
    std::thread::spawn(move || tunnel::run(Arc::new(policy), "test".into(), "alice", c2s_rx, s2c_tx));
    let client = TunnelClient::spawn(TcpListener::bind("127.0.0.1:0").unwrap(), ClientConfig::default(), s2c_rx, c2s_tx).unwrap();
    let t0 = Instant::now();
    while !matches!(client.try_event(), Some(TunnelEvent::Ready { .. })) {
        assert!(t0.elapsed() < Duration::from_secs(5), "tunnel prêt");
        std::thread::sleep(Duration::from_millis(5));
    }

    let get = |path: &str| -> Vec<u8> {
        let mut s = socks5(client.local_addr(), HOST, 80).expect("hôte virtuel accepté");
        write!(s, "GET {path} HTTP/1.1\r\nHost: web1998\r\nConnection: keep-alive\r\n\r\n").unwrap();
        let mut resp = Vec::new();
        s.read_to_end(&mut resp).unwrap();
        resp
    };
    let home = String::from_utf8(get("/")).unwrap();
    assert!(home.starts_with("HTTP/1.0 200 OK\r\n"), "{home}");
    assert!(home.contains("Tinyverse 1998"));
    assert!(home.contains("\r\nX-Tinyverse-Elements: 2\r\n"), "chantier + compteur : {home}");

    let page = String::from_utf8(get(&format!("/p?u=http://127.0.0.1:{}/page", site.port))).unwrap();
    assert!(page.contains("<h1>Bienvenue été</h1>"), "{page}");

    let img = get(&format!("/i?u=http://127.0.0.1:{}/img.png", site.port));
    let at = img.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    assert!(img.starts_with(b"HTTP/1.0 200 OK") && img[at..].starts_with(b"GIF89a"));

    // la barre de chargement de l'appli suit la page et son image
    let t0 = Instant::now();
    let p = loop {
        let pr = client.progress();
        if pr.streams == 0 && pr.page.as_ref().is_some_and(|p| p.done >= 1) {
            break pr.page.unwrap();
        }
        assert!(t0.elapsed() < Duration::from_secs(5), "{pr:?}");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(p.doc_done && p.seq == 2 && p.doc_len == Some(p.doc_rx), "{p:?}");
    assert_eq!(p.elements_rx, (img.len() - at) as u64);

    // une vraie connexion vers 127.0.0.1 reste refusée par le filtre
    assert!(socks5(client.local_addr(), "127.0.0.1", site.port).is_err());
    assert!(socks5(client.local_addr(), HOST, 443).is_err(), "web1998:443 n'est pas l'hôte virtuel");
    assert!(web.visits() >= 1);
}

/// Poignée de main SOCKS5 avec un nom de domaine ; Err(code) si refusé.
fn socks5(proxy: std::net::SocketAddr, host: &str, port: u16) -> Result<TcpStream, u8> {
    let mut s = TcpStream::connect(proxy).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(&[5, 1, 0]).unwrap();
    let mut r = [0u8; 2];
    s.read_exact(&mut r).unwrap();
    let mut req = vec![5, 1, 0, 3, host.len() as u8];
    req.extend_from_slice(host.as_bytes());
    req.extend_from_slice(&port.to_be_bytes());
    s.write_all(&req).unwrap();
    let mut rep = [0u8; 10];
    s.read_exact(&mut rep).unwrap();
    if rep[1] == 0 {
        Ok(s)
    } else {
        Err(rep[1])
    }
}
