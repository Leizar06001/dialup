//! 08 36 99 36 99 : l'horloge parlante du réseau. Elle donne l'heure de
//! Paris, toutes les dix secondes, au quatrième top… et la date, en 1998.
//! Entre deux annonces, des « messages de nos partenaires » : les indices
//! vers d'autres numéros cachés.

use std::time::Duration;

use super::{choix, date_1998, unix_now, Fin, Ligne};

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

fn heure(h: u32, m: u32, s: u32) -> String {
    let pl = |n: u32, mot: &str| format!("{n} {mot}{}", if n > 1 { "s" } else { "" });
    format!("{}, {} et {}", pl(h, "heure"), pl(m, "minute"), pl(s, "seconde"))
}

pub fn lancer(l: &mut Ligne) -> Result<(), Fin> {
    l.dire("HORLOGE PARLANTE DU RÉSEAU TINYVERSE")?;
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
