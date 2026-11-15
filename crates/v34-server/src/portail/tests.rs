//! Parcours complets du site, sans réseau : inscription, validation par le
//! lien de l'e-mail, décision de l'administrateur, page d'admin, espace
//! abonné, mot de passe oublié, livre d'or, état du réseau, pages légales.

use super::*;
use crate::comptes::Comptes;
use livre::Livre;

struct Banc {
    p: Arc<Portail>,
    comptes: Arc<Comptes>,
    dir: PathBuf,
}

impl Drop for Banc {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn banc() -> Banc {
    let dir = std::env::temp_dir().join(format!("v34-portail-{}", to_hex(&random_bytes::<6>())));
    std::fs::create_dir_all(&dir).unwrap();
    let h = PasswordHash::new("mdp-raziel", 1000);
    let h2 = PasswordHash::new("mdp-prunelle", 1000);
    std::fs::write(
        dir.join("config.toml"),
        format!(
            "[[abonnes]]\nnom = \"raziel\"\nnumero = \"0142424242\"\nmot_de_passe_hash = \"{h}\"\n\n\
             [[abonnes]]\nnom = \"prunelle\"\nnumero = \"0143434343\"\nmot_de_passe_hash = \"{h2}\"\nfai = false\n"
        ),
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
        editeur: EDITEUR_PAR_DEFAUT.into(),
        contact: "contact@example.org".into(),
        hebergeur: "Hébergeur SAS\n1 rue du Modem\n75000 Paris".into(),
        etat: dir.join("etat.json"),
        livre: dir.join("livre-dor.toml"),
    };
    let comptes = Comptes::ouvrir(&r.comptes).unwrap();
    let livre = Livre::ouvrir(&r.livre).unwrap();
    let smtp = SmtpConfig { hote: "journal".into(), port: 465, utilisateur: String::new(), mot_de_passe: String::new(), expediteur: "x@example.org".into(), tls_direct: true };
    Banc { p: Portail::new(r, comptes.clone(), livre, Some(smtp)), comptes, dir }
}

fn get(p: &Portail, url: &str) -> Reponse {
    let (chemin, requete) = url.split_once('?').unwrap_or((url, ""));
    p.repondre(&Requete { methode: "GET".into(), chemin: chemin.into(), requete: requete.into(), ip: "192.0.2.1".into(), ..Default::default() })
}

fn post(p: &Portail, chemin: &str, champs: &[(&str, &str)], cookie: Option<&str>) -> Reponse {
    post_depuis(p, chemin, champs, cookie, "192.0.2.1")
}

fn post_depuis(p: &Portail, chemin: &str, champs: &[(&str, &str)], cookie: Option<&str>, ip: &str) -> Reponse {
    let corps = form_urlencoded::Serializer::new(String::new()).extend_pairs(champs).finish();
    p.repondre(&Requete { methode: "POST".into(), chemin: chemin.into(), corps: corps.into_bytes(), cookie: cookie.map(String::from), ip: ip.into(), ..Default::default() })
}

fn get_avec(p: &Portail, url: &str, cookie: &str) -> Reponse {
    let (chemin, requete) = url.split_once('?').unwrap_or((url, ""));
    p.repondre(&Requete { methode: "GET".into(), chemin: chemin.into(), requete: requete.into(), cookie: Some(cookie.into()), ip: "192.0.2.1".into(), ..Default::default() })
}

fn entete<'a>(r: &'a Reponse, nom: &str) -> Option<&'a str> {
    r.entetes.iter().find(|(k, _)| *k == nom).map(|(_, v)| v.as_str())
}

/// « nom=valeur » du cookie posé par la réponse.
fn cookie(r: &Reponse) -> String {
    entete(r, "Set-Cookie").expect("cookie").split(';').next().unwrap().to_string()
}

/// Valeur du champ caché `csrf` d'une page.
fn csrf(page: &str) -> String {
    page.split("name=\"csrf\" value=\"").nth(1).expect("jeton CSRF").split('"').next().unwrap().to_string()
}

/// Compte du site créé directement (empreinte légère, pour aller vite).
fn compte(b: &Banc, nom: &str, numero: &str, courriel: &str, mdp: &str) {
    let j = b.comptes.inscrire(nom, numero, &PasswordHash::new(mdp, 1000).to_string(), courriel).unwrap();
    b.comptes.valider(&j, |_, _| true).unwrap();
}

/// Le mot de passe du compte du site est-il celui-là ?
fn mdp_est(b: &Banc, nom: &str, mdp: &str) -> bool {
    let h = PasswordHash::parse(&b.comptes.compte(nom).unwrap().mot_de_passe_hash).unwrap();
    PasswordHash::with_salt(mdp, &h.salt, h.iterations).stored_key == h.stored_key
}

/// Connexion à l'espace abonné : cookie de session.
fn connexion(p: &Portail, nom: &str, mdp: &str, ip: &str) -> String {
    let r = post_depuis(p, "/compte/connexion", &[("nom", nom), ("mdp", mdp)], None, ip);
    assert_eq!(r.statut, 303, "{}", texte(&r));
    cookie(&r)
}

fn texte(r: &Reponse) -> String {
    String::from_utf8_lossy(&r.corps).into_owned()
}

/// Jeton de temps du formulaire d'inscription, vieilli de 10 s (anti-robot).
fn jeton_formulaire(p: &Portail) -> String {
    jeton(p, "formulaire")
}

