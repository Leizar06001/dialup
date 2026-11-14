//! 01 45 78 19 98 : le serveur du Ministère des Postes et Télécommunications,
//! service des Lignes Perdues. « SYSTÈME 98 », réservé aux agents : un petit
//! jeu de piratage.
//!
//! Trois niveaux d'habilitation, chacun avec son mot de passe caché ailleurs
//! sur le réseau :
//! 1. agent (n'importe quel identifiant) : « bogue », le mot officiel pour
//!    « bug », soufflé par la Commission de l'an 2000 (0 800 20 00 00) ;
//! 2. chef : le nom de son chat, qu'il a dicté sur le répondeur de Gérard
//!    (01 43 33 12 12) en 1997 ;
//! 3. directeur : le prix, en francs, de son cadeau de Noël (un Tamagotchi),
//!    que connaît le jeu du Prix en Francs (08 36 15 19 98).
//!
//! Le dossier rouge du directeur donne le dernier numéro caché, la ligne de
//! 2026. Succès : « Promotion » (chef), « Secret d'État » (dossier rouge).
//!
//! Un vrai petit système de fichiers, en lecture seule : LISTE, VA, LIRE,
//! plus QUI, ANNUAIRE, JOURNAL, CONNEXION, et quelques surprises.

use super::{choix, hasard, Fin, Ligne};

pub const MOT_DE_PASSE: &str = "bogue";
pub const MOT_DE_PASSE_CHEF: &str = "minouche";
pub const MOT_DE_PASSE_DIRECTEUR: &str = "150";

/// Un fichier : (dossier, nom, niveau requis, contenu)
struct Fichier {
    dossier: &'static str,
    nom: &'static str,
    niveau: u8,
    texte: &'static str,
}

const DOSSIERS: &[(&str, u8)] = &[("BUREAU", 1), ("COURRIER", 1), ("SYSTEME", 1), ("ARCHIVES", 2), ("DIRECTION", 3)];

const FICHIERS: &[Fichier] = &[
    Fichier {
        dossier: "BUREAU",
        nom: "LISEZMOI.TXT",
        niveau: 1,
        texte: "Bienvenue au Service des Lignes Perdues.\n\
Ici, on retrouve les lignes que les abonnés perdent, et on perd celles qu'on retrouve.\n\
Règle n° 1 : ne jamais noter son mot de passe.\n\
Règle n° 2 : voir la règle n° 1.\n\
Pour toute question, adressez-vous à votre chef de service (habilitation de niveau 2).",
    },
    Fichier {
        dossier: "BUREAU",
        nom: "POSTIT.TXT",
        niveau: 1,
        texte: "[un post-it jaune, collé sur l'écran]\n\
« Rappeler Gérard au 01 43 33 12 12. Il a encore sur son répondeur le message où le chef\n\
dicte son nouveau mot de passe. Quelle idée de dire ça au téléphone ! »",
    },
    Fichier {
        dossier: "BUREAU",
        nom: "REGISTRE.TXT",
        niveau: 1,
        texte: "REGISTRE DES NUMÉROS NON ATTRIBUÉS (confidentiel)\n\
01 43 33 12 12 : ligne de M. Gérard, abonné parti sans résilier. Le répondeur tourne toujours.\n\
08 36 15 19 98 : jeu « Le Prix en Francs », dossier d'agrément en cours depuis 1987.\n\
08 36 99 36 99 : horloge parlante. Fonctionne. Personne ne sait pourquoi.\n\
08 36 68 68 68 : météo des lignes. Prévisions non contractuelles.\n\
0 800 20 00 00 : Commission de l'an 2000. Dissolution prévue le 2 janvier 2000.\n\
(Un numéro a été retiré du registre sur ordre de la direction. Voir DIRECTION.)",
    },
    Fichier {
        dossier: "BUREAU",
        nom: "CAFE.TXT",
        niveau: 1,
        texte: "Note de service n° 98-1211 : la machine à café est en panne. Une commission d'enquête a été nommée.\n\
Note de service n° 98-1212 : la commission d'enquête demande du café.",
    },
    Fichier {
        dossier: "COURRIER",
        nom: "DU-CHEF.TXT",
        niveau: 1,
        texte: "De : le chef de service\nÀ : tous les agents\n\
Le dossier ROUGE est rangé dans DIRECTION. Seul le directeur y a accès.\n\
Inutile d'essayer de deviner son mot de passe : c'est un prix, en francs,\n\
et personne ici ne connaît le prix de quoi que ce soit.",
    },
    Fichier {
        dossier: "COURRIER",
        nom: "DE-GERARD.TXT",
        niveau: 1,
        texte: "De : Gérard\nÀ : le service\n\
Je pars. Je laisse ma ligne ouverte et mon répondeur branché, on ne sait jamais.\n\
Le chef a mon numéro. Il y laisse d'ailleurs souvent des messages qu'il vaudrait mieux ne pas laisser.\n\
— Gérard, le 14 février 1997",
    },
    Fichier {
        dossier: "SYSTEME",
        nom: "JOURNAL.LOG",
        niveau: 1,
        texte: JOURNAL,
    },
    Fichier {
        dossier: "ARCHIVES",
        nom: "FACTURES.TXT",
        niveau: 2,
        texte: "NOTES DE FRAIS DE LA DIRECTION (exercice 1997)\n\
12/03/1997 · Agrafeuse (modèle de luxe) : 89 F\n\
04/07/1997 · Ventilateur de bureau : 249 F\n\
25/12/1997 · Cadeau de Noël du directeur : un Tamagotchi. Prix : non renseigné\n\
             (le service comptable suggère d'appeler le jeu du 08 36 15 19 98).\n\
Remarque du chef : le directeur a pris le prix de son Tamagotchi comme mot de passe.\n\
Il n'a aucune imagination.",
    },
    Fichier {
        dossier: "ARCHIVES",
        nom: "PERSONNEL.TXT",
        niveau: 2,
        texte: "FICHES DU PERSONNEL\n\
Gérard : parti en 1997, ligne jamais résiliée. Motif du départ : « trop de lignes perdues ».\n\
Le chef : chat nommé Minouche, allergique au fax.\n\
Le directeur : ne quitte jamais son bureau. Joue au Prix en Francs pendant les réunions.",
    },
    Fichier {
        dossier: "ARCHIVES",
        nom: "AN2000.TXT",
        niveau: 2,
        texte: "PLAN DE PASSAGE À L'AN 2000\n\
Étape 1 : constituer une commission.\nÉtape 2 : attendre.\nÉtape 3 : voir étape 2.",
    },
    Fichier {
        dossier: "DIRECTION",
        nom: "ROUGE.TXT",
        niveau: 3,
        texte: "DOSSIER ROUGE · SECRET D'ÉTAT\n\
Le 3 novembre 1998, à 3 h 33, le standard du ministère a reçu un appel venu… de l'année 2026.\n\
L'appelant connaissait le résultat de la Coupe du monde avant la finale, la date de\n\
l'arrêt du Minitel, et le prix d'une baguette en euros. Le ministère a classé l'affaire.\n\
La ligne existe toujours. Elle a été retirée de tous les registres :\n\
                      08 36 20 26 26\n\
Ne la composez sous aucun prétexte. (Le directeur l'appelle tous les vendredis.)",
    },
    Fichier {
        dossier: "DIRECTION",
        nom: "MINISTRE.TXT",
        niveau: 3,
        texte: "Le ministre ne lit pas ses fichiers. Le ministre fait lire ses fichiers.",
    },
];

