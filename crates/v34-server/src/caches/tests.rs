use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, sync_channel};

use v34_net::bbs::{self, Decoder, Frame, Item};

use super::*;

/// Joue un service avec ces lignes tapées (puis l'appelant raccroche) ;
/// rend tout ce qui s'est affiché et la fin.
fn jouer(service: &str, entrees: &[&str], donnees: Option<&Path>) -> (String, Result<(), Fin>) {
    jouer_avec(service, entrees, donnees, &Default::default())
}

fn jouer_avec(service: &str, entrees: &[&str], donnees: Option<&Path>, exploits: &Exploits) -> (String, Result<(), Fin>) {
    let n = LISTE.iter().find(|n| n.service == service).expect("service");
    let (to_modem, sortie) = sync_channel(10_000);
    let (entree, from_modem) = channel();
    for e in entrees {
        entree.send(bbs::encode(&Frame::Chat(e.to_string()))).unwrap();
    }
    drop(entree);
    let mut l = Ligne::new(to_modem, &from_modem, "testeur", donnees, 10_000_000);
    l.exploits = Some(exploits);
    let r = (n.lancer)(&mut l);
    drop(l);
    let mut dec = Decoder::default();
    let mut texte = String::new();
    while let Ok(b) = sortie.try_recv() {
        for it in dec.push(&b) {
            if let Item::Frame(Frame::Chat(t)) = it {
                texte.push_str(&t);
            }
        }
    }
    (texte, r)
}