/// Jeton de temps d'un formulaire public, vieilli de 10 s.
fn jeton(p: &Portail, usage: &str) -> String {
    let ts = (maintenant() - 10).to_string();
    format!("{ts}.{}", &p.signer(&[usage, &ts])[..20])
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
    assert!(accueil.contains("08 36 15 42 86") && accueil.contains("56K") && accueil.contains("Visiophone"));
    let aide = texte(&get(p, "/aide"));
    assert!(aide.contains("TINYVERSE") && aide.contains("DIP 1") && aide.contains("UDP 3434"), "{aide}");
    assert!(texte(&get(p, "/inscription")).contains("name=\"numero\""));

    // Erreurs : nom pris (config, insensible à la casse), numéro spécial, mots de passe différents.
    let t = jeton_formulaire(p);
    let r = post(p, "/inscription", &[("t", &t), ("nom", "Raziel"), ("numero", "08 36 15 42 86"), ("courriel", "g@example.org"), ("mdp", "motdepasse"), ("mdp2", "autre-chose"), ("cgu", "1")], None);
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
    assert!(l[2].corps.contains("TINYVERSE") && l[2].corps.contains("08 36 15 42 86"), "le FAI s'appelle depuis l'annuaire : {}", l[2].corps);

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

#[test]
fn espace_abonne_connexion_et_sessions() {
    let b = banc();
    let p = &b.p;
    let page = texte(&get(p, "/compte"));
    assert!(page.contains("action=\"/compte/connexion\"") && page.contains("href=\"/oubli\""), "{page}");
    compte(&b, "gaston", "0612345678", "gaston@example.org", "motdepasse");

    // Mauvais mot de passe et nom inconnu : même réponse.
    let r = post(p, "/compte/connexion", &[("nom", "gaston"), ("mdp", "faux-faux")], None);
    assert_eq!(r.statut, 403);
    let r2 = post(p, "/compte/connexion", &[("nom", "inconnu"), ("mdp", "faux-faux")], None);
    assert_eq!(r2.statut, 403);
    assert!(texte(&r).contains("Nom d&#39;abonné ou mot de passe incorrect") && texte(&r2).contains("Nom d&#39;abonné ou mot de passe incorrect"));
    assert!(entete(&r, "Set-Cookie").is_none());

    // Connexion : cookie de session propre à l'espace abonné.
    let r = post(p, "/compte/connexion", &[("nom", "gaston"), ("mdp", "motdepasse")], None);
    assert_eq!((r.statut, entete(&r, "Location")), (303, Some("/compte")));
    let c = entete(&r, "Set-Cookie").unwrap();
    assert!(c.starts_with("v34abo=") && c.contains("HttpOnly") && c.contains("SameSite=Strict") && c.contains("Secure") && c.contains("Max-Age=43200"), "{c}");
    let session = cookie(&r);
    let page = texte(&get_avec(p, "/compte", &session));
    for attendu in ["gaston", "06 12 34 56 78", "gaston@example.org", "en attente de décision", "dialup.example.org:3434", "/compte/mdp", "/compte/supprimer", "/compte/deconnexion"] {
        assert!(page.contains(attendu), "{attendu} absent : {page}");
    }
    b.comptes.regler_fai("gaston", false).unwrap();
    assert!(texte(&get_avec(p, "/compte", &session)).contains("<b>refusé</b>"));
    b.comptes.regler_fai("gaston", true).unwrap();
    assert!(texte(&get_avec(p, "/compte", &session)).contains("<b>accordé</b>"));

    // Sessions distinctes de celles de l'administration, dans les deux sens.
    let jeton = session.split_once('=').unwrap().1;
    assert!(!texte(&get_avec(p, "/admin", &format!("v34adm={jeton}"))).contains("Abonnés inscrits"));
    let r = post(p, "/admin/connexion", &[("mdp", "mdp-raziel")], None);
    let admin = cookie(&r);
    let jeton_admin = admin.split_once('=').unwrap().1;
    assert!(texte(&get_avec(p, "/compte", &format!("v34abo={jeton_admin}"))).contains("action=\"/compte/connexion\""));

    // Abonné de la configuration : lecture seule, drapeau « fai » de la configuration.
    let raziel = connexion(p, "raziel", "mdp-raziel", "192.0.2.2");
    let page = texte(&get_avec(p, "/compte", &raziel));
    assert!(page.contains("géré par l'opérateur") && page.contains("<b>accordé</b>") && page.contains("01 42 42 42 42"), "{page}");
    assert!(!page.contains("/compte/supprimer") && !page.contains("/compte/mdp"));
    let r = post(p, "/compte/mdp", &[("csrf", &csrf(&page)), ("mdp", "mdp-raziel"), ("nouveau", "nouveau-mdp"), ("nouveau2", "nouveau-mdp")], Some(&raziel));
    assert_eq!(r.statut, 403);
    let prunelle = connexion(p, "prunelle", "mdp-prunelle", "192.0.2.2");
    assert!(texte(&get_avec(p, "/compte", &prunelle)).contains("<b>refusé</b>"), "fai = false dans la configuration");

    // Déconnexion (POST + CSRF) : la session est oubliée.
    let page = texte(&get_avec(p, "/compte", &session));
    assert_eq!(post(p, "/compte/deconnexion", &[("csrf", "faux")], Some(&session)).statut, 403);
    let r = post(p, "/compte/deconnexion", &[("csrf", &csrf(&page))], Some(&session));
    assert_eq!((r.statut, entete(&r, "Location")), (303, Some("/")));
    assert!(entete(&r, "Set-Cookie").unwrap().contains("Max-Age=0"));
    assert!(texte(&get_avec(p, "/compte", &session)).contains("action=\"/compte/connexion\""));

    // Au plus 5 essais par adresse IP et par quart d'heure (même avec le bon mot de passe).
    for _ in 0..5 {
        assert_ne!(post_depuis(p, "/compte/connexion", &[("nom", "gaston"), ("mdp", "faux-faux")], None, "192.0.2.9").statut, 429);
    }
    let r = post_depuis(p, "/compte/connexion", &[("nom", "gaston"), ("mdp", "motdepasse")], None, "192.0.2.9");
    assert_eq!(r.statut, 429);
    assert!(texte(&r).contains("Trop d&#39;essais"));
}

#[test]
fn changement_de_mot_de_passe_avec_csrf() {
    let b = banc();
    let p = &b.p;
    compte(&b, "gaston", "0612345678", "gaston@example.org", "motdepasse");
    let session = connexion(p, "gaston", "motdepasse", "192.0.2.1");
    let autre = connexion(p, "gaston", "motdepasse", "192.0.2.2");
    let jeton_csrf = csrf(&texte(&get_avec(p, "/compte", &session)));
    let changer = |csrf: &str, actuel: &str, n1: &str, n2: &str, cookie: Option<&str>| post(p, "/compte/mdp", &[("csrf", csrf), ("mdp", actuel), ("nouveau", n1), ("nouveau2", n2)], cookie);

    assert_eq!(changer("faux", "motdepasse", "nouveau-mdp", "nouveau-mdp", Some(&session)).statut, 403, "CSRF invalide");
    assert_eq!(changer(&csrf(&texte(&get_avec(p, "/compte", &autre))), "motdepasse", "nouveau-mdp", "nouveau-mdp", Some(&session)).statut, 403, "jeton d'une autre session");
    assert_eq!(changer(&jeton_csrf, "motdepasse", "nouveau-mdp", "nouveau-mdp", None).statut, 303, "sans session");
    let r = changer(&jeton_csrf, "pas-le-bon", "nouveau-mdp", "nouveau-mdp", Some(&session));
    assert!(r.statut == 403 && texte(&r).contains("actuel incorrect"));
    let r = changer(&jeton_csrf, "motdepasse", "court", "court", Some(&session));
    assert!(r.statut == 400 && texte(&r).contains("8 à 128"));
    let r = changer(&jeton_csrf, "motdepasse", "nouveau-mdp", "nouveau-mdx", Some(&session));
    assert!(r.statut == 400 && texte(&r).contains("différents"));
    assert!(mdp_est(&b, "gaston", "motdepasse") && lettres(p).is_empty(), "rien de changé jusqu'ici");

    let r = changer(&jeton_csrf, "motdepasse", "nouveau-mdp", "nouveau-mdp", Some(&session));
    assert_eq!(r.statut, 200, "{}", texte(&r));
    assert!(texte(&r).contains("Mot de passe changé"));
    assert!(mdp_est(&b, "gaston", "nouveau-mdp"));
    let h = PasswordHash::parse(&b.comptes.compte("gaston").unwrap().mot_de_passe_hash).unwrap();
    assert_eq!(h.iterations, DEFAULT_ITERATIONS);
    let l = lettres(p);
    assert_eq!(l.len(), 1);
    assert!(l[0].a == "gaston@example.org" && l[0].sujet.contains("mot de passe changé") && l[0].corps.contains("/oubli"), "{:?}", l[0]);
    // La session en cours reste ouverte, les autres sont fermées.
    assert!(texte(&get_avec(p, "/compte", &session)).contains("/compte/mdp"));
    assert!(texte(&get_avec(p, "/compte", &autre)).contains("action=\"/compte/connexion\""));
}

#[test]
fn mot_de_passe_oublie_parcours_complet() {
    let b = banc();
    let p = &b.p;
    compte(&b, "gaston", "0612345678", "gaston@example.org", "motdepasse");
    let ouverte = connexion(p, "gaston", "motdepasse", "192.0.2.7");
    let formulaire = texte(&get(p, "/oubli"));
    assert!(formulaire.contains("action=\"/oubli\"") && formulaire.contains("name=\"t\"") && formulaire.contains("name=\"site\""));
    let t = jeton(p, "oubli");
    let demander = |courriel: &str| post(p, "/oubli", &[("t", &t), ("courriel", courriel)], None);

    // Adresse inconnue : réponse standard, aucun e-mail.
    let inconnue = demander("personne@example.org");
    assert_eq!(inconnue.statut, 200);
    assert!(lettres(p).is_empty());
    // Adresse connue (majuscules comprises) : MÊME page, et un e-mail.
    let connue = demander("Gaston@Example.org");
    assert_eq!(connue.statut, 200);
    assert_eq!(texte(&inconnue).replace("personne@example.org", "X"), texte(&connue).replace("Gaston@Example.org", "X"), "pas d'énumération des adresses");
    let l = lettres(p);
    assert_eq!(l.len(), 1);
    assert_eq!(l[0].a, "gaston@example.org");
    let lien_reinit = lien(&l[0].corps, "https://dialup.example.org/reinitialiser?t=");
    let j = lien_reinit.split("t=").nth(1).unwrap().to_string();
    assert!(!std::fs::read_to_string(b.dir.join("comptes.toml")).unwrap().contains(&j), "jeton jamais stocké en clair");

    // Robot : jeton trop frais.
    let ts = maintenant().to_string();
    let frais = format!("{ts}.{}", &p.signer(&["oubli", &ts])[..20]);
    assert!(texte(&post(p, "/oubli", &[("t", &frais), ("courriel", "gaston@example.org")], None)).contains("Formulaire invalide"));

    // Le lien affiche le formulaire ; le POST applique.
    assert_eq!(get(p, "/reinitialiser?t=faux").statut, 404);
    let page = texte(&get(p, &format!("/reinitialiser?t={j}")));
    assert!(page.contains("gaston") && page.contains("name=\"mdp2\""), "{page}");
    let r = post(p, "/reinitialiser", &[("t", &j), ("mdp", "nouveau-mdp"), ("mdp2", "autre-chose")], None);
    assert!(r.statut == 400 && texte(&r).contains("différents"));
    assert!(mdp_est(&b, "gaston", "motdepasse"));
    let r = post(p, "/reinitialiser", &[("t", &j), ("mdp", "nouveau-mdp"), ("mdp2", "nouveau-mdp")], None);
    assert_eq!(r.statut, 200, "{}", texte(&r));
    assert!(mdp_est(&b, "gaston", "nouveau-mdp"));
    let l = lettres(p);
    assert!(l.len() == 2 && l[1].sujet.contains("mot de passe changé"), "titulaire prévenu : {l:?}");
    // Usage unique, et les sessions ouvertes avant sont fermées.
    assert_eq!(post(p, "/reinitialiser", &[("t", &j), ("mdp", "encore-autre"), ("mdp2", "encore-autre")], None).statut, 404);
    assert_eq!(get(p, &format!("/reinitialiser?t={j}")).statut, 404);
    assert!(texte(&get_avec(p, "/compte", &ouverte)).contains("action=\"/compte/connexion\""));

    // Un changement de mot de passe depuis l'espace abonné annule le lien en cours.
    demander("gaston@example.org");
    let l = lettres(p);
    let j2 = lien(&l[2].corps, "https://dialup.example.org/reinitialiser?t=").split("t=").nth(1).unwrap().to_string();
    assert_eq!(get(p, &format!("/reinitialiser?t={j2}")).statut, 200);
    let session = connexion(p, "gaston", "nouveau-mdp", "192.0.2.1");
    let c = csrf(&texte(&get_avec(p, "/compte", &session)));
    assert_eq!(post(p, "/compte/mdp", &[("csrf", &c), ("mdp", "nouveau-mdp"), ("nouveau", "troisieme-mdp"), ("nouveau2", "troisieme-mdp")], Some(&session)).statut, 200);
    assert_eq!(get(p, &format!("/reinitialiser?t={j2}")).statut, 404);

    // Limite par adresse IP (3 demandes par heure), même réponse pour toutes les adresses.
    let r = demander("personne@example.org");
    assert!(r.statut == 429 && texte(&r).contains("Trop de demandes"));
}

#[test]
fn suppression_de_compte_par_l_abonne() {
    let b = banc();
    let p = &b.p;
    compte(&b, "gaston", "0612345678", "gaston@example.org", "motdepasse");
    b.comptes.demander_reinitialisation("gaston").unwrap();
    let session = connexion(p, "gaston", "motdepasse", "192.0.2.1");
    let c = csrf(&texte(&get_avec(p, "/compte", &session)));
    assert_eq!(post(p, "/compte/supprimer", &[("csrf", "faux"), ("mdp", "motdepasse"), ("sur", "1")], Some(&session)).statut, 403);
    let r = post(p, "/compte/supprimer", &[("csrf", &c), ("mdp", "motdepasse")], Some(&session));
    assert!(r.statut == 400 && texte(&r).contains("Cochez la case"));
    assert_eq!(post(p, "/compte/supprimer", &[("csrf", &c), ("mdp", "pas-le-bon"), ("sur", "1")], Some(&session)).statut, 403);
    assert!(b.comptes.compte("gaston").is_some());

    let r = post(p, "/compte/supprimer", &[("csrf", &c), ("mdp", "motdepasse"), ("sur", "1")], Some(&session));
    assert_eq!(r.statut, 200, "{}", texte(&r));
    assert!(texte(&r).contains("06 12 34 56 78") && entete(&r, "Set-Cookie").unwrap().contains("Max-Age=0"));
    assert!(b.comptes.compte("gaston").is_none());
    assert_eq!(b.comptes.conflit("autre", "0612345678", "autre@example.org"), None, "numéro libéré");
    let fichier = std::fs::read_to_string(b.dir.join("comptes.toml")).unwrap();
    assert!(!fichier.contains("gaston"), "plus aucune trace dans le fichier : {fichier}");
    let l = lettres(p);
    assert!(l.len() == 1 && l[0].sujet.contains("résilié"));
    assert!(texte(&get_avec(p, "/compte", &session)).contains("action=\"/compte/connexion\""));
}

#[test]
fn livre_d_or_avec_moderation() {
    let b = banc();
    let p = &b.p;
    assert!(texte(&get(p, "/livre")).contains("encore vierge"));
    let t = jeton(p, "livre");
    let signer = |pseudo: &str, message: &str| post(p, "/livre", &[("t", &t), ("pseudo", pseudo), ("message", message)], None);

    // Robots, saisies invalides.
    let r = post(p, "/livre", &[("t", &t), ("site", "http://spam"), ("pseudo", "bot"), ("message", "achetez")], None);
    assert!(r.statut == 400 && texte(&r).contains("Formulaire invalide"));
    assert!(post(p, "/livre", &[("t", &jeton(p, "formulaire")), ("pseudo", "bot"), ("message", "x")], None).statut == 400, "jeton d'un autre formulaire");
    let r = signer(&"x".repeat(33), "trop long");
    assert!(r.statut == 400 && texte(&r).contains("Pseudo"));
    assert_eq!(signer("gaston", &"y".repeat(501)).statut, 400);

    // Message en attente de relecture : pas encore affiché.
    let r = signer("Gaston <b>", "Super site !\n<script>alert(1)</script>");
    assert_eq!(r.statut, 200, "{}", texte(&r));
    assert!(texte(&r).contains("Merci"));
    assert!(!texte(&get(p, "/livre")).contains("Super site"));

    // Modération dans /admin : Publier (CSRF).
    let admin = cookie(&post(p, "/admin/connexion", &[("mdp", "mdp-raziel")], None));
    let page = texte(&get_avec(p, "/admin/livre", &admin));
    assert!(page.contains("messages à relire (1)") && page.contains("&lt;script&gt;") && !page.contains("<script>"), "{page}");
    let id = page.split("name=\"id\" value=\"").nth(1).unwrap().split('"').next().unwrap().to_string();
    assert_eq!(post(p, "/admin/livre/publier", &[("csrf", "faux"), ("id", &id)], Some(&admin)).statut, 403);
    assert_eq!(post(p, "/admin/livre/publier", &[("csrf", &csrf(&page)), ("id", &id)], None).statut, 303, "sans session");
    let r = post(p, "/admin/livre/publier", &[("csrf", &csrf(&page)), ("id", &id)], Some(&admin));
    assert!(r.statut == 200 && texte(&r).contains("publié"));
    let livre = texte(&get(p, "/livre"));
    assert!(livre.contains("Gaston &lt;b&gt;") && livre.contains("Super site !<br>&lt;script&gt;") && livre.contains("1 message dans le livre"), "{livre}");
    assert!(!livre.contains("<script>"));
    assert!(b.dir.join("livre-dor.toml").exists());

    // Du plus récent au plus ancien ; 3 messages par heure et par adresse.
    assert_eq!(signer("lagaffe", "Deuxième message").statut, 200);
    assert_eq!(signer("prunelle", "Troisième message").statut, 200);
    let r = signer("lagaffe", "Quatrième, de trop");
    assert_eq!(r.statut, 429);
    assert_eq!(post_depuis(p, "/livre", &[("t", &t), ("pseudo", "voisin"), ("message", "Bonjour")], None, "192.0.2.50").statut, 200, "autre adresse");
    for m in p.livre.en_attente() {
        p.livre.publier(&m.id).unwrap();
    }
    let livre = texte(&get(p, "/livre"));
    let (i1, i2) = (livre.find("Bonjour").unwrap(), livre.find("Super site").unwrap());
    assert!(i1 < i2, "le plus récent d'abord");

    // Suppression (CSRF).
    let page = texte(&get_avec(p, "/admin/livre", &admin));
    let r = post(p, "/admin/livre/supprimer", &[("csrf", &csrf(&page)), ("id", &id)], Some(&admin));
    assert!(r.statut == 200 && texte(&r).contains("supprimé"));
    assert!(!texte(&get(p, "/livre")).contains("Super site"));
}

#[test]
fn etat_du_reseau_frais_et_perime() {
    let b = banc();
    let p = &b.p;
    let fichier = b.dir.join("etat.json");
    let ecrire = |maj: u64| {
        std::fs::write(
            &fichier,
            format!(r#"{{"version":"0.1.1","depuis":{},"maj":{maj},"abonnes":42,"connectes":3,"appels":1,"modems":{{"occupes":1,"max":4}},"salon":2}}"#, maj - 93_784),
        )
        .unwrap();
    };
    // Fichier absent.
    let r = get(p, "/etat");
    assert!(r.statut == 503 && texte(&r).contains("HORS SERVICE"));
    // Fichier frais.
    ecrire(maintenant());
    let r = get(p, "/etat");
    let h = texte(&r);
    assert_eq!(r.statut, 200, "{h}");
    for attendu in ["EN SERVICE", "1 j 02 h 03 min", "version 0.1.1", "<b>3</b> sur 42 abonnés", "<b>1 communication</b>", "<b>3 libres</b>, 1 occupé", "<b>2 présents</b>"] {
        assert!(h.contains(attendu), "{attendu} absent : {h}");
    }
    assert_eq!(h.matches("voyant v-rouge").count() - h.matches("<span class=\"voyant v-rouge\"></span> occupé ou hors service").count(), 1, "un voyant rouge par modem occupé");
    assert_eq!(entete(&r, "Refresh"), Some("30"));
    // Fichier périmé (plus de 30 s), puis illisible.
    ecrire(maintenant() - 31);
    let r = get(p, "/etat");
    assert!(r.statut == 503 && texte(&r).contains("HORS SERVICE") && !texte(&r).contains("EN SERVICE"));
    std::fs::write(&fichier, "{pas du json").unwrap();
    assert_eq!(get(p, "/etat").statut, 503);
    // Champs inconnus ignorés ; date dans le futur lointain refusée.
    let now = maintenant();
    std::fs::write(&fichier, format!(r#"{{"maj":{now},"nouveau":true}}"#)).unwrap();
    assert!(etat::lire(&fichier, now).is_some());
    assert!(etat::lire(&fichier, now - 120).is_none());
}

#[test]
fn pages_legales_presentes_et_liees_partout() {
    let b = banc();
    let p = &b.p;
    for chemin in ["/", "/inscription", "/compte", "/oubli", "/telechargement", "/aide", "/admin", "/mentions", "/conditions", "/confidentialite", "/etat", "/livre", "/webring", "/page-inexistante"] {
        let h = texte(&get(p, chemin));
        for lien in ["href=\"/mentions\"", "href=\"/conditions\"", "href=\"/confidentialite\"", "href=\"/compte\">Mon compte", "href=\"/etat\"", "href=\"/livre\"", "href=\"/webring/hasard\"", "class=\"badge b-ns\"", "Modem V.34 inside", "<kbd>Ctrl</kbd>+<kbd>D</kbd>"] {
            assert!(h.contains(lien), "{chemin} : {lien} absent");
        }
    }
    assert!(texte(&get(p, "/inscription")).contains("J'accepte les <a href=\"/conditions\""));

    let m = texte(&get(p, "/mentions"));
    assert!(m.contains("Un particulier — identité communiquée à l&#39;hébergeur (LCEN, art. 6-III-2)"), "{m}");
    assert!(m.contains("mailto:contact@example.org") && m.contains("Hébergeur SAS<br>1 rue du Modem<br>75000 Paris"));
    let c = texte(&get(p, "/conditions"));
    for attendu in ["gratuit", "sans garantie", "Usage légal", "adresse IP du serveur de l'opérateur", "retirer l'accès", "monopolisez"] {
        assert!(c.contains(attendu), "conditions : {attendu} absent");
    }
    let v = texte(&get(p, "/confidentialite"));
    for attendu in ["empreinte", "48 heures", "jusqu'à sa suppression", "durée de rétention", "débogage", "CNIL", "v34abo", "strictement nécessaires", "GitHub", "serveur d'envoi d'e-mails", "effacement", "3615 ORACLE", "OpenRouter", "DeepSeek", "le temps de l'appel", "suspension éventuelle", "qui est en ligne"] {
        assert!(v.contains(attendu), "confidentialité : {attendu} absent");
    }
    // 3615 ORACLE dans le tableau des services et dans l'aide
    assert!(texte(&get(p, "/")).contains("3615 ORACLE"));
    let a = texte(&get(p, "/aide"));
    assert!(a.contains("<h3>3615 ORACLE</h3>") && a.contains("08 36 67 22 53") && a.contains("/fin"));

    // Webring : redirection au hasard vers un membre.
    let r = get(p, "/webring/hasard");
    assert_eq!(r.statut, 303);
    assert!(pages::WEBRING.iter().any(|(u, _, _)| entete(&r, "Location") == Some(*u)));
    assert!(texte(&get(p, "/webring")).contains("https://wiby.me/"));
    // Accueil : « Quoi de neuf ? » avec l'ouverture du site (pas de dépôt configuré).
    assert!(texte(&get(p, "/")).contains("Quoi de neuf ?") && texte(&get(p, "/")).contains("14/11/2026"));
    assert!(texte(&get(p, "/robots.txt")).contains("Disallow: /compte"));
}

#[test]
fn quoi_de_neuf_d_apres_github() {
    let v: serde_json::Value = serde_json::json!([
        {"name": "", "tag_name": "v0.3.0", "published_at": "2026-12-01T10:00:00Z", "draft": true, "html_url": "https://github.com/a/b/releases/v0.3.0"},
        {"name": "V34 Modem 0.2.0", "tag_name": "v0.2.0", "published_at": "2026-11-20T10:00:00Z", "prerelease": true, "html_url": "https://github.com/a/b/releases/v0.2.0"},
        {"name": "", "tag_name": "v0.1.1", "published_at": "2026-11-15T08:00:00Z", "html_url": "javascript:alert(1)"},
        {"name": "v0.1.0", "tag_name": "v0.1.0", "published_at": "2026-11-10T08:00:00Z"},
        {"name": "v0.0.9", "tag_name": "v0.0.9", "published_at": "pas une date"},
        {"name": "v0.0.8", "tag_name": "v0.0.8", "published_at": "2026-11-01T08:00:00Z"}
    ]);
    let n = analyser_nouvelles(&v);
    assert_eq!(n.len(), 3, "{n:?}");
    assert_eq!(n[0].titre, "Nouvelle version du logiciel : V34 Modem 0.2.0 (préversion)");
    assert_eq!((n[1].titre.as_str(), n[1].lien.as_str()), ("Nouvelle version du logiciel : v0.1.1", ""), "lien hors de GitHub ignoré");
    let q = quoi_de_neuf(&n, &[]);
    assert_eq!(q.len(), 3 + NOUVELLES.len() + 1);
    assert_eq!(q.iter().map(|n| n.date.as_str()).collect::<Vec<_>>(), ["2026-11-20", "2026-11-15", "2026-11-15", "2026-11-14", "2026-11-14", "2026-11-14", "2026-11-10"]);
    assert!(q[2].titre.contains("Télécopie") && q[3].titre.contains("56K") && q[4].titre.contains("08 36 15 42 86") && q[5].titre.starts_with("Ouverture du site"), "à date égale : les nouvelles, puis l'ouverture : {q:?}");
    let h = pages::accueil("Tinyverse Dialup", "c:3434", true, &q);
    assert!(h.contains("<b>20/11/2026</b>") && h.contains("Ouverture du site") && h.contains("NOUVEAU"));
}

#[test]
fn reglages_et_dates() {
    let env = |v: &'static [(&'static str, &'static str)]| move |k: &str| v.iter().find(|(n, _)| *n == k).map(|(_, x)| x.to_string());
    let r = Reglages::depuis_env(env(&[("PORTAIL_URL", "https://dialup.example.org"), ("PORTAIL_ADMIN_COURRIEL", "moi@example.org")])).unwrap();
    assert_eq!(r.editeur, EDITEUR_PAR_DEFAUT);
    assert_eq!(r.contact, "moi@example.org", "contact : l'administrateur par défaut");
    assert!(r.hebergeur.is_empty());
    assert_eq!(r.etat, PathBuf::from("/var/lib/v34-server/etat.json"));
    assert_eq!(r.livre, PathBuf::from("/var/lib/v34-server/livre-dor.toml"));
    let r = Reglages::depuis_env(env(&[
        ("PORTAIL_URL", "https://dialup.example.org"),
        ("PORTAIL_EDITEUR", "Raphaël"),
        ("PORTAIL_CONTACT", "contact@example.org"),
        ("PORTAIL_HEBERGEUR", "\"OVH SAS\\n2 rue Kellermann \\n59100 Roubaix\""),
        ("V34_ETAT", "/tmp/etat.json"),
        ("V34_LIVRE", "/tmp/livre.toml"),
    ]))
    .unwrap();
    assert_eq!((r.editeur.as_str(), r.contact.as_str(), r.hebergeur.as_str()), ("Raphaël", "contact@example.org", "OVH SAS\n2 rue Kellermann\n59100 Roubaix"));
    assert_eq!(r.etat, PathBuf::from("/tmp/etat.json"));
    assert!(Reglages::depuis_env(env(&[("PORTAIL_URL", "https://dialup.example.org"), ("PORTAIL_CONTACT", "pas une adresse")])).is_err());

    // Heure de Paris, été comme hiver.
    assert_eq!(pages::date_heure_fr(1_782_907_200), "01/07/2026 à 14 h 00"); // 2026-07-01 12:00 UTC
    assert_eq!(pages::date_heure_fr(1_796_126_400), "01/12/2026 à 13 h 00"); // 2026-12-01 12:00 UTC
    assert_eq!(pages::date_heure_fr(1_774_744_200), "29/03/2026 à 1 h 30"); // 00:30 UTC, juste avant le changement d'heure
    assert_eq!(pages::date_heure_fr(1_774_747_800), "29/03/2026 à 3 h 30"); // 01:30 UTC, juste après
    assert_eq!(pages::date_fr(1_798_761_599), "01/01/2027", "31/12 à 23 h 59 UTC : déjà le 1er janvier à Paris");
    assert_eq!(pages::duree_fr(93_784), "1 j 02 h 03 min");
    assert_eq!(pages::duree_fr(30), "moins d'une minute");
}

#[test]
fn facture_succes_et_classement_dans_l_espace_abonne() {
    use crate::facturation::{Facturation, Ticket};
    let b = banc();
    let p = &b.p;
    let maintenant = crate::comptes::maintenant();
    // Le central a noté deux appels de raziel et un de prunelle ce mois-ci
    let mut f = Facturation::ouvrir(b.dir.clone(), maintenant);
    let debut = maintenant - 7200;
    if v34_net::tarif::mois(debut) != v34_net::tarif::mois(maintenant) {
        return; // tout début de mois : les appels seraient sur le relevé précédent
    }
    f.appel(Ticket::nouveau(debut, 600, ("raziel", "0142424242"), "0836672253", "oracle", Some("oracle")));
    f.appel(Ticket::nouveau(debut + 1000, 120, ("raziel", "0142424242"), "0143434343", "prunelle", None));
    f.appel(Ticket::nouveau(debut + 2000, 60, ("prunelle", "0143434343"), "0142424242", "raziel", None));
    let session = connexion(p, "raziel", "mdp-raziel", "192.0.2.1");
    let page = texte(&get_avec(p, "/compte", &session));
    // 3615 ORACLE 10 min (22,30 F) + appel local (0,74 F) = 23,04 F
    for attendu in ["Ma facture de", "2 appels", "23,04 F", "Mes succès", "Consultation", "Allô ?", "Les plus grosses factures", "/compte/facture?mois="] {
        assert!(page.contains(attendu), "{attendu} absent de la page");
    }
    assert!(page.contains("???"), "les succès cachés restent cachés");
    // Le relevé détaillé : ses appels seulement
    let mois = v34_net::tarif::mois(maintenant);
    let releve = texte(&get_avec(p, &format!("/compte/facture?mois={mois}"), &session));
    for attendu in ["Relevé de communications", "3615 ORACLE", "prunelle", "Kiosque 08 36", "22,30 F", "23,04 F", "jamais"] {
        assert!(releve.contains(attendu), "{attendu} absent du relevé");
    }
    assert_eq!(releve.matches("<tr><td>").count(), 2, "les appels de prunelle n'y sont pas");
    // Sans session : le formulaire de connexion ; mois absurde : le mois en cours
    assert!(texte(&get(p, "/compte/facture")).contains("action=\"/compte/connexion\""));
    assert!(texte(&get_avec(p, "/compte/facture?mois=999999", &session)).contains("23,04 F"));
}

// ───────────────────────── administration ─────────────────────────

/// Session d'administration : (cookie, jeton CSRF).
fn admin(p: &Portail) -> (String, String) {
    let c = cookie(&post(p, "/admin/connexion", &[("mdp", "mdp-raziel")], None));
    let page = texte(&get_avec(p, "/admin", &c));
    let j = csrf(&page);
    (c, j)
}

/// Action d'administration (jeton CSRF ajouté).
fn action(p: &Portail, a: &(String, String), chemin: &str, champs: &[(&str, &str)]) -> Reponse {
    let mut v = champs.to_vec();
    v.push(("csrf", &a.1));
    post(p, chemin, &v, Some(&a.0))
}

/// Écrit l'état du central (frais).
fn etat_central(b: &Banc, en_ligne: &str, communications: &str) {
    let now = maintenant();
    std::fs::write(
        b.dir.join("etat.json"),
        format!(r#"{{"version":"0.1.7","depuis":{},"maj":{now},"abonnes":4,"connectes":2,"appels":1,"modems":{{"occupes":1,"max":4}},"salon":0,"en_ligne":{en_ligne},"communications":{communications}}}"#, now - 3600),
    )
    .unwrap();
}

#[test]
fn admin_tableau_de_bord_et_pages() {
    use crate::facturation::{Facturation, Ticket};
    let b = banc();
    let p = &b.p;
    compte(&b, "gaston", "0612345678", "gaston@example.org", "motdepasse");
    compte(&b, "lagaffe", "0699999999", "lagaffe@example.org", "motdepasse");
    b.comptes.regler_fai("lagaffe", true).unwrap();
    b.comptes.inscrire("jeanne", "0611111111", "h", "jeanne@example.org").unwrap();
    let now = maintenant();
    etat_central(&b, r#"["gaston","raziel"]"#, &format!(r#"[{{"de":"gaston","numero":"0836154286","vers":"fai","decroche":{}}}]"#, now - 150));
    let mut f = Facturation::ouvrir(b.dir.clone(), now);
    let meme_mois = v34_net::tarif::mois(now - 700) == v34_net::tarif::mois(now);
    f.appel(Ticket::nouveau(now - 700, 300, ("lagaffe", "0699999999"), "0612345678", "gaston", None));

    let a = admin(p);
    let page = texte(&get_avec(p, "/admin", &a.0));
    for attendu in [
        "PUPITRE DU CENTRAL",
        "EN SERVICE",
        "version 0.1.7",
        "<a href=\"/admin/abonne?nom=gaston\">gaston</a>, <a href=\"/admin/abonne?nom=raziel\">raziel</a>",
        "→ TINYVERSE (accès Internet) (08 36 15 42 86), depuis 2 min",
        "1 à décider",
        "Inscriptions en attente",
        "Derniers inscrits",
        "Accorder",
        "Refuser",
        "href=\"/admin/abonnes\"",
        "href=\"/admin/inscriptions\"",
        "href=\"/admin/factures\"",
        "href=\"/admin/tinyblog\"",
        "href=\"/admin/caches\"",
        "href=\"/admin/livre\"",
        "href=\"/admin/site\"",
        "Se déconnecter",
    ] {
        assert!(page.contains(attendu), "{attendu} absent du tableau de bord : {page}");
    }
    if meme_mois {
        assert!(page.contains("1 appel (5 min)"), "appels du mois : {page}");
    }
    // Toutes les pages : le menu, et rien en cache
    for chemin in [
        "/admin/abonnes",
        "/admin/abonne?nom=gaston",
        "/admin/abonne?nom=raziel",
        "/admin/inscriptions",
        "/admin/factures",
        "/admin/facture?nom=lagaffe",
        "/admin/tinyblog",
        "/admin/caches",
        "/admin/livre",
        "/admin/site",
    ] {
        let r = get_avec(p, chemin, &a.0);
        assert_eq!(r.statut, 200, "{chemin} : {}", texte(&r));
        assert!(texte(&r).contains("class=\"adm-menu\""), "{chemin} : menu absent");
        assert_eq!(entete(&r, "Cache-Control"), Some("no-store"), "{chemin}");
    }
    // Liste : config.toml et site, recherche par numéro (espaces permis)
    let l = texte(&get_avec(p, "/admin/abonnes", &a.0));
    assert!(l.contains("Tous les abonnés (4)") && l.contains("prunelle") && !l.contains("jeanne"), "{l}");
    let l = texte(&get_avec(p, "/admin/abonnes?q=06+99", &a.0));
    assert!(l.contains("abonne?nom=lagaffe") && !l.contains("abonne?nom=gaston"), "{l}");
    let l = texte(&get_avec(p, "/admin/abonnes?q=GASTON%40example", &a.0));
    assert!(l.contains("abonne?nom=gaston") && !l.contains("abonne?nom=lagaffe"), "par adresse : {l}");
    // Introuvables
    assert_eq!(get_avec(p, "/admin/abonne?nom=personne", &a.0).statut, 404);
    assert_eq!(get_avec(p, "/admin/inexistante", &a.0).statut, 404);
    // Sans session : le formulaire de connexion partout
    for chemin in ["/admin/abonnes", "/admin/site", "/admin/tinyblog/img?f=gaston-1.gif"] {
        assert!(texte(&get(p, chemin)).contains("action=\"/admin/connexion\""), "{chemin}");
    }
    // Actions : session et jeton CSRF exigés
    assert_eq!(post(p, "/admin/numero", &[("nom", "gaston"), ("numero", "0655555555"), ("csrf", &a.1)], None).statut, 303);
    assert_eq!(post(p, "/admin/numero", &[("nom", "gaston"), ("numero", "0655555555"), ("csrf", "faux")], Some(&a.0)).statut, 403);
    assert_eq!(action(p, &a, "/admin/inconnue", &[]).statut, 404);
    assert_eq!(b.comptes.compte("gaston").unwrap().numero, "0612345678");
    // Accès Internet depuis le tableau de bord : on y revient
    let r = action(p, &a, "/admin/fai", &[("nom", "gaston"), ("d", "non"), ("retour", "tableau")]);
    assert!(r.statut == 200 && texte(&r).contains("Accès Internet de gaston : refusé.") && texte(&r).contains("PUPITRE"), "{}", texte(&r));
    // Central muet : le pupitre le dit
    std::fs::remove_file(b.dir.join("etat.json")).unwrap();
    assert!(texte(&get_avec(p, "/admin", &a.0)).contains("HORS SERVICE ou muet"));
}

#[test]
fn admin_fiche_numero_suspension_mot_de_passe_suppression() {
    let b = banc();
    let p = &b.p;
    compte(&b, "gaston", "0612345678", "gaston@example.org", "motdepasse");
    b.comptes.inscrire("lagaffe", "0699999999", "h", "lagaffe@example.org").unwrap();
    let session = connexion(p, "gaston", "motdepasse", "192.0.2.3");
    let a = admin(p);
    let fiche = texte(&get_avec(p, "/admin/abonne?nom=gaston", &a.0));
    for attendu in ["Fiche de l'abonné gaston", "06 12 34 56 78", "mailto:gaston@example.org", "action=\"/admin/numero\"", "/admin/mdp/lien", "/admin/mdp/provisoire", "Suspendre", "Supprimer le compte", "ne publie pas son état", "Facture de", "Pas de Tinyblog"] {
        assert!(fiche.contains(attendu), "{attendu} absent de la fiche : {fiche}");
    }

    // Numéro : format, numéros spéciaux et cachés, configuration, inscriptions en attente, le sien
    for faux in ["12345", "0836154286", "0143331212", "0142424242", "0699999999", "06 12 34 56 78"] {
        let r = action(p, &a, "/admin/numero", &[("nom", "gaston"), ("numero", faux)]);
        assert_eq!(r.statut, 400, "{faux} accepté : {}", texte(&r));
        assert!(texte(&r).contains("class=\"erreur\""));
    }
    assert_eq!(b.comptes.compte("gaston").unwrap().numero, "0612345678");
    let r = action(p, &a, "/admin/numero", &[("nom", "gaston"), ("numero", "06.55.55.55.55")]);
    assert_eq!(r.statut, 200, "{}", texte(&r));
    assert!(texte(&r).contains("nouveau numéro 06 55 55 55 55"));
    assert_eq!(b.comptes.compte("gaston").unwrap().numero, "0655555555");
    let l = lettres(p);
    assert!(l.len() == 1 && l[0].a == "gaston@example.org" && l[0].sujet.contains("nouveau numéro") && l[0].corps.contains("06 55 55 55 55"), "{l:?}");

    // Suspension : « sûr » exigé ; l'abonné le voit dans son espace
    let r = action(p, &a, "/admin/suspendre", &[("nom", "gaston"), ("d", "oui")]);
    assert_eq!(r.statut, 400);
    assert!(!b.comptes.compte("gaston").unwrap().suspendu);
    let r = action(p, &a, "/admin/suspendre", &[("nom", "gaston"), ("d", "oui"), ("sur", "1")]);
    assert!(r.statut == 200 && texte(&r).contains("suspendu"), "{}", texte(&r));
    assert!(b.comptes.compte("gaston").unwrap().suspendu);
    assert!(crate::comptes::lire_comptes(&b.dir.join("comptes.toml")).unwrap()[0].suspendu, "le central le lit");
    assert!(texte(&get_avec(p, "/compte", &session)).contains("Abonnement suspendu"));
    assert!(texte(&get_avec(p, "/admin/abonnes", &a.0)).contains("<b>suspendu</b>"));
    let r = action(p, &a, "/admin/suspendre", &[("nom", "gaston"), ("d", "non")]);
    assert!(r.statut == 200 && texte(&r).contains("réactivé"));
    assert!(!b.comptes.compte("gaston").unwrap().suspendu);
    assert!(!texte(&get_avec(p, "/compte", &session)).contains("Abonnement suspendu"));

    // Lien de réinitialisation, comme « mot de passe oublié »
    let r = action(p, &a, "/admin/mdp/lien", &[("nom", "gaston")]);
    assert!(r.statut == 200 && texte(&r).contains("Lien de réinitialisation envoyé à gaston@example.org"), "{}", texte(&r));
    let l = lettres(p);
    assert!(l[1].a == "gaston@example.org" && l[1].corps.contains("L'opérateur du central vous invite à changer le mot de passe"), "{:?}", l[1]);
    let j = lien(&l[1].corps, "https://dialup.example.org/reinitialiser?t=").split("t=").nth(1).unwrap().to_string();
    assert_eq!(get(p, &format!("/reinitialiser?t={j}")).statut, 200);

    // Mot de passe provisoire : « sûr » exigé, affiché une fois, sessions fermées, titulaire prévenu
    assert_eq!(action(p, &a, "/admin/mdp/provisoire", &[("nom", "gaston")]).statut, 400);
    assert!(mdp_est(&b, "gaston", "motdepasse"));
    let r = action(p, &a, "/admin/mdp/provisoire", &[("nom", "gaston"), ("sur", "1")]);
    assert_eq!(r.statut, 200);
    let t = texte(&r);
    let mdp: String = t.split("Mot de passe provisoire de gaston : ").nth(1).expect("mot de passe affiché").chars().take(14).collect();
    assert!(mdp.len() == 14 && mdp.matches('-').count() == 2, "{mdp}");
    assert!(mdp_est(&b, "gaston", &mdp));
    assert_eq!(get(p, &format!("/reinitialiser?t={j}")).statut, 404, "le lien en cours ne marche plus");
    assert!(texte(&get_avec(p, "/compte", &session)).contains("action=\"/compte/connexion\""), "session fermée");
    let l = lettres(p);
    assert!(l[2].sujet.contains("mot de passe provisoire") && !l[2].corps.contains(&mdp), "le mot de passe ne part pas par e-mail : {:?}", l[2]);
    assert_eq!(entete(&r, "Cache-Control"), Some("no-store"));

    // Abonné de la configuration : lecture seule
    let fiche = texte(&get_avec(p, "/admin/abonne?nom=raziel", &a.0));
    assert!(fiche.contains("config.toml") && !fiche.contains("action=\"/admin/numero\"") && !fiche.contains("Supprimer le compte"), "{fiche}");
    assert_eq!(action(p, &a, "/admin/numero", &[("nom", "raziel"), ("numero", "0655555556")]).statut, 404);

    // Suppression : « sûr » exigé ; avec « tout », son Tinyblog et ses messages partent aussi
    crate::tinyblog::store::Store::new(b.dir.join("tinyblog")).modifier("gaston", |bl| bl.titre = "Chez Gaston".into());
    std::fs::write(b.dir.join("repondeur.json"), r#"[{"de":"gaston","quand":1,"texte":"Allô ?"},{"de":"lagaffe","quand":2,"texte":"M'enfin"}]"#).unwrap();
    assert!(texte(&get_avec(p, "/admin/abonne?nom=gaston", &a.0)).contains("Chez Gaston"));
    assert_eq!(action(p, &a, "/admin/supprimer", &[("nom", "gaston"), ("tout", "1")]).statut, 400);
    assert!(b.comptes.compte("gaston").is_some());
    let r = action(p, &a, "/admin/supprimer", &[("nom", "gaston"), ("tout", "1"), ("sur", "1")]);
    assert!(r.statut == 200 && texte(&r).contains("Compte gaston supprimé") && texte(&r).contains("Tinyblog est effacé") && texte(&r).contains("1 message"), "{}", texte(&r));
    assert!(b.comptes.compte("gaston").is_none());
    assert!(!b.dir.join("tinyblog/gaston.json").exists());
    let rep = std::fs::read_to_string(b.dir.join("repondeur.json")).unwrap();
    assert!(!rep.contains("gaston") && rep.contains("lagaffe"), "{rep}");
}

#[test]
fn admin_inscriptions_en_attente() {
    let b = banc();
    let p = &b.p;
    let t = jeton_formulaire(p);
    let r = post(p, "/inscription", &[("t", &t), ("nom", "lagaffe"), ("numero", "0699999999"), ("courriel", "lagaffe@example.org"), ("mdp", "motdepasse"), ("mdp2", "motdepasse"), ("cgu", "1")], None);
    assert_eq!(r.statut, 200);
    let j1 = lien(&lettres(p)[0].corps, "https://dialup.example.org/valider?t=").split("t=").nth(1).unwrap().to_string();
    let a = admin(p);
    let page = texte(&get_avec(p, "/admin/inscriptions", &a.0));
    assert!(page.contains("lagaffe") && page.contains("lagaffe@example.org") && page.contains("Renvoyer le lien") && page.contains("Expire le"), "{page}");
    assert!(texte(&get_avec(p, "/admin", &a.0)).contains("Inscriptions (1)"), "compteur du menu");

    // Renvoyer : un nouveau lien, l'ancien ne marche plus
    let r = action(p, &a, "/admin/inscription/renvoyer", &[("nom", "lagaffe")]);
    assert!(r.statut == 200 && texte(&r).contains("Nouveau lien d&#39;activation envoyé à lagaffe@example.org"), "{}", texte(&r));
    let l = lettres(p);
    assert_eq!(l.len(), 2);
    let j2 = lien(&l[1].corps, "https://dialup.example.org/valider?t=").split("t=").nth(1).unwrap().to_string();
    assert_ne!(j1, j2);
    assert_eq!(get(p, &format!("/valider?t={j1}")).statut, 404);
    assert_eq!(get(p, &format!("/valider?t={j2}")).statut, 200);
    assert_eq!(action(p, &a, "/admin/inscription/renvoyer", &[("nom", "personne")]).statut, 400);

    // Supprimer : « sûr » exigé, puis nom, numéro et adresse libérés
    assert_eq!(action(p, &a, "/admin/inscription/supprimer", &[("nom", "lagaffe")]).statut, 400);
    assert_eq!(b.comptes.en_attente().len(), 1);
    let r = action(p, &a, "/admin/inscription/supprimer", &[("nom", "lagaffe"), ("sur", "1")]);
    assert!(r.statut == 200 && texte(&r).contains("Inscription de lagaffe supprimée"));
    assert!(b.comptes.en_attente().is_empty());
    assert_eq!(get(p, &format!("/valider?t={j2}")).statut, 404);
    assert_eq!(b.comptes.conflit("lagaffe", "0699999999", "lagaffe@example.org"), None);
}

#[test]
fn admin_factures_de_tous_les_abonnes() {
    use crate::facturation::{Facturation, Ticket};
    let b = banc();
    let p = &b.p;
    let now = maintenant();
    let debut = now - 7200;
    if v34_net::tarif::mois(debut) != v34_net::tarif::mois(now) {
        return; // tout début de mois : les appels seraient sur le relevé précédent
    }
    let mut f = Facturation::ouvrir(b.dir.clone(), now);
    f.appel(Ticket::nouveau(debut, 600, ("raziel", "0142424242"), "0836672253", "oracle", Some("oracle")));
    f.appel(Ticket::nouveau(debut + 1000, 120, ("raziel", "0142424242"), "0143434343", "prunelle", None));
    f.appel(Ticket::nouveau(debut + 2000, 60, ("prunelle", "0143434343"), "0142424242", "raziel", None));
    let a = admin(p);
    let mois = v34_net::tarif::mois(now);
    let page = texte(&get_avec(p, "/admin/factures", &a.0));
    let lien_releve = format!("/admin/facture?nom=raziel&amp;mois={mois}");
    for attendu in ["3 appels", "23,78 F", "23,04 F", "0,74 F", "Journal des communications", "3615 ORACLE", lien_releve.as_str()] {
        assert!(page.contains(attendu), "{attendu} absent des factures : {page}");
    }
    assert!(page.contains("abonne?nom=raziel"));
    let (i_raziel, i_prunelle) = (page.find(">raziel</a></td><td class=\"n\">2").unwrap(), page.find(">prunelle</a></td><td class=\"n\">1").unwrap());
    assert!(i_raziel < i_prunelle, "la plus grosse facture d'abord");
    // Le relevé d'un abonné, avec les liens de l'administration
    let r = texte(&get_avec(p, &format!("/admin/facture?nom=raziel&mois={mois}"), &a.0));
    assert!(r.contains("Relevé de communications") && r.contains("23,04 F") && r.contains("/admin/abonne?nom=raziel") && !r.contains("/compte/facture"), "{r}");
    // Fiche : la facture du mois et les succès
    let fiche = texte(&get_avec(p, "/admin/abonne?nom=raziel", &a.0));
    assert!(fiche.contains("23,04 F") && fiche.contains("Succès (") && fiche.contains("Consultation"), "{fiche}");
    // Un mois sans rien
    assert!(texte(&get_avec(p, "/admin/factures?mois=199812", &a.0)).contains("Aucune communication"));
}

#[test]
fn admin_moderation_des_tinyblogs() {
    use crate::tinyblog::store::{Article, Com, Store};
    let b = banc();
    let p = &b.p;
    let now = maintenant();
    let store = Store::new(b.dir.join("tinyblog"));
    let com = |de: &str, date: u64, texte: &str| Com { de: de.into(), date, texte: texte.into() };
    store.modifier("gaston", |bl| {
        bl.titre = "Le blog <b>de Gaston</b>".into();
        bl.description = "Gaffes en tout genre".into();
        bl.cree = now - 1000;
        bl.articles.push(Article { id: 1, date: now - 500, titre: "Premier".into(), texte: "Bonjour <script>alert(1)</script>".into(), image: Some("gaston-1.gif".into()), kiffs: vec![], coms: vec![com("lagaffe", now - 50, "M'enfin <i>"), com("prunelle", now - 40, "Rogntudju")] });
        bl.articles.push(Article { id: 2, date: now - 100, titre: "Deuxième".into(), texte: "Encore".into(), image: None, kiffs: vec!["lagaffe".into()], coms: vec![] });
        bl.suivant = 2;
    });
    store.modifier("gaston-x", |bl| bl.articles.push(Article { id: 1, date: now - 10, titre: "Voisin".into(), image: Some("gaston-x-1.gif".into()), ..Default::default() }));
    for f in ["gaston-1.gif", "gaston-7.gif", "gaston-x-1.gif"] {
        assert!(store.enregistrer_image(f, b"GIF89a"));
    }
    let a = admin(p);
    let page = texte(&get_avec(p, "/admin/tinyblog", &a.0));
    for attendu in ["Les Tinyblogs (2)", "/admin/tinyblog?nom=gaston", "Premier", "Derniers articles", "Derniers commentaires", "M&#39;enfin &lt;i&gt;", "/admin/tinyblog/img?f=gaston-1.gif"] {
        assert!(page.contains(attendu), "{attendu} absent : {page}");
    }
    assert!(!page.contains("<script>") && !page.contains("<b>de Gaston"), "tout est échappé");
    let blog = texte(&get_avec(p, "/admin/tinyblog?nom=gaston", &a.0));
    assert!(blog.contains("Le blog &lt;b&gt;de Gaston&lt;/b&gt;") && blog.contains("Supprimer l'article") && blog.contains("Rogntudju"), "{blog}");
    assert_eq!(get_avec(p, "/admin/tinyblog?nom=personne", &a.0).statut, 404);
    // Les images, derrière la session seulement
    let r = get_avec(p, "/admin/tinyblog/img?f=gaston-1.gif", &a.0);
    assert!(r.statut == 200 && r.type_contenu == "image/gif" && r.corps == b"GIF89a");
    assert!(entete(&r, "Cache-Control").unwrap().starts_with("private"));
    assert_eq!(get_avec(p, "/admin/tinyblog/img?f=../comptes.toml", &a.0).statut, 404);

    // Commentaire : l'empreinte doit correspondre (la liste a pu bouger)
    let e = admin::empreinte_com(&com("prunelle", now - 40, "Rogntudju"));
    assert_eq!(action(p, &a, "/admin/tinyblog/com/supprimer", &[("nom", "gaston"), ("id", "1"), ("k", "0"), ("e", &e)]).statut, 404);
    let r = action(p, &a, "/admin/tinyblog/com/supprimer", &[("nom", "gaston"), ("id", "1"), ("k", "1"), ("e", &e), ("retour", "blog")]);
    assert!(r.statut == 200 && texte(&r).contains("Commentaire de prunelle supprimé") && texte(&r).contains("Tinyblog de gaston"), "{}", texte(&r));
    let coms = &store.lire("gaston").unwrap().articles[0].coms;
    assert!(coms.len() == 1 && coms[0].de == "lagaffe");

    // Article : « sûr » exigé, l'image part avec
    assert_eq!(action(p, &a, "/admin/tinyblog/article/supprimer", &[("nom", "gaston"), ("id", "1")]).statut, 400);
    let r = action(p, &a, "/admin/tinyblog/article/supprimer", &[("nom", "gaston"), ("id", "1"), ("sur", "1")]);
    assert!(r.statut == 200 && texte(&r).contains("Article « Premier »"), "{}", texte(&r));
    assert_eq!(store.lire("gaston").unwrap().articles.len(), 1);
    assert!(!b.dir.join("tinyblog/img/gaston-1.gif").exists());
    assert_eq!(action(p, &a, "/admin/tinyblog/article/supprimer", &[("nom", "gaston"), ("id", "1"), ("sur", "1")]).statut, 404);

    // Titre et présentation effacés ; la modération ne crée pas de blog
    assert_eq!(action(p, &a, "/admin/tinyblog/vider", &[("nom", "gaston")]).statut, 200);
    let g = store.lire("gaston").unwrap();
    assert!(g.titre.is_empty() && g.description.is_empty() && g.articles.len() == 1);
    assert_eq!(action(p, &a, "/admin/tinyblog/vider", &[("nom", "personne")]).statut, 404);
    assert!(!b.dir.join("tinyblog/personne.json").exists());

    // Tout le blog : ses images (même orphelines), pas celles du voisin « gaston-x »
    assert_eq!(action(p, &a, "/admin/tinyblog/supprimer", &[("nom", "gaston")]).statut, 400);
    let r = action(p, &a, "/admin/tinyblog/supprimer", &[("nom", "gaston"), ("sur", "1")]);
    assert!(r.statut == 200 && texte(&r).contains("Tinyblog de gaston supprimé"), "{}", texte(&r));
    assert!(store.lire("gaston").is_none() && !b.dir.join("tinyblog/img/gaston-7.gif").exists());
    assert!(store.lire("gaston-x").is_some() && b.dir.join("tinyblog/img/gaston-x-1.gif").exists());
}

#[test]
fn admin_moderation_des_numeros_caches() {
    let b = banc();
    let p = &b.p;
    std::fs::write(b.dir.join("repondeur.json"), r#"[{"de":"gaston","quand":1794315600,"texte":"Allô <b>Gérard</b> ?"},{"de":"lagaffe","quand":1794315700,"texte":"M'enfin"}]"#).unwrap();
    std::fs::write(b.dir.join("prix.json"), r#"{"gaston":1200,"lagaffe":800}"#).unwrap();
    let a = admin(p);
    let page = texte(&get_avec(p, "/admin/caches", &a.0));
    for attendu in ["répondeur de Gérard (01 43 33 12 12)", "Allô &lt;b&gt;Gérard&lt;/b&gt; ?", "M&#39;enfin", "1200", "/admin/caches/supprimer"] {
        assert!(page.contains(attendu), "{attendu} absent : {page}");
    }
    let depots = admin::depots::lister(&b.dir);
    let e = depots[0].entrees.iter().find(|e| e.de.as_deref() == Some("gaston")).unwrap().clone();
    // Empreinte périmée : rien n'est effacé
    let r = action(p, &a, "/admin/caches/supprimer", &[("fichier", "repondeur.json"), ("cle", &e.cle), ("e", "0123456789abcdef")]);
    assert_eq!(r.statut, 400);
    assert_eq!(action(p, &a, "/admin/caches/supprimer", &[("fichier", "comptes.toml"), ("cle", "0"), ("e", &e.empreinte)]).statut, 400);
    let r = action(p, &a, "/admin/caches/supprimer", &[("fichier", "repondeur.json"), ("cle", &e.cle), ("e", &e.empreinte)]);
    assert!(r.statut == 200 && texte(&r).contains("Entrée de gaston retirée"), "{}", texte(&r));
    assert!(!std::fs::read_to_string(b.dir.join("repondeur.json")).unwrap().contains("Gérard"));
    // Tout ce qu'un abonné a laissé (depuis sa fiche)
    assert!(texte(&get_avec(p, "/admin/abonne?nom=raziel", &a.0)).contains("0 message"));
    assert_eq!(action(p, &a, "/admin/caches/supprimer-de", &[("nom", "lagaffe")]).statut, 400);
    let r = action(p, &a, "/admin/caches/supprimer-de", &[("nom", "lagaffe"), ("sur", "1")]);
    assert!(r.statut == 200 && texte(&r).contains("2 message(s) ou score(s) de lagaffe"), "{}", texte(&r));
    assert_eq!(std::fs::read_to_string(b.dir.join("prix.json")).unwrap(), r#"{"gaston":1200}"#);
    assert_eq!(std::fs::read_to_string(b.dir.join("repondeur.json")).unwrap(), "[]");
}

#[test]
fn admin_le_site_nouvelles_et_inscriptions() {
    let b = banc();
    let p = &b.p;
    let bouton = "href=\"/inscription\">» Je m'inscris";
    assert!(texte(&get(p, "/")).contains(bouton));
    let a = admin(p);
    let page = texte(&get_avec(p, "/admin/site", &a.0));
    assert!(page.contains("Ouvertes de votre côté") && page.contains("Ouverture du site") && page.contains("écrite dans le code"), "{page}");

    // Nouvelles : ajoutée, affichée (échappée) sur l'accueil, puis retirée
    assert_eq!(action(p, &a, "/admin/site/nouvelle", &[("date", "16/11/2026"), ("titre", "x")]).statut, 400);
    assert_eq!(action(p, &a, "/admin/site/nouvelle", &[("date", "2026-11-16"), ("titre", "  ")]).statut, 400);
    let r = action(p, &a, "/admin/site/nouvelle", &[("date", "2099-01-01"), ("titre", "Le <b>Morse</b> arrive")]);
    assert!(r.statut == 200 && texte(&r).contains("ajoutée"), "{}", texte(&r));
    let accueil = texte(&get(p, "/"));
    assert!(accueil.contains("<b>01/01/2099</b> — Le &lt;b&gt;Morse&lt;/b&gt; arrive <span class=\"nouveau\">NOUVEAU !</span>"), "{accueil}");
    assert!(b.dir.join("portail.toml").exists());
    let id = p.site.lire().nouvelle[0].id.clone();
    assert!(texte(&get_avec(p, "/admin/site", &a.0)).contains(&format!("name=\"id\" value=\"{id}\"")));
    let r = action(p, &a, "/admin/site/nouvelle/supprimer", &[("id", &id)]);
    assert!(r.statut == 200 && texte(&r).contains("retirée"));
    assert!(!texte(&get(p, "/")).contains("Morse"));

    // Inscriptions fermées : plus de bouton, formulaire refusé, les inscriptions en attente se confirment encore
    let t = jeton_formulaire(p);
    let champs = |nom: &'static str, num: &'static str| [("t", t.clone()), ("nom", nom.into()), ("numero", num.into()), ("courriel", format!("{nom}@example.org")), ("mdp", "motdepasse".into()), ("mdp2", "motdepasse".into()), ("cgu", "1".into())];
    let c = champs("lagaffe", "0699999999");
    let c: Vec<(&str, &str)> = c.iter().map(|(k, v)| (*k, v.as_str())).collect();
    assert_eq!(post(p, "/inscription", &c, None).statut, 200);
    let j = lien(&lettres(p)[0].corps, "https://dialup.example.org/valider?t=").split("t=").nth(1).unwrap().to_string();
    let r = action(p, &a, "/admin/site/inscriptions", &[("d", "fermer")]);
    assert!(r.statut == 200 && texte(&r).contains("Inscriptions fermées."));
    assert!(!texte(&get(p, "/")).contains(bouton));
    assert_eq!(get(p, "/inscription").statut, 503);
    let c = champs("jeanne", "0611111111");
    let c: Vec<(&str, &str)> = c.iter().map(|(k, v)| (*k, v.as_str())).collect();
    assert_eq!(post(p, "/inscription", &c, None).statut, 503);
    assert!(texte(&get_avec(p, "/admin", &a.0)).contains("<b>fermées</b> (par vous)"));
    assert_eq!(post(p, "/valider", &[("t", &j)], None).statut, 200, "l'inscription en attente se confirme");
    // Relu au redémarrage du site
    assert!(admin::site::Site::ouvrir(&b.dir.join("portail.toml")).lire().inscriptions_fermees);
    let r = action(p, &a, "/admin/site/inscriptions", &[("d", "ouvrir")]);
    assert!(r.statut == 200 && texte(&r).contains("rouvertes"));
    assert!(texte(&get(p, "/")).contains(bouton));
    assert_eq!(get(p, "/inscription").statut, 200);
}
