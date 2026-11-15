//! 08 36 68 68 68 : la météo des lignes. Le bulletin du jour (le même toute
//! la journée), d'après l'état réel du réseau publié par le central
//! (`etat.json`) : abonnés connectés, communications en cours, salon du FAI.
//! RÉPÉTITION (ou « /bulletin ») le relit.

use std::time::Duration;

use serde::Deserialize;

use v34_net::videotex::{Touche, CYAN, JAUNE, NOIR};

use super::{choix, unix_now, Cadre, Fin, Ligne, Style};

#[derive(Default, Deserialize)]
#[serde(default)]
struct Etat {
    connectes: u64,
    appels: u64,
    salon: u64,
}

const SITUATIONS: &[&str] = &[
    "un anticyclone de bande passante s'étend sur l'ensemble du réseau",
    "une dépression venue du 3615 s'approche lentement des centraux",
    "un front de parasites traverse les lignes du nord au sud",
    "le baromètre des modems reste au beau fixe, porté par les vents du 56K",
    "une masse d'air humide fait gonfler les fils de cuivre",
];
const PARFAITE: &[&str] = &["grand soleil, rapport signal sur bruit au beau fixe", "ciel limpide, pas un craquement à l'horizon", "beau temps, idéal pour un CONNECT 56000"];
const URBAINE: &[&str] = &["nuageux, quelques craquements en soirée", "éclaircies entre deux bus, rafales de diaphonie aux heures de pointe", "brumes matinales, débit en légère baisse"];
const CAMPAGNE: &[&str] = &["temps couvert, ligne longue et un peu sourde", "vent d'ouest dans les poteaux, aigus en berne", "averses de souffle, les vaches regardent passer les octets"];
const ORAGE: &[&str] = &["rafales de clics jusqu'à quarante par seconde", "coupures de porteuse possibles, foudre sur le répartiteur", "houle de bruit impulsif, réentraînements fréquents"];

fn bulletin(l: &mut Ligne, t: u64) -> Result<(), Fin> {
    let h = v34_net::tarif::paris(t);
    let e: Etat = l.donnees.and_then(|d| std::fs::read(d.join("etat.json")).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    // Un bulletin par jour : le tirage dépend de la date
    let jour = (h.cle_mois() * 100 + h.jour) as usize;
    let tire = |l: &[&'static str], k: usize| l[(jour / 7 + k * 13) % l.len()];
    let pl = |n: u64, mot: &str| format!("{n} {mot}{}", if n > 1 { "s" } else { "" });
    let mer = match e.salon {
        0 => "mer d'huile, personne à l'horizon".to_string(),
        n => format!("mer agitée, {} au large", pl(n, "bavard")),
    };
    l.ecran.vider_zone()?;
    l.dire_style(&format!("Bulletin de {} h {:02}", h.heure, h.minute), Style::fg(JAUNE))?;
    l.dire(&format!(
        "Situation générale : {}. On compte {} sur le réseau et {} en cours.\n\
         Ligne Parfaite : {}.\n\
         Ligne Urbaine : {}.\n\
         Ligne Campagne : {}.\n\
         Ligne Orage : avis de grand frais ! {}.\n\
         Salon du FAI : {mer}.\n\
         Prochain bulletin à {} h. Ce bulletin vous est offert par le Ministère des Postes.",
        tire(SITUATIONS, 0),
        pl(e.connectes, "abonné"),
        pl(e.appels, "communication"),
        tire(PARFAITE, 1),
        tire(URBAINE, 2),
        tire(CAMPAGNE, 3),
        tire(ORAGE, 4),
        (h.heure + 1) % 24,
    ))?;
    l.dire_style("RÉPÉTITION : relire le bulletin. CONNEXION/FIN : raccrocher.", Style::fg(CYAN))
}

pub fn lancer(l: &mut Ligne) -> Result<(), Fin> {
    l.cadre(&Cadre {
        titre: "METEO DES LIGNES",
        sous_titre: "le bulletin du réseau, offert par le Ministère".into(),
        fg: NOIR,
        fond: CYAN,
        touches: "RÉPÉTITION : le bulletin",
        invite: ">",
    })?;
    l.touches(&[(Touche::Repetition, "/bulletin"), (Touche::Sommaire, "/bulletin"), (Touche::Suite, "/bulletin")]);
    l.guide("La météo des lignes donne l'état du réseau, une fois par jour. RÉPÉTITION pour relire le bulletin.");
    bulletin(l, unix_now())?;
    loop {
        if let Some(s) = l.lire(Duration::from_secs(60))? {
            if s.to_lowercase().trim_start_matches('/').starts_with("bulletin") {
                bulletin(l, unix_now())?;
            } else {
                l.dire(choix(&["La météo ne répond pas aux questions : elle prévoit.", "Pour toute réclamation, adressez-vous au Ministère. La météo, elle, ne fait que constater."]))?;
            }
        }
    }
}
