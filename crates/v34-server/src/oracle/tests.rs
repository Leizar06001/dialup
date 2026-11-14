//! Tests de l'oracle, avec un faux OpenRouter local (jamais le vrai).

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use v34_net::bbs::{self, Decoder, Frame, Item};

use super::*;
use crate::log::{set_level, Level};

const CLE: &str = "sk-or-v1-faussecledetest0123456789";

/// Faux point d'accès : note chaque requête (en-tête Authorization, corps)
/// et répond ce que dit `repondre` (statut, corps, pause avant de répondre).
struct Faux {
    url: String,
    requetes: Arc<Mutex<Vec<(String, String)>>>,
}

impl Faux {
    fn new(repondre: impl Fn(usize, &str) -> (u16, String, Duration) + Send + 'static) -> Faux {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let requetes = Arc::new(Mutex::new(Vec::new()));
        let r = requetes.clone();
        std::thread::spawn(move || {
            for mut req in server.incoming_requests() {
                let mut corps = String::new();
                let _ = req.as_reader().read_to_string(&mut corps);
                let auth = req.headers().iter().find(|h| h.field.equiv("Authorization")).map(|h| h.value.to_string()).unwrap_or_default();
                let n = {
                    let mut v = r.lock().unwrap();
                    v.push((auth, corps.clone()));
                    v.len()
                };
                let (code, rep, pause) = repondre(n, &corps);
                std::thread::sleep(pause);
                let _ = req.respond(tiny_http::Response::from_string(rep).with_status_code(code));
            }
        });
        Faux { url: format!("http://127.0.0.1:{port}/api/v1/chat/completions"), requetes }
    }

    /// Répond toujours la même chose, tout de suite.
    fn qui_repond(texte: &'static str) -> Faux {
        Faux::new(move |_, _| (200, ia_dit(texte, "stop"), Duration::ZERO))
    }

    fn n(&self) -> usize {
        self.requetes.lock().unwrap().len()
    }

    fn corps(&self, i: usize) -> serde_json::Value {
        serde_json::from_str(&self.requetes.lock().unwrap()[i].1).unwrap()
    }
}

fn ia_dit(texte: &str, fin: &str) -> String {
    serde_json::json!({
        "id": "gen-1",
        "choices": [{ "message": { "role": "assistant", "content": texte }, "finish_reason": fin }],
        "usage": { "prompt_tokens": 800, "completion_tokens": 50, "prompt_tokens_details": { "cached_tokens": 768 } }
    })
    .to_string()
}