const JOURNAL: &str = "JOURNAL DU SYSTÈME 98\n\
02/11/1998 08:59 chef : connexion (mot de passe incorrect ×4, puis accepté — merci Gérard)\n\
02/11/1998 09:02 chef : lecture de ARCHIVES/FACTURES.TXT\n\
02/11/1998 11:47 directeur : connexion depuis le 08 36 15 19 98 (encore ce jeu…)\n\
03/11/1998 03:33 standard : APPEL ENTRANT D'ORIGINE INCONNUE (année d'émission : 2026 ?)\n\
03/11/1998 03:34 directeur : création de DIRECTION/ROUGE.TXT\n\
14/11/1998 23:59 agent : tentative de lecture de DIRECTION/ROUGE.TXT — ACCÈS REFUSÉ";

const ANNUAIRE: &str = "ANNUAIRE INTERNE\n\
Standard du ministère ........ 01 45 78 19 98\n\
Horloge parlante ............. 08 36 99 36 99\n\
Météo des lignes ............. 08 36 68 68 68\n\
Commission de l'an 2000 ...... 0 800 20 00 00\n\
Gérard (parti) ............... 01 43 33 12 12\n\
Ligne directe du ministre .... ** ** ** ** ** (habilitation de niveau 4)";

const AIDE: &str = "COMMANDES DU SYSTÈME 98\n\
LISTE ............ ce que contient le dossier courant\n\
VA <dossier> ..... changer de dossier (VA .. pour remonter)\n\
LIRE <fichier> ... afficher un fichier\n\
QUI .............. qui êtes-vous, qui est connecté\n\
ANNUAIRE ......... les numéros internes\n\
JOURNAL .......... le journal du système\n\
CONNEXION <nom> .. changer d'identité (chef, directeur…)\n\
FIN .............. se déconnecter";

const REFUS: &[&str] = &[
    "Cet incident sera signalé à votre chef de service.",
    "Votre demande a été transmise au service compétent, qui ne vous répondra pas.",
    "Un formulaire de demande d'habilitation est disponible au guichet 12, ouvert le mardi de 14 h à 14 h 15.",
];

