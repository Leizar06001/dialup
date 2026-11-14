//! 01 45 78 19 98 : le serveur du Ministère des Postes, service des Lignes
//! Perdues. Réservé aux agents : il faut le mot de passe, le terme officiel
//! pour « bug » (la Commission de l'an 2000 le souffle). Dedans, le registre
//! des numéros non attribués : la liste des autres numéros cachés.

use super::{choix, hasard, Fin, Ligne};

pub const MOT_DE_PASSE: &str = "bogue";

const REGISTRE: &str = "REGISTRE DES NUMÉROS NON ATTRIBUÉS (confidentiel)\n\
01 43 33 12 12 : ligne de M. Gérard, abonné parti sans résilier. Le répondeur tourne toujours.\n\
08 36 15 19 98 : jeu « Le Prix en Francs », dossier d'agrément en cours depuis 1987.\n\
08 36 99 36 99 : horloge parlante. Fonctionne. Personne ne sait pourquoi.\n\
08 36 68 68 68 : météo des lignes. Prévisions non contractuelles.\n\
0 800 20 00 00 : Commission de l'an 2000. Dissolution prévue le 2 janvier 2000.\n\
Fin du registre. Toute copie sera tamponnée.";

const NOTES: &[&str] = &[
    "Note de service n° 98-1203 : il est rappelé aux agents que le Minitel du bureau n'est pas une console de jeu.",
    "Note de service n° 98-1207 : les trombones sont désormais attribués sur formulaire, en trois exemplaires.",
    "Note de service n° 98-1211 : la machine à café est en panne. Une commission d'enquête a été nommée.",
    "Note de service n° 98-1214 : suite au passage à l'an 2000, les dossiers de 1900 seront rouverts. Prévoyez des chaises.",
];

const MENU: &str = "1 - Registre des numéros non attribués\n\
2 - Déposer une réclamation (formulaire Cerfa n° 1998-56K)\n\
3 - Note de service du jour\n\
/fin pour raccrocher.";

/// « Bogue », « BOGUE », « bogue. »… : le bon mot de passe ?
pub fn mot_de_passe_ok(s: &str) -> bool {
    let t: String = s.trim().trim_end_matches(['.', '!']).to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
    t == MOT_DE_PASSE
}

pub fn lancer(l: &mut Ligne) -> Result<(), Fin> {
    l.dire(
        "MINISTÈRE DES POSTES ET TÉLÉCOMMUNICATIONS\n\
         Service des Lignes Perdues · serveur réservé aux agents\n\
         Toute tentative d'accès non autorisée sera tamponnée en trois exemplaires.\n\
         IDENTIFIANT ?",
    )?;
    let agent = l.attendre()?.to_uppercase();
    for essai in 1..=3 {
        l.dire("MOT DE PASSE ? (il s'affiche en clair : c'est normal, c'est le ministère)")?;
        if mot_de_passe_ok(&l.attendre()?) {
            break;
        }
        if essai == 3 {
            l.dire("ACCÈS REFUSÉ. Votre dossier a été transmis au service compétent, qui vous répondra sous 6 à 8 semaines. Ne quittez pas… *clic*")?;
            return Ok(());
        }
        l.dire(&format!("MOT DE PASSE INCORRECT (tentative {essai} sur 3)."))?;
        if essai == 2 {
            l.dire("Rappel du service informatique : le mot de passe est le terme officiel, en bon français, d'un défaut informatique dont tout le monde parle en ce moment.")?;
        }
    }
    l.dire(&format!("BIENVENUE, AGENT {agent}. Vous avez {} dossiers en retard.\n{MENU}", 1000 + hasard(9000)))?;
    loop {
        let s = l.attendre()?;
        match s.trim().chars().next() {
            Some('1') => l.dire(REGISTRE)?,
            Some('2') => {
                l.dire("OBJET DE LA RÉCLAMATION ?")?;
                l.attendre()?;
                l.dire(&format!(
                    "Votre réclamation n° {:05}/98 a été enregistrée. Délai de traitement estimé : 1 998 jours ouvrés. Merci de ne pas rappeler.",
                    hasard(100_000)
                ))?;
            }
            Some('3') => l.dire(choix(NOTES))?,
            _ => {
                l.dire(&format!("Commande non réglementaire.\n{MENU}"))?;
                continue;
            }
        }
        l.dire("Autre chose, agent ? (1, 2, 3 ou /fin)")?;
    }
}
