//! Parcours complet du site, sans réseau : inscription, validation par le
//! lien de l'e-mail, décision de l'administrateur, page d'admin.

use super::*;
use crate::comptes::Comptes;

struct Banc {
    p: Arc<Portail>,
    comptes: Arc<Comptes>,
    _dir: PathBuf,
}

fn banc() -> Banc {
    let dir = std::env::temp_dir().join(format!("v34-portail-{}", to_hex(&random_bytes::<6>())));
    std::fs::create_dir_all(&dir).unwrap();
    let h = PasswordHash::new("mdp-raziel", 1000);
    std::fs::write(
        dir.join("config.toml"),
        format!("[[abonnes]]\nnom = \"raziel\"\nnumero = \"0142424242\"\nmot_de_passe_hash = \"{h}\"\n"),
    )
    .unwrap();
    let r = Reglages {
        ecoute: "127.0.0.1:0".into(),
        url: "https://dialup.example.org".into(),
        nom: "Tinyverse Dialup".into(),
        central: "dialup.example.org:3434".into(),
        admin: "raziel".into(),
        admin_courriel: "admin@example.org".into(),
        depot: String::new(),
        inscriptions_par_heure: 3,
        config: dir.join("config.toml"),
        comptes: dir.join("comptes.toml"),
        proxy_de_confiance: true,
    };
    let comptes = Comptes::ouvrir(&r.comptes).unwrap();
    let smtp = SmtpConfig { hote: "journal".into(), port: 465, utilisateur: String::new(), mot_de_passe: String::new(), expediteur: "x@example.org".into(), tls_direct: true };
    Banc { p: Portail::new(r, comptes.clone(), Some(smtp)), comptes, _dir: dir }
}

fn get(p: &Portail, url: &str) -> Reponse {
    let (chemin, requete) = url.split_once('?').unwrap_or((url, ""));
    p.repondre(&Requete { methode: "GET".into(), chemin: chemin.into(), requete: requete.into(), ip: "192.0.2.1".into(), ..Default::default() })
}

fn post(p: &Portail, chemin: &str, champs: &[(&str, &str)], cookie: Option<&str>) -> Reponse {
    let corps = form_urlencoded::Serializer::new(String::new()).extend_pairs(champs).finish();
    p.repondre(&Requete { methode: "POST".into(), chemin: chemin.into(), corps: corps.into_bytes(), cookie: cookie.map(String::from), ip: "192.0.2.1".into(), ..Default::default() })
}

fn texte(r: &Reponse) -> String {
    String::from_utf8_lossy(&r.corps).into_owned()
}

/// Jeton de temps du formulaire, vieilli de 10 s (anti-robot).
fn jeton_formulaire(p: &Portail) -> String {
    let ts = (maintenant() - 10).to_string();
    format!("{ts}.{}", &p.signer(&["formulaire", &ts])[..20])
}

fn lettres(p: &Portail) -> Vec<Lettre> {
    std::thread::sleep(Duration::from_millis(50));
    p.postier.boite.lock().unwrap().clone()
}

/// Extrait le premier lien commençant par `prefixe` d'un e-mail.
fn lien(corps: &str, prefixe: &str) -> String {
    let i = corps.find(prefixe).expect("lien dans l'e-mail");
    corps[i..].split_whitespace().next().unwrap().to_string()
}