/// « Bogue », « BOGUE. », « bogue ! »… : un mot de passe, sans la casse ni
/// la ponctuation.
fn norme(s: &str) -> String {
    s.trim().to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

/// Niveau du compte, et son mot de passe.
fn compte(nom: &str) -> (u8, &'static str) {
    match norme(nom).as_str() {
        "chef" | "chefdeservice" => (2, MOT_DE_PASSE_CHEF),
        "directeur" | "dir" => (3, MOT_DE_PASSE_DIRECTEUR),
        _ => (1, MOT_DE_PASSE),
    }
}

fn titre(niveau: u8) -> &'static str {
    match niveau {
        3 => "directeur",
        2 => "chef de service",
        _ => "agent",
    }
}

struct Session {
    nom: String,
    niveau: u8,
    /// Dossier courant (vide : la racine)
    dossier: &'static str,
}

impl Session {
    fn invite(&self) -> String {
        if self.dossier.is_empty() {
            "SLP:/>".into()
        } else {
            format!("SLP:/{}>", self.dossier)
        }
    }
}

/// Demande un mot de passe pour ce compte ; vrai s'il est bon.
fn verifier(l: &mut Ligne, nom: &str) -> Result<bool, Fin> {
    let (_, mdp) = compte(nom);
    l.dire("MOT DE PASSE ? (il s'affiche en clair : c'est normal, c'est le ministère)")?;
    let tape = l.attendre()?;
    // un prix (le directeur) : « 150 F », « 150 francs », « 150,00 »…
    if mdp.chars().all(|c| c.is_ascii_digit()) {
        return Ok(super::prix::lire_prix(&tape).map(|p| p.to_string()).as_deref() == Some(mdp));
    }
    Ok(norme(&tape) == mdp)
}

/// Une nouvelle identité : succès éventuel, et salut.
fn entrer(l: &mut Ligne, s: &mut Session, nom: &str) -> Result<(), Fin> {
    let (niveau, _) = compte(nom);
    s.niveau = niveau;
    s.nom = nom.to_uppercase();
    if niveau >= 2 {
        l.succes("chef");
    }
    let accueil = match niveau {
        3 => "Bonjour, Monsieur le directeur. Votre partie de Prix en Francs vous attend.".to_string(),
        2 => "Bonjour, chef. Minouche vous salue.".to_string(),
        _ => format!("Vous avez {} dossiers en retard.", 1000 + hasard(9000)),
    };
    l.dire(&format!("BIENVENUE, {} {}. Niveau d'habilitation : {niveau} ({}).\n{accueil}\nTapez AIDE pour la liste des commandes.", titre(niveau).to_uppercase(), s.nom, titre(niveau)))
}

pub fn lancer(l: &mut Ligne) -> Result<(), Fin> {
    l.dire(
        "MINISTÈRE DES POSTES ET TÉLÉCOMMUNICATIONS\n\
         Service des Lignes Perdues · SYSTÈME 98 · réservé aux agents\n\
         Toute tentative d'accès non autorisée sera tamponnée en trois exemplaires.\n\
         IDENTIFIANT ?",
    )?;
    let nom = l.attendre()?;
    let mut ok = false;
    for essai in 1..=3 {
        if verifier(l, &nom)? {
            ok = true;
            break;
        }
        if essai == 3 {
            break;
        }
        l.dire(&format!("MOT DE PASSE INCORRECT (tentative {essai} sur 3)."))?;
        if essai == 2 && compte(&nom).0 == 1 {
            l.dire("Rappel du service informatique : le mot de passe des agents est le terme officiel, en bon français, d'un défaut informatique dont tout le monde parle en ce moment.")?;
        }
    }
    if !ok {
        l.dire("ACCÈS REFUSÉ. Votre dossier a été transmis au service compétent, qui vous répondra sous 6 à 8 semaines. Ne quittez pas… *clic*")?;
        return Ok(());
    }
    let mut s = Session { nom: String::new(), niveau: 1, dossier: "" };
    entrer(l, &mut s, &nom)?;
    l.dire(&s.invite())?;
    loop {
        let ligne = l.attendre()?;
        let mut mots = ligne.split_whitespace();
        let cmd = mots.next().unwrap_or("").to_uppercase();
        let arg = mots.collect::<Vec<_>>().join(" ");
        match cmd.as_str() {
            "AIDE" | "?" | "HELP" => l.dire(AIDE)?,
            "LISTE" | "LS" | "DIR" => liste(l, &s)?,
            "VA" | "CD" => va(l, &mut s, &arg)?,
            "LIRE" | "CAT" | "TYPE" | "MORE" => lire(l, &s, &arg)?,
            "QUI" | "WHOAMI" => l.dire(&format!("Vous êtes {} {}, niveau {}.\nConnectés : vous, et le directeur (inactif depuis 1997).", titre(s.niveau).to_uppercase(), s.nom, s.niveau))?,
            "ANNUAIRE" => l.dire(ANNUAIRE)?,
            "JOURNAL" | "LOG" => l.dire(JOURNAL)?,
            "CONNEXION" | "SU" | "LOGIN" => {
                if arg.is_empty() {
                    l.dire("CONNEXION <nom> : par exemple CONNEXION chef.")?;
                } else if verifier(l, &arg)? {
                    entrer(l, &mut s, &arg)?;
                } else {
                    l.dire(&format!("MOT DE PASSE INCORRECT. {}", choix(REFUS)))?;
                }
            }
            "FIN" | "QUITTER" | "EXIT" | "LOGOUT" => {
                l.dire("Déconnexion. N'oubliez pas de pointer en sortant. *clic*")?;
                return Ok(());
            }
            // quelques surprises
            "SUDO" | "ROOT" => l.dire("Ce n'est pas un système UNIX, ici. Pour obtenir des droits supplémentaires, remplissez le formulaire Cerfa n° 98-SU en trois exemplaires.")?,
            "CAFE" | "CAFÉ" => l.dire("La machine à café est en panne. Voir BUREAU/CAFE.TXT.")?,
            "MINISTRE" => l.dire("Le ministre ne prend pas d'appels. Le ministre est en réunion. Le ministre est toujours en réunion.")?,
            "PIRATER" | "HACK" | "HACKER" => l.dire("Le piratage est formellement interdit au ministère. Nous vous prions de bien vouloir lire le JOURNAL, puis de ne rien en faire.")?,
            "" => {}
            _ => l.dire("COMMANDE INCONNUE. Tapez AIDE.")?,
        }
        l.dire(&s.invite())?;
    }
}