fn dossier(nom: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("v34-caches-{nom}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn numeros_et_succes() {
    for (i, n) in LISTE.iter().enumerate() {
        assert_eq!(index(n.numero), Some(i as u8));
        assert!(v34_net::succes::trouver(n.service).is_some_and(|s| s.cache), "succès caché pour {}", n.service);
        assert!(n.numero.len() == 10 && n.numero.chars().all(|c| c.is_ascii_digit()));
    }
    // les numéros cachés répondent, mais ne sont pas annoncés aux clients
    let cfg = crate::config::Config::from_toml("").unwrap();
    assert_eq!(cfg.service_for("0836993699"), Some(crate::config::Service::Cache(0)));
    assert!(cfg.services().iter().all(|(num, _)| index(num).is_none()));
    assert_eq!(crate::config::Service::Cache(0).name(), "cache-horloge");
    // tarifs : 08 36 au kiosque, 0 800 gratuit, le reste local
    use v34_net::tarif::Categorie;
    assert_eq!(Categorie::de_l_appel("0836993699", Some("cache-horloge")), Categorie::Kiosque);
    assert_eq!(Categorie::de_l_appel("0800200000", Some("cache-an2000")), Categorie::Vert);
    assert_eq!(Categorie::de_l_appel("0145781998", Some("cache-ministere")), Categorie::Local);
}

#[test]
fn horloge() {
    let (t, r) = jouer("cache-horloge", &[], None);
    assert_eq!(r, Err(Fin::Raccroche));
    assert!(t.contains("HORLOGE PARLANTE") && t.contains("1998") && t.contains("Au quatrième top, il sera exactement"), "{t}");
}

#[test]
fn meteo() {
    let d = dossier("meteo");
    std::fs::write(d.join("etat.json"), r#"{"connectes":2,"appels":1,"salon":0}"#).unwrap();
    let (t, _) = jouer("cache-meteo", &["/bulletin"], Some(&d));
    assert!(t.contains("2 abonnés sur le réseau et 1 communication en cours"), "{t}");
    assert!(t.contains("mer d'huile") && t.contains("Ligne Orage : avis de grand frais"));
    assert_eq!(t.matches("MÉTÉO DES LIGNES").count(), 2, "bulletin réécouté");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn an2000_souffle_le_mot_de_passe() {
    let (t, r) = jouer("cache-an2000", &["3", "/fin"], None);
    assert_eq!(r, Err(Fin::Demande));
    assert!(t.contains("jours avant l'an 2000") && t.contains("« bogue »"), "{t}");
}

#[test]
fn ministere() {
    let (t, r) = jouer("cache-ministere", &["raziel", "bug", "Bogue.", "LIRE BUREAU/REGISTRE.TXT", "/fin"], None);
    assert_eq!(r, Err(Fin::Demande));
    assert!(t.contains("MOT DE PASSE INCORRECT (tentative 1 sur 3)") && t.contains("BIENVENUE, AGENT RAZIEL"), "{t}");
    // le registre donne tous les numéros, sauf celui du dossier rouge
    for n in LISTE.iter().filter(|n| !matches!(n.service, "cache-ministere" | "cache-futur")) {
        let lisible: String = n.numero.chars().collect::<Vec<_>>().chunks(2).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join(" ");
        let lisible = if n.numero.starts_with("0800") { format!("0 800 {}", &lisible[6..]) } else { lisible };
        assert!(t.contains(&lisible), "{lisible} absent du registre");
    }
    let (t, r) = jouer("cache-ministere", &["x", "a", "b", "c"], None);
    assert_eq!(r, Ok(()));
    assert!(t.contains("ACCÈS REFUSÉ") && t.contains("terme officiel"));
}

/// Le piratage complet : agent, fouille, chef (mot de passe du répondeur),
/// directeur (prix du Tamagotchi), dossier rouge, et la ligne de 2026.
#[test]
fn piratage_du_ministere() {
    let exploits: Exploits = Default::default();
    let (t, r) = jouer_avec(
        "cache-ministere",
        &[
            "raziel", "bogue", "LISTE", "VA DIRECTION", "VA BUREAU", "LIRE POSTIT", "VA ..", "LIRE ARCHIVES/FACTURES.TXT",
            "CONNEXION chef", "minouche", "LIRE ARCHIVES/FACTURES.TXT", "CONNEXION directeur", "1290", "CONNEXION directeur", "150 F",
            "VA DIRECTION", "LISTE", "LIRE ROUGE", "SUDO", "QUI", "LOGOUT",
        ],
        None,
        &exploits,
    );
    assert_eq!(r, Ok(()));
    for attendu in [
        "DIRECTION/   [niveau 3]",
        "ACCÈS REFUSÉ : DIRECTION demande une habilitation de niveau 3",
        "Il a encore sur son répondeur",
        "ACCÈS REFUSÉ : habilitation de niveau 2 requise",
        "BIENVENUE, CHEF DE SERVICE CHEF",
        "un Tamagotchi",
        "MOT DE PASSE INCORRECT.",
        "BIENVENUE, DIRECTEUR DIRECTEUR",
        "SLP:/DIRECTION>",
        "08 36 20 26 26",
        "Cerfa n° 98-SU",
        "Déconnexion",
    ] {
        assert!(t.contains(attendu), "« {attendu} » absent :\n{t}");
    }
    let e = exploits.lock().unwrap().clone();
    assert_eq!(e, [("testeur".to_string(), "chef"), ("testeur".to_string(), "chef"), ("testeur".to_string(), "secret-etat")]);
    // les indices : le chef sur le répondeur, le Tamagotchi au catalogue
    let (t, _) = jouer("cache-repondeur", &["/ecouter", "/fin"], None);
    assert!(t.contains("Minouche"), "{t}");
    let zeros = ["0"; 15];
    let mut entrees: Vec<&str> = zeros.to_vec();
    entrees.push("/catalogue");
    let (t, _) = jouer("cache-prix", &entrees, None);
    assert!(t.contains("un Tamagotchi : 150 F"), "{t}");
    // la ligne de 2026 : succès, nouvelles, message pour 1998
    let ex2: Exploits = Default::default();
    let (t, r) = jouer_avec("cache-futur", &["Bonjour 1998 !"], None, &ex2);
    assert_eq!(r, Ok(()));
    assert!(t.contains("Ici 2026") && t.contains("Message parti pour 1998"));
    assert_eq!(ex2.lock().unwrap().as_slice(), [("testeur".to_string(), "cache-futur")]);
}

#[test]
fn repondeur_garde_les_messages() {
    let d = dossier("repondeur");
    let (t, r) = jouer("cache-repondeur", &["Salut Gérard, c'est le facteur"], Some(&d));
    assert_eq!(r, Ok(()));
    assert!(t.contains("vous êtes bien chez Gérard") && t.contains("Message enregistré"));
    let (t, _) = jouer("cache-repondeur", &["/ecouter", "/fin"], Some(&d));
    // (le vieux message du chef, plus celui-ci)
    assert!(t.contains("2 messages sur la bande") && t.contains("de testeur : Salut Gérard, c'est le facteur"), "{t}");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn prix_en_francs() {
    assert_eq!(prix::lire_prix("1 290 F"), Some(1290));
    assert_eq!(prix::lire_prix("1290,50"), Some(1290));
    assert_eq!(prix::lire_prix("beaucoup"), None);
    assert_eq!(prix::points(100, 100, true), 150);
    assert_eq!(prix::points(100, 100, false), 100);
    assert_eq!(prix::points(100, 150, false), 45);
    assert_eq!(prix::points(100, 1000, false), 0);
    let d = dossier("prix");
    let zeros = ["0"; 15];
    let (t, r) = jouer("cache-prix", &zeros, Some(&d));
    // (après la partie, le jeu attend /catalogue ; l'appelant raccroche)
    assert_eq!(r, Err(Fin::Raccroche));
    assert_eq!(t.matches("C'est plus !").count(), 10, "{t}");
    assert!(t.contains("Score final : 0 points") && t.contains("TABLEAU D'HONNEUR"));
}