#[test]
fn parcours_complet() {
    let b = banc();
    let p = &b.p;
    let accueil = texte(&get(p, "/"));
    assert!(accueil.contains("Tinyverse") && accueil.contains("/inscription") && accueil.contains("dialup.example.org:3434"));
    assert!(texte(&get(p, "/inscription")).contains("name=\"numero\""));

    // Erreurs : nom pris (config, insensible à la casse), numéro spécial, mots de passe différents.
    let t = jeton_formulaire(p);
    let r = post(p, "/inscription", &[("t", &t), ("nom", "Raziel"), ("numero", "08 36 65 65 65"), ("courriel", "g@example.org"), ("mdp", "motdepasse"), ("mdp2", "autre-chose"), ("cgu", "1")], None);
    assert_eq!(r.statut, 400);
    let e = texte(&r);
    assert!(e.contains("déjà pris") && e.contains("01 à 07") && e.contains("différents"), "{e}");
    assert!(e.contains("value=\"Raziel\""), "le formulaire garde les saisies");

    // Robot : champ piège, ou formulaire envoyé dans la seconde.
    let r = post(p, "/inscription", &[("t", &t), ("site", "http://spam"), ("nom", "bot"), ("numero", "0123456789"), ("courriel", "b@example.org"), ("mdp", "motdepasse"), ("mdp2", "motdepasse"), ("cgu", "1")], None);
    assert!(texte(&r).contains("Formulaire invalide"));
    let ts = maintenant().to_string();
    let frais = format!("{ts}.{}", &p.signer(&["formulaire", &ts])[..20]);
    let r = post(p, "/inscription", &[("t", &frais), ("nom", "bot"), ("numero", "0123456789"), ("courriel", "b@example.org"), ("mdp", "motdepasse"), ("mdp2", "motdepasse"), ("cgu", "1")], None);
    assert!(texte(&r).contains("Formulaire invalide"));

    // Inscription correcte : e-mail de validation.
    let r = post(p, "/inscription", &[("t", &t), ("nom", "gaston"), ("numero", "06.12.34.56.78"), ("courriel", "gaston@example.org"), ("mdp", "motdepasse"), ("mdp2", "motdepasse"), ("cgu", "1")], None);
    assert_eq!(r.statut, 200, "{}", texte(&r));
    assert!(texte(&r).contains("06 12 34 56 78"));
    let l = lettres(p);
    assert_eq!(l.len(), 1);
    assert_eq!(l[0].a, "gaston@example.org");
    let valider = lien(&l[0].corps, "https://dialup.example.org/valider?t=");
    let jeton = valider.split("t=").nth(1).unwrap();
    // Numéro réservé pendant l'attente.
    let r = post(p, "/inscription", &[("t", &t), ("nom", "lagaffe"), ("numero", "0612345678"), ("courriel", "l@example.org"), ("mdp", "motdepasse"), ("mdp2", "motdepasse"), ("cgu", "1")], None);
    assert!(texte(&r).contains("déjà attribué"));

    // Le lien (GET) n'active rien tout seul : il faut le bouton (POST).
    assert!(texte(&get(p, &format!("/valider?t={jeton}"))).contains("Activer mon abonnement"));
    assert!(b.comptes.liste().is_empty());
    let r = post(p, "/valider", &[("t", jeton)], None);
    assert!(texte(&r).contains("Bienvenue, gaston") && texte(&r).contains("en attente"), "{}", texte(&r));
    let g = b.comptes.compte("gaston").unwrap();
    assert!(!g.fai, "pas d'Internet sans l'accord de l'administrateur");

    // E-mail à l'administrateur : liens signés [Accorder] / [Refuser].
    let l = lettres(p);
    assert_eq!(l[1].a, "admin@example.org");
    let accorder = lien(&l[1].corps, "https://dialup.example.org/decision?");
    assert!(accorder.contains("d=oui"));
    let q = accorder.split_once('?').unwrap().1;
    // Signature falsifiée : refusée.
    let r = get(p, &format!("/decision?{}", q.replace("d=oui", "d=non")));
    assert_eq!(r.statut, 403);
    // Le lien affiche une confirmation ; le bouton applique.
    let r = get(p, &format!("/decision?{q}"));
    assert!(texte(&r).contains("Accorder l'accès Internet"));
    let champs: Vec<(String, String)> = form_urlencoded::parse(q.as_bytes()).into_owned().collect();
    let champs: Vec<(&str, &str)> = champs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let r = post(p, "/decision", &champs, None);
    assert_eq!(r.statut, 200);
    assert!(b.comptes.compte("gaston").unwrap().fai);
    let l = lettres(p);
    assert!(l[2].a == "gaston@example.org" && l[2].sujet.contains("activé"));

    // Admin : mauvais mot de passe, puis connexion, puis action avec jeton CSRF.
    assert_eq!(post(p, "/admin/connexion", &[("mdp", "faux")], None).statut, 403);
    let r = post(p, "/admin/connexion", &[("mdp", "mdp-raziel")], None);
    assert_eq!(r.statut, 303);
    let cookie = r.entetes.iter().find(|(k, _)| *k == "Set-Cookie").unwrap().1.split(';').next().unwrap().to_string();
    assert!(r.entetes.iter().any(|(k, v)| *k == "Set-Cookie" && v.contains("HttpOnly") && v.contains("SameSite=Strict") && v.contains("Secure")));
    let admin = p.repondre(&Requete { methode: "GET".into(), chemin: "/admin".into(), cookie: Some(cookie.clone()), ..Default::default() });
    let page = texte(&admin);
    assert!(page.contains("gaston") && page.contains("raziel") && page.contains("Retirer"), "{page}");
    let csrf = page.split("name=\"csrf\" value=\"").nth(1).unwrap().split('"').next().unwrap().to_string();
    assert_eq!(post(p, "/admin/fai", &[("nom", "gaston"), ("d", "non"), ("csrf", "faux")], Some(&cookie)).statut, 403);
    let r = post(p, "/admin/fai", &[("nom", "gaston"), ("d", "non"), ("csrf", &csrf)], Some(&cookie));
    assert!(texte(&r).contains("retiré"));
    assert!(!b.comptes.compte("gaston").unwrap().fai);
    let r = post(p, "/admin/supprimer", &[("nom", "gaston"), ("sur", "1"), ("csrf", &csrf)], Some(&cookie));
    assert!(texte(&r).contains("supprimé"));
    assert!(b.comptes.liste().is_empty());
    // Sans session : renvoyé à la connexion.
    assert_eq!(post(p, "/admin/fai", &[("nom", "x"), ("d", "oui"), ("csrf", &csrf)], None).statut, 303);
}

