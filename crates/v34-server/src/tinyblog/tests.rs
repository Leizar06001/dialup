use std::path::PathBuf;

use super::*;

fn dossier(nom: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("v34-tinyblog-{nom}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn get(t: &Tinyblog, qui: &str, cible: &str) -> Reponse {
    t.traiter(qui, &Requete { methode: "GET".into(), cible: cible.into(), type_contenu: String::new(), corps: Vec::new() })
}

fn post(t: &Tinyblog, qui: &str, cible: &str, champs: &[(&str, &str)]) -> Reponse {
    let corps = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(champs).finish();
    t.traiter(qui, &Requete { methode: "POST".into(), cible: cible.into(), type_contenu: "application/x-www-form-urlencoded".into(), corps: corps.into_bytes() })
}

/// Un formulaire multipart comme l'envoie un navigateur.
fn article(t: &Tinyblog, qui: &str, titre: &str, texte: &str, image: Option<&[u8]>) -> Reponse {
    let b = "----frontiere1998";
    let mut corps = Vec::new();
    for (n, v) in [("titre", titre), ("texte", texte)] {
        corps.extend(format!("--{b}\r\nContent-Disposition: form-data; name=\"{n}\"\r\n\r\n{v}\r\n").into_bytes());
    }
    corps.extend(format!("--{b}\r\nContent-Disposition: form-data; name=\"image\"; filename=\"photo.png\"\r\nContent-Type: image/png\r\n\r\n").into_bytes());
    corps.extend_from_slice(image.unwrap_or(&[]));
    corps.extend(format!("\r\n--{b}--\r\n").into_bytes());
    t.traiter(qui, &Requete { methode: "POST".into(), cible: "/gerer/article".into(), type_contenu: format!("multipart/form-data; boundary={b}"), corps })
}

fn texte(r: &Reponse) -> String {
    String::from_utf8_lossy(&r.corps).into_owned()
}

fn png() -> Vec<u8> {
    let img = image::RgbImage::from_fn(640, 480, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 128]));
    let mut v = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut v), image::ImageFormat::Png).unwrap();
    v
}

