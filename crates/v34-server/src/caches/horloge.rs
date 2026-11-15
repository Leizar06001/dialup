//! 08 36 99 36 99 : l'horloge parlante du réseau. Elle donne l'heure de
//! Paris, toutes les dix secondes, au quatrième top… et la date, en 1998.
//! Entre deux annonces, des « messages de nos partenaires » : les indices
//! vers d'autres numéros cachés.
//!
//! Sur le Minitel : l'heure en grands chiffres (double taille) en haut de la
//! page, l'annonce et les tops qui s'égrènent en dessous.

use std::time::Duration;

use v34_net::videotex::{BLANC, BLEU, JAUNE};

use super::{choix, date_1998, unix_now, Cadre, Fin, Ligne, Style, Taille};

const VARIANTES: &[&str] = &[
    "",
    "",
    "",
    " environ",
    ", à peu près",
    ", heure de Paris, enfin presque",
    ", sauf erreur de l'horloge",
    ", plus ou moins un top",
];

const PARTENAIRES: &[&str] = &[
    "Message de nos partenaires : un ciel chargé sur vos lignes ? Écoutez la MÉTÉO DES LIGNES au 08 36 68 68 68.",
    "Message de nos partenaires : l'an 2000 approche. Votre modem est-il prêt ? Renseignez-vous gratuitement au 0 800 20 00 00.",
    "L'horloge parlante vous est offerte par le Ministère des Postes. Ses agents sont joignables au 01 45 78 19 98 (réservé aux agents).",
];

const MESSAGES: &[&str] = &[
    "L'horloge ne prend pas les messages : elle donne l'heure, c'est tout.",
    "Inutile d'insister, l'horloge est une machine. Une machine très ponctuelle.",
    "L'horloge vous a entendu. Elle s'en moque : elle compte.",
];

/// Le grand affichage (double taille : rangées 6 et 7).
const RANGEE_HEURE: u8 = 7;

fn heure(h: u32, m: u32, s: u32) -> String {
    let pl = |n: u32, mot: &str| format!("{n} {mot}{}", if n > 1 { "s" } else { "" });
    format!("{}, {} et {}", pl(h, "heure"), pl(m, "minute"), pl(s, "seconde"))
}

pub fn lancer(l: &mut Ligne) -> Result<(), Fin> {
    l.cadre(&Cadre {
        titre: "HORLOGE PARLANTE",
        sous_titre: "du réseau Tinyverse · heure de Paris".into(),
        fg: JAUNE,
        fond: BLEU,
        touches: "parler à l'horloge (inutile)",
        invite: ">",
    })?;
    l.guide("L'horloge donne l'heure toutes les dix secondes, au quatrième top. Elle ne prend pas les messages. CONNEXION/FIN pour raccrocher.");
    // l'horloge ne s'arrête pas pour qu'on lise : sa zone défile sans attendre SUITE
    l.ecran.attente_suite = Duration::ZERO;
    l.ecran.zone(9, 20, Style::NORMAL);
    // (une rangée de 40 colonnes : la date seule, encadrée)
    l.ecran.centrer(5, Style::fg(BLANC), &format!("- {} -", date_1998(unix_now())))?;
    l.dire(&format!("Nous sommes le {}.", date_1998(unix_now())))?;
    let mut n = 0usize;
    loop {
        // Le prochain multiple de dix secondes, au moins cinq secondes plus tard
        let cible = (unix_now() + 5) / 10 * 10 + 10;
        let h = v34_net::tarif::paris(cible);
        l.dire(&format!("Au quatrième top, il sera exactement {}{}.", heure(h.heure, h.minute, (cible % 60) as u32), choix(VARIANTES)))?;
        for k in (0..4).rev() {
            let t = cible - k;
            let now = unix_now();
            if t > now {
                // les lignes tapées entre-temps attendent : on y répond après
                l.pause(Duration::from_secs(t - now))?;
            }
            if k == 0 {
                l.suite(" TOP !")?;
                let affiche = format!("{:02}:{:02}:{:02}", h.heure, h.minute, cible % 60);
                l.ecran.centrer(RANGEE_HEURE, Style::fg(JAUNE).taille(Taille::Double), &affiche)?;
            } else if k == 3 {
                l.dire("top…")?;
            } else {
                l.suite(" top…")?;
            }
        }
        n += 1;
        if n % 3 == 0 {
            l.dire(PARTENAIRES[(n / 3 - 1) % PARTENAIRES.len()])?;
        }
        if l.lire(Duration::ZERO)?.is_some() {
            l.dire(choix(MESSAGES))?;
        }
    }
}