#[test]
fn limite_par_adresse_et_echappement() {
    let b = banc();
    let p = &b.p;
    let t = jeton_formulaire(p);
    for i in 0..3 {
        let num = format!("060000000{i}");
        let r = post(p, "/inscription", &[("t", &t), ("nom", &format!("n{i}")), ("numero", &num), ("courriel", &format!("n{i}@example.org")), ("mdp", "motdepasse"), ("mdp2", "motdepasse"), ("cgu", "1")], None);
        assert_eq!(r.statut, 200);
    }
    let r = post(p, "/inscription", &[("t", &t), ("nom", "n9"), ("numero", "0600000009"), ("courriel", "n9@example.org"), ("mdp", "motdepasse"), ("mdp2", "motdepasse"), ("cgu", "1")], None);
    assert!(texte(&r).contains("Trop d&#39;inscriptions"), "{}", texte(&r));
    // Saisie hostile renvoyée échappée.
    let r = post(p, "/inscription", &[("t", &t), ("nom", "<script>"), ("numero", "1"), ("courriel", "\"><img>"), ("mdp", "x"), ("mdp2", "x")], None);
    let h = texte(&r);
    assert!(!h.contains("<script>") && !h.contains("\"><img>") && h.contains("&lt;script&gt;"));
}

#[test]
fn fichiers_classes_par_systeme() {
    assert_eq!(classer("V34 Modem_0.1.0_aarch64.dmg"), Some("Macintosh (Apple Silicon)"));
    assert_eq!(classer("V34 Modem_0.1.0_x64.dmg"), Some("Macintosh (Intel)"));
    assert_eq!(classer("V34 Modem_0.1.0_x64-setup.exe"), Some("Windows 10/11 (installateur)"));
    assert_eq!(classer("V34 Modem_0.1.0_x64_fr-FR.msi"), Some("Windows 10/11 (MSI)"));
    assert_eq!(classer("V34 Modem_0.1.0_amd64.AppImage"), Some("Linux (AppImage)"));
    assert_eq!(classer("v34-modem_0.1.0_amd64.deb"), Some("Linux (Debian, Ubuntu)"));
    assert_eq!(classer("V34 Modem_0.1.0_amd64.AppImage.sig"), None);
    assert!(courriel_valide("a.b+c@example.org") && !courriel_valide("pas-une-adresse") && !courriel_valide("a@b") && !courriel_valide("a b@c.fr"));
}