#[test]
fn un_tinyblog_de_bout_en_bout() {
    let d = dossier("complet");
    let exploits: Exploits = Default::default();
    let t = Tinyblog::new(d.clone(), exploits.clone());
    assert!(texte(&get(&t, "alice", "/")).contains("Pas encore d'article"));
    assert!(texte(&get(&t, "alice", "/gerer")).contains("Nouvel article"));
    // réglages
    let r = post(&t, "alice", "/gerer/reglages", &[("titre", "Le blog d'Alice"), ("description", "[b]Bienvenue[/b] :)"), ("theme", "rose")]);
    assert_eq!((r.statut, r.location.as_deref()), (303, Some("/gerer")));
    // un article avec une image, du code et une tentative de script
    let r = article(&t, "alice", "Mon premier article", "<script>alert(1)</script>\n[b]gras[/b] [couleur=rose]rose [clignote]qui clignote &lt;3", Some(&png()));
    assert_eq!((r.statut, r.location.as_deref()), (303, Some("/alice#a1")));
    assert_eq!(exploits.lock().unwrap().as_slice(), [("alice".to_string(), "blogueur")]);
    let page = texte(&get(&t, "bob", "/alice"));
    for attendu in ["Le blog d'Alice", "<b>Bienvenue</b> ☺", "Mon premier article", "&lt;script&gt;alert(1)&lt;/script&gt;<br>", "<b>gras</b>", "<font color=\"#ff66cc\">rose <span class=\"clignote\">qui clignote", "</span></font>", "/img/alice-1.gif", "#ffd6f0", "J'kiffe ♥ 0"] {
        assert!(page.contains(attendu), "« {attendu} » absent :\n{page}");
    }
    assert!(!page.contains("<script>"));
    // l'image, réduite en GIF
    let img = get(&t, "bob", "/img/alice-1.gif");
    assert_eq!((img.statut, img.type_), (200, "image/gif"));
    assert!(img.corps.starts_with(b"GIF8") && img.corps.len() <= 25 * 1024, "{} octets", img.corps.len());
    // j'kiffe, et j'kiffe plus
    post(&t, "bob", "/alice/kiffe?id=1", &[]);
    assert!(texte(&get(&t, "bob", "/alice")).contains("Je kiffe plus ♥ 1"));
    post(&t, "bob", "/alice/kiffe?id=1", &[]);
    assert!(texte(&get(&t, "bob", "/alice")).contains("J'kiffe ♥ 0"));
    // com's : un toutes les 5 s, supprimables par leur auteur et par l'auteur du blog
    let r = post(&t, "bob", "/alice/com?id=1", &[("texte", "Trop bien ton blog <3")]);
    assert_eq!(r.statut, 303);
    assert_eq!(post(&t, "bob", "/alice/com?id=1", &[("texte", "encore")]).statut, 429);
    let page = texte(&get(&t, "carole", "/alice"));
    assert!(page.contains("<b>bob</b>") && page.contains("Trop bien ton blog <font color=\"#ff3366\">♥</font>"));
    assert!(!page.contains("com/supprimer"), "carole ne peut rien supprimer");
    assert!(texte(&get(&t, "bob", "/alice")).contains("com/supprimer?id=1&amp;k=0"));
    post(&t, "carole", "/alice/com/supprimer?id=1&k=0", &[]);
    assert!(texte(&get(&t, "bob", "/alice")).contains("Trop bien"), "carole n'a rien supprimé");
    post(&t, "alice", "/alice/com/supprimer?id=1&k=0", &[]);
    assert!(!texte(&get(&t, "bob", "/alice")).contains("Trop bien"));
    // l'accueil
    let acc = texte(&get(&t, "bob", "/"));
    assert!(acc.contains("Mon premier article") && acc.contains("Le blog d'Alice"));
    // suppression de l'article (et de son image)
    post(&t, "alice", "/gerer/supprimer?id=1", &[]);
    assert!(texte(&get(&t, "alice", "/alice")).contains("encore vide"));
    assert_eq!(get(&t, "bob", "/img/alice-1.gif").statut, 404);
    // blogs inconnus, noms bizarres
    assert_eq!(get(&t, "bob", "/personne").statut, 404);
    assert_eq!(get(&t, "bob", "/..%2Fetc").statut, 404);
    assert_eq!(get(&t, "bob", "/img/..%2Fcoms.json").statut, 404);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn succes_des_coms_et_des_kiffs() {
    let d = dossier("succes");
    let exploits: Exploits = Default::default();
    let t = Tinyblog::new(d.clone(), exploits.clone());
    article(&t, "alice", "Kiffez-moi", "", None);
    for i in 0..10 {
        post(&t, &format!("fan{i}"), "/alice/kiffe?id=1", &[]);
    }
    for i in 0..5 {
        // (un autre article à chaque fois, pour ne pas attendre 5 s)
        article(&t, &format!("blog{i}"), "Un article", "", None);
        post(&t, "bavard", &format!("/blog{i}/com?id=1"), &[("texte", "Premier !")]);
        t.derniers_coms.lock().unwrap().clear();
    }
    let e = exploits.lock().unwrap().clone();
    assert!(e.contains(&("alice".to_string(), "star")), "{e:?}");
    assert!(e.contains(&("bavard".to_string(), "coms")), "{e:?}");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn mise_en_forme_sans_fuite() {
    use pages::mise_en_forme as m;
    assert_eq!(m("[b]gras"), "<b>gras</b>", "balise refermée");
    assert_eq!(m("[i][b]x[/i] y"), "<i><b>x</b></i> y", "imbrication réparée");
    assert_eq!(m("[couleur=javascript:alert(1)]x[/couleur]"), "[couleur=javascript:alert(1)]x", "couleur inconnue : laissée telle quelle");
    assert_eq!(m("\"><img src=x onerror=alert(1)>"), "&quot;&gt;&lt;img src=x onerror=alert(1)&gt;");
    assert_eq!(m("a\nb"), "a<br>\nb");
}

#[test]
fn web1998_renvoie_vers_tinyblog() {
    let w = crate::web1998::Web1998::new(Default::default(), Vec::new());
    let r = w.handle("/p?u=http%3A%2F%2Ftinyblog%2Falice");
    assert_eq!((r.status, r.location.as_deref()), (302, Some("http://tinyblog/alice")));
    assert!(w.handle("/").body.windows(17).any(|x| x == b"http://tinyblog/\""));
}

#[test]
fn par_le_tunnel_du_fai() {
    use std::io::{Read, Write};
    let d = dossier("tunnel");
    let t = Tinyblog::new(d.clone(), Default::default());
    article(&t, "alice", "Vu par le modem", "", None);
    let policy = crate::tunnel::TunnelPolicy { tinyblog: Some(t), ..Default::default() };
    assert!(policy.is_virtual("tinyblog", 80) && policy.is_virtual("TinyBlog.", 80) && !policy.is_virtual("tinyblog", 8080));
    // le tunnel branche l'hôte virtuel, au nom de l'abonné
    let mut s = crate::tunnel::connect_as("tinyblog", 80, &policy, "bob").expect("hôte virtuel");
    s.write_all(b"GET http://tinyblog/alice HTTP/1.0\r\nHost: tinyblog\r\n\r\n").unwrap();
    let mut r = String::new();
    s.read_to_string(&mut r).unwrap();
    assert!(r.starts_with("HTTP/1.0 200") && r.contains("Vu par le modem") && r.contains("connecté en tant que <b>bob</b>"), "{r}");
    let _ = std::fs::remove_dir_all(d);
}