fn liste(l: &mut Ligne, s: &Session) -> Result<(), Fin> {
    let mut t = String::new();
    if s.dossier.is_empty() {
        for (d, n) in DOSSIERS {
            let marque = if *n > s.niveau { format!("   [niveau {n}]") } else { String::new() };
            t.push_str(&format!("{d}/{marque}\n"));
        }
    } else {
        for f in FICHIERS.iter().filter(|f| f.dossier == s.dossier) {
            let marque = if f.niveau > s.niveau { format!("   [niveau {}]", f.niveau) } else { String::new() };
            t.push_str(&format!("{}{marque}\n", f.nom));
        }
        t.push_str("..  (dossier parent)");
    }
    l.dire(t.trim_end())
}

fn va(l: &mut Ligne, s: &mut Session, arg: &str) -> Result<(), Fin> {
    let a = arg.trim().trim_matches('/').to_uppercase();
    if a.is_empty() || a == ".." || a == "/" || a == "\\" {
        s.dossier = "";
        return Ok(());
    }
    let a = a.trim_end_matches('/');
    match DOSSIERS.iter().find(|(d, _)| *d == a) {
        Some((d, n)) if *n > s.niveau => l.dire(&format!("ACCÈS REFUSÉ : {d} demande une habilitation de niveau {n}. {}", choix(REFUS))),
        Some((d, _)) => {
            s.dossier = d;
            Ok(())
        }
        None => l.dire(&format!("DOSSIER INTROUVABLE : {a}. (LISTE pour voir ce qu'il y a.)")),
    }
}

fn lire(l: &mut Ligne, s: &Session, arg: &str) -> Result<(), Fin> {
    let a = arg.trim().to_uppercase();
    if a.is_empty() {
        return l.dire("LIRE <fichier> : par exemple LIRE LISEZMOI.TXT.");
    }
    // « LIRE DOSSIER/FICHIER » ou un fichier du dossier courant (extension facultative)
    let (dossier, nom) = match a.split_once('/') {
        Some((d, n)) => (d.to_string(), n.to_string()),
        None => (s.dossier.to_string(), a.clone()),
    };
    let f = FICHIERS.iter().find(|f| f.dossier == dossier && (f.nom == nom || f.nom.trim_end_matches(".TXT").trim_end_matches(".LOG") == nom));
    let Some(f) = f else {
        return l.dire(&format!("FICHIER INTROUVABLE : {a}."));
    };
    let niveau_dossier = DOSSIERS.iter().find(|(d, _)| *d == f.dossier).map_or(1, |(_, n)| *n);
    let requis = f.niveau.max(niveau_dossier);
    if requis > s.niveau {
        return l.dire(&format!("ACCÈS REFUSÉ : habilitation de niveau {requis} requise. {}", choix(REFUS)));
    }
    if f.dossier == "DIRECTION" && f.nom == "ROUGE.TXT" {
        l.succes("secret-etat");
    }
    l.dire(f.texte)
}