/// Fichier de clé temporaire (un par test).
fn fichier_cle(nom: &str, contenu: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("v34-oracle-{}-{nom}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("openrouter.key");
    std::fs::write(&p, contenu).unwrap();
    p
}

fn oracle(url: &str, cle: &std::path::Path, regler: impl FnOnce(&mut config::Oracle)) -> Oracle {
    set_level(Level::Error);
    let mut c = config::Oracle {
        url: url.into(),
        fichier_cle: cle.display().to_string(),
        questions_par_jour: 100,
        questions_par_jour_total: 1000,
        delai_s: 2,
        debit_affichage: 56000,
        ..Default::default()
    };
    regler(&mut c);
    let mut o = Oracle::new(&c).expect("configuration de test valide");
    o.attente_cartes = Duration::ZERO;
    o
}

#[test]
fn quotas_par_abonne_et_plafond_global() {
    let f = Faux::qui_repond("Les astres disent oui.");
    let cle = fichier_cle("quotas", CLE);
    let o = oracle(&f.url, &cle, |c| {
        c.questions_par_jour = 3;
        c.questions_par_jour_total = 5;
    });
    let k = o.cle().unwrap();
    let mut alice = Conversation::new("alice");
    for i in 0..3 {
        let r = o.repondre(&mut alice, &format!("question {i} d'alice"), Some(&k));
        assert_eq!(r.source, Source::Ia, "{r:?}");
    }
    assert_eq!(o.restantes("alice"), 0);
    let r = o.repondre(&mut alice, "encore une ?", Some(&k));
    assert_eq!((r.source, r.texte.as_str()), (Source::Refus, persona::QUOTA_ABONNE), "quota de l'abonné épuisé");

    // Bob : deux questions, puis le plafond global (5) est atteint
    let mut bob = Conversation::new("bob");
    assert_eq!(o.repondre(&mut bob, "première de bob", Some(&k)).source, Source::Ia);
    assert_eq!(o.repondre(&mut bob, "deuxième de bob", Some(&k)).source, Source::Ia);
    let r = o.repondre(&mut bob, "troisième de bob", Some(&k));
    assert_eq!(r.source, Source::Cartes, "au-delà du plafond, les cartes");
    assert!(r.texte.starts_with(persona::QUOTA_GLOBAL), "{}", r.texte);
    assert_eq!(f.n(), 5, "jamais plus de requêtes que le plafond global");

    // Le lendemain, tout recommence
    let demain = jour_utc() + 1;
    match o.preparer_le(&mut alice, "et demain ?", Some(&k), demain) {
        Preparation::Requete(_) => {}
        Preparation::Immediate(r) => panic!("le quota devrait être remis à zéro : {r:?}"),
    }
    assert_eq!(o.restantes_le("alice", demain), 2);
    let c = o.compteurs();
    assert_eq!((c.ia, c.refus, c.cartes), (5, 1, 1));
}

#[test]
fn regle_d_acces() {
    use v34_net::auth::PasswordHash;
    let h = PasswordHash::new("x", 1000);
    let toml = format!(
        "[[abonnes]]\nnom = \"alice\"\nnumero = \"01\"\nmot_de_passe_hash = \"{h}\"\n\
         [[abonnes]]\nnom = \"bob\"\nnumero = \"02\"\nmot_de_passe_hash = \"{h}\"\nfai = false\n"
    );
    let annuaire = crate::Config::from_toml(&toml).unwrap().annuaire().unwrap();
    let cle = fichier_cle("acces", CLE);
    let par_defaut = oracle("https://openrouter.ai/api/v1/chat/completions", &cle, |_| {});
    assert!(par_defaut.reserve_au_fai());
    assert!(par_defaut.autorise(&annuaire, "alice"));
    assert!(!par_defaut.autorise(&annuaire, "bob"), "sans accès Internet, pas d'oracle par défaut");
    assert!(!par_defaut.autorise(&annuaire, "inconnu"));
    let tous = oracle("https://openrouter.ai/api/v1/chat/completions", &cle, |c| c.acces = "tous".into());
    assert!(!tous.reserve_au_fai());
    assert!(tous.autorise(&annuaire, "bob"), "acces = tous : tous les abonnés");
    assert!(!tous.autorise(&annuaire, "inconnu"));
}

#[test]
fn sans_cle_les_cartes() {
    let f = Faux::qui_repond("jamais appelé");
    let absente = std::env::temp_dir().join("v34-oracle-nulle-part/openrouter.key");
    let o = oracle(&f.url, &absente, |_| {});
    assert!(o.cle().is_err());
    let mut conv = Conversation::new("alice");
    let r = o.repondre(&mut conv, "Vais-je être riche ?", None);
    assert_eq!(r.source, Source::Cartes);
    assert!(!r.texte.is_empty());
    assert_eq!(f.n(), 0, "hors ligne : aucune requête");
    assert_eq!(o.restantes("alice"), 100, "les cartes ne coûtent rien");
    // clés invalides
    assert!(lire_cle(fichier_cle("vide", "  \n").to_str().unwrap()).is_err());
    assert!(lire_cle(fichier_cle("espaces", "deux mots").to_str().unwrap()).is_err());
    assert!(lire_cle("").is_err());
    assert_eq!(lire_cle(fichier_cle("ok", &format!("{CLE}\n")).to_str().unwrap()).unwrap(), CLE);
}

#[test]
fn pannes_et_delais_se_rabattent_sur_les_cartes() {
    let cle = fichier_cle("pannes", CLE);
    let cas: Vec<(Faux, &str, bool)> = vec![
        (Faux::new(|_, _| (500, "oups".into(), Duration::ZERO)), persona::NOTE_PANNE, true),
        (Faux::new(|_, _| (200, r#"{"error":{"code":429,"message":"Rate limited"}}"#.into(), Duration::ZERO)), persona::NOTE_PANNE, true),
        (Faux::new(|_, _| (200, r#"{"choices":[{"message":{"content":null,"reasoning":"hmm"}}]}"#.into(), Duration::ZERO)), persona::NOTE_PANNE, true),
        (Faux::new(|_, _| (200, "<html>passerelle</html>".into(), Duration::ZERO)), persona::NOTE_PANNE, true),
        // trop lent : délai dépassé (la question reste comptée, elle a pu être facturée)
        (Faux::new(|_, _| (200, ia_dit("Trop tard.", "stop"), Duration::from_millis(2500))), persona::NOTE_DELAI, false),
    ];
    for (f, note, rendue) in cas {
        let o = oracle(&f.url, &cle, |c| c.delai_s = 1);
        let k = o.cle().unwrap();
        let mut conv = Conversation::new("alice");
        let t0 = Instant::now();
        let r = o.repondre(&mut conv, "Que me réserve l'avenir ?", Some(&k));
        assert!(t0.elapsed() < Duration::from_millis(2000), "jamais d'appel bloqué ({:?})", t0.elapsed());
        assert_eq!(r.source, Source::Cartes, "{}", f.url);
        assert!(r.texte.starts_with(note), "{}", r.texte);
        assert_eq!(o.restantes("alice"), if rendue { 100 } else { 99 });
        assert!(conv.echanges().is_empty(), "les cartes ne vont pas dans la mémoire envoyée à l'IA");
        assert_eq!(o.compteurs().erreurs, 1);
    }
    // personne au bout du fil
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1/chat/completions", l.local_addr().unwrap());
    drop(l);
    let o = oracle(&url, &cle, |_| {});
    let r = o.repondre(&mut Conversation::new("alice"), "Allô ?", Some(CLE));
    assert!(r.texte.starts_with(persona::NOTE_PANNE), "{}", r.texte);
}

#[test]
fn reponses_coupees_et_nettoyees() {
    let long = format!("**Vision** : {} Et puis enfin la suite qui ne finit jam", "Les brumes s'écartent lentement sur la ville. ".repeat(15));
    let f = Faux::new(move |_, _| (200, ia_dit(&long, "length"), Duration::ZERO));
    let cle = fichier_cle("coupe", CLE);
    let o = oracle(&f.url, &cle, |_| {});
    let r = o.repondre(&mut Conversation::new("alice"), "Que vois-tu ?", Some(CLE));
    assert_eq!(r.source, Source::Ia);
    assert!(r.texte.chars().count() <= texte::REPONSE_MAX, "{}", r.texte.len());
    assert!(r.texte.ends_with("ville."), "coupée à la fin d'une phrase : {}", r.texte);
    assert!(r.texte.starts_with("Vision : Les brumes"), "sans Markdown : {}", r.texte);
}

#[test]
fn requete_au_prefixe_stable_sans_raisonnement_et_jetons_comptes() {
    let f = Faux::new(|n, _| (200, ia_dit(&format!("Réponse numéro {n}."), "stop"), Duration::ZERO));
    let cle = fichier_cle("prefixe", CLE);
    let o = oracle(&f.url, &cle, |c| c.jetons_max = 180);
    // (des noms qu'on ne risque pas de trouver dans la consigne : « malice » contient « alice »)
    let mut alice = Conversation::new("gaston_lagaffe");
    let mut bob = Conversation::new("mlle_jeanne");
    o.repondre(&mut alice, "Qui gagnera la Coupe ?", Some(CLE));
    o.repondre(&mut bob, "Quel temps fera-t-il ?", Some(CLE));
    o.repondre(&mut alice, "Et après ?", Some(CLE));
    assert_eq!(f.n(), 3);
    let systeme = serde_json::json!({ "role": "system", "content": persona::SYSTEME });
    for i in 0..3 {
        let (auth, brut) = f.requetes.lock().unwrap()[i].clone();
        assert_eq!(auth, format!("Bearer {CLE}"));
        let v = f.corps(i);
        assert_eq!(v["messages"][0], systeme, "la consigne d'abord, identique pour tous");
        assert_eq!(v["model"], "deepseek/deepseek-v4.1-flash");
        assert_eq!(v["max_tokens"], 180);
        assert_eq!(v["reasoning"]["enabled"], false, "pas de jetons de réflexion");
        assert_eq!(v["stream"], false);
        assert!(!brut.contains("gaston") && !brut.contains("jeanne"), "rien sur l'abonné dans la requête");
    }
    // 2e question d'alice : son 1er échange renvoyé tel quel, la question en dernier
    let v1 = f.corps(0);
    let v3 = f.corps(2);
    assert_eq!(v3["messages"][1], v1["messages"][1]);
    assert_eq!(v3["messages"][2], serde_json::json!({ "role": "assistant", "content": "Réponse numéro 1." }));
    assert_eq!(v3["messages"][3], serde_json::json!({ "role": "user", "content": "Et après ?" }));
    assert_eq!(v3["messages"].as_array().unwrap().len(), 4);
    let c = o.compteurs();
    assert_eq!((c.jetons_prompt, c.jetons_caches, c.jetons_reponse), (2400, 2304, 150));
}

#[test]
fn memoire_bornee_et_prefixe_stable() {
    let f = Faux::new(|n, _| (200, ia_dit(&format!("Réponse {n}."), "stop"), Duration::ZERO));
    let cle = fichier_cle("memoire", CLE);
    let o = oracle(&f.url, &cle, |_| {});
    let mut conv = Conversation::new("alice");
    for i in 0..20 {
        assert_eq!(o.repondre(&mut conv, &format!("Question {i} ?"), Some(CLE)).source, Source::Ia);
        assert!(conv.echanges().len() <= MEMOIRE_MAX);
    }
    assert_eq!(f.n(), 20);
    let msgs: Vec<Vec<serde_json::Value>> = (0..20).map(|i| f.corps(i)["messages"].as_array().unwrap().clone()).collect();
    let mut prolongements = 0;
    for i in 0..20 {
        assert!(msgs[i].len() <= 2 + 2 * MEMOIRE_MAX, "requête {i} : {} messages", msgs[i].len());
        assert_eq!(msgs[i].last().unwrap()["content"], format!("Question {i} ?"), "la question en dernier");
        // la requête précédente entière (question comprise) en tête de celle-ci
        if i > 0 && msgs[i].len() > msgs[i - 1].len() && msgs[i][..msgs[i - 1].len()] == msgs[i - 1][..] {
            prolongements += 1;
        }
    }
    // l'oubli se fait par paquets : le préfixe reste le même la plupart du temps
    assert!(prolongements >= 15, "{prolongements} requêtes sur 19 prolongent la précédente");
    // la dernière requête se souvient des 4 à 8 derniers échanges seulement
    let derniere = &msgs[19];
    assert!(derniere.iter().all(|m| m["content"] != "Question 0 ?"), "les vieux échanges sont oubliés");
}

#[test]
fn cache_des_premieres_questions() {
    let f = Faux::qui_repond("Les astres sont formels : oui.");
    let cle = fichier_cle("cache", CLE);
    let o = oracle(&f.url, &cle, |_| {});
    let r1 = o.repondre(&mut Conversation::new("alice"), "Qui va gagner la Coupe du monde ?", Some(CLE));
    let mut bob = Conversation::new("bob");
    let r2 = o.repondre(&mut bob, "qui va gagner la coupe du monde", Some(CLE));
    assert_eq!((r1.source, r2.source), (Source::Ia, Source::Cache));
    assert_eq!(r1.texte, r2.texte);
    assert_eq!(f.n(), 1, "une seule requête pour deux fois la même première question");
    assert_eq!(o.restantes("bob"), 100, "une réponse du cache ne coûte rien");
    assert_eq!(bob.echanges().len(), 1, "elle entre dans la mémoire de l'appel");
    // la suite de la conversation n'est jamais servie par le cache
    o.repondre(&mut bob, "Qui va gagner la Coupe du monde ?", Some(CLE));
    assert_eq!(f.n(), 2);
    assert_eq!(o.compteurs().cache, 1);
}

/// Texte des trames de chat (tout doit être en trames de chat).
fn lire_ecran(octets: &[u8]) -> (String, usize) {
    let mut d = Decoder::default();
    let mut texte = String::new();
    let mut trames = 0;
    for it in d.push(octets) {
        match it {
            Item::Frame(Frame::Chat(t)) => {
                texte.push_str(&t);
                trames += 1;
            }
            autre => panic!("l'oracle n'envoie que des trames de chat : {autre:?}"),
        }
    }
    (texte, trames)
}

#[test]
fn cadence_de_l_ecran() {
    let (tx, rx) = mpsc::sync_channel(1024);
    let mut e = Ecran::new(tx, 1200);
    let texte = "x".repeat(60);
    let t0 = Instant::now();
    e.lignes(&texte).unwrap();
    let dt = t0.elapsed().as_secs_f64();
    // 1200 bit/s = 120 caractères par seconde : 60 caractères ≈ 0,5 s
    assert!((0.35..0.8).contains(&dt), "60 caractères en {dt:.2} s");
    let octets: Vec<u8> = rx.try_iter().flatten().collect();
    let (t, trames) = lire_ecran(&octets);
    assert_eq!(t, format!("\n{texte}"));
    assert!(trames >= 4, "envoyé par petits morceaux ({trames} trames)");
    // chaque ligne commence par un retour à la ligne, jamais le texte ne finit par un
    e.lignes("a\nb").unwrap();
    e.suite("...").unwrap();
    let (t, _) = lire_ecran(&rx.try_iter().flatten().collect::<Vec<u8>>());
    assert_eq!(t, "\na\nb...");
}

/// Fait tourner une consultation ; renvoie (envoyer, recevoir, fin).
fn consultation_test(o: Oracle, abonne: &str) -> (mpsc::Sender<Vec<u8>>, mpsc::Receiver<Vec<u8>>, std::thread::JoinHandle<String>) {
    let (in_tx, in_rx) = mpsc::channel();
    let (out_tx, out_rx) = mpsc::sync_channel(4096);
    let o = Arc::new(o);
    let a = abonne.to_string();
    let h = std::thread::spawn(move || session(o, "ORACLE test", &a, in_rx, out_tx));
    (in_tx, out_rx, h)
}

/// Lit l'écran jusqu'à voir `motif` (5 s au plus).
fn attendre(rx: &mpsc::Receiver<Vec<u8>>, tout: &mut Vec<u8>, motif: &str) -> String {
    let t0 = Instant::now();
    loop {
        let (t, _) = lire_ecran(tout);
        if t.contains(motif) {
            return t;
        }
        assert!(t0.elapsed() < Duration::from_secs(5), "« {motif} » jamais affiché ; écran :\n{t}");
        if let Ok(b) = rx.recv_timeout(Duration::from_millis(50)) {
            tout.extend(b);
        }
    }
}

#[test]
fn consultation_complete_avec_l_ia() {
    let f = Faux::new(|_, _| (200, ia_dit("Les astres disent oui, mortel.", "stop"), Duration::from_millis(800)));
    let cle = fichier_cle("session", CLE);
    let o = oracle(&f.url, &cle, |_| {});
    let (tx, rx, fin) = consultation_test(o, "gaston");
    let mut ecran = Vec::new();
    let t = attendre(&rx, &mut ecran, "/fin : raccrocher.");
    assert!(t.contains("3 6 1 5") && t.contains("2,23 F la minute") && t.contains("Bienvenue, gaston") && t.contains("100 questions"), "{t}");
    tx.send(bbs::encode(&Frame::Chat("Vais-je être riche ?".into()))).unwrap();
    let t = attendre(&rx, &mut ecran, "mortel.");
    assert!(t.contains(&format!("\n{}.", persona::ATTENTE_ASTRES)), "des points pendant l'attente : {t}");
    assert!(t.contains("\nLes astres disent oui, mortel."), "{t}");
    attendre(&rx, &mut ecran, " F ]");
    tx.send(bbs::encode(&Frame::Chat("/fin".into()))).unwrap();
    let t = attendre(&rx, &mut ecran, "Au revoir !");
    assert_eq!(fin.join().unwrap(), "consultation terminée (/fin)");
    assert!(t.starts_with("\n   3 6 1 5") && !t.ends_with('\n'), "un retour à la ligne avant chaque ligne, jamais après");
    assert!(!t.contains("\n\n\n"));
    assert_eq!(f.n(), 1);
}

#[test]
fn consultation_hors_ligne_texte_brut_et_raccroche() {
    let absente = std::env::temp_dir().join("v34-oracle-nulle-part/openrouter.key");
    let o = oracle("http://127.0.0.1:9/", &absente, |_| {});
    let (tx, rx, fin) = consultation_test(o, "gaston");
    let mut ecran = Vec::new();
    let t = attendre(&rx, &mut ecran, "/fin : raccrocher.");
    assert!(t.contains("les cartes qui répondent"), "{t}");
    // un terminal à l'ancienne envoie du texte brut, ligne par ligne
    tx.send(b"Qui a gagne la coupe du monde ?\r".to_vec()).unwrap();
    let t = attendre(&rx, &mut ecran, " F ]");
    assert!(t.contains("Brésil") || t.contains("Stade de France"), "réponse à thème : {t}");
    tx.send(bbs::encode(&Frame::Chat("/aide".into()))).unwrap();
    attendre(&rx, &mut ecran, "GUIDE");
    drop(tx);
    assert_eq!(fin.join().unwrap(), "raccroché par l'appelant");
}

#[test]
fn consultation_quota_epuise_et_inactivite() {
    let f = Faux::qui_repond("jamais appelé");
    let cle = fichier_cle("epuise", CLE);
    let o = oracle(&f.url, &cle, |c| c.questions_par_jour = 1);
    o.reserver("gaston", jour_utc()).unwrap();
    let (_tx, rx, fin) = consultation_test(o, "gaston");
    assert_eq!(fin.join().unwrap(), "quota du jour épuisé", "l'oracle raccroche de lui-même");
    let t = attendre(&rx, &mut Vec::new(), persona::QUOTA_ABONNE);
    assert!(!t.contains("Posez votre question"));
    assert_eq!(f.n(), 0);

    let mut o = oracle(&f.url, &cle, |_| {});
    o.cfg.inactivite_s = 1; // (30 s au moins dans la configuration)
    let (_tx, rx, fin) = consultation_test(o, "gaston");
    let t0 = Instant::now();
    assert_eq!(fin.join().unwrap(), "raccroché par l'oracle (inactivité)");
    assert!(t0.elapsed() < Duration::from_secs(4));
    attendre(&rx, &mut Vec::new(), "Au revoir !");
}
