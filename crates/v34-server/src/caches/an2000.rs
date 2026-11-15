//! 0 800 20 00 00 : la Commission nationale de préparation à l'an 2000.
//! Numéro vert. Compte à rebours (depuis « aujourd'hui, en 1998 »),
//! conseils, et le prophète. Indice : le mot de passe du Ministère.

use v34_net::videotex::{Touche, JAUNE, NOIR, VERT};

use super::{aujourd_hui_1998, date_1998, unix_now, Cadre, Fin, Ligne, Style};

const MENU: &str = "1 - Mon modem est-il prêt pour l'an 2000 ?\n\
2 - Que se passera-t-il le 1er janvier 2000 à minuit ?\n\
3 - Les conseils de la Commission\n\
4 - Le prophète\n\
Tapez un chiffre, puis ENVOI. SOMMAIRE : ce menu. CONNEXION/FIN : raccrocher.";

const REPONSES: [&str; 4] = [
    "Votre modem compte les années sur deux chiffres, comme tout le monde. Le 1er janvier 2000, il se croira en 1900 et refusera de se connecter tant que le téléphone n'aura pas été inventé. La Commission recommande de le débrancher le 31 décembre à 23 h 59, et de le rebrancher… plus tard.",
    "Les experts hésitent entre trois scénarios : rien du tout, la fin de la civilisation, ou les deux. Les distributeurs de billets pourraient rendre des francs de 1900. Les magnétoscopes clignoteront 00:00, mais c'est déjà le cas.",
    "Faites des réserves de piles, de bougies et de disquettes. Imprimez vos courriels importants. Évitez l'ascenseur le 1er janvier entre minuit et minuit cinq. Et surtout, ne dites jamais « bug » devant un fonctionnaire : le terme officiel, recommandé par la Commission de terminologie, est « bogue ».",
    "Le prophète prend la ligne : « Mes frères, l'an 2000 approche ! Les ordinateurs se réveilleront en 1900, les fax enverront des télégrammes, les Minitels parleront latin ! Repentez-vous, et sauvegardez sur disquette ! » La Commission décline toute responsabilité quant aux propos du prophète.",
];

pub fn lancer(l: &mut Ligne) -> Result<(), Fin> {
    let t = unix_now();
    let (_, j, m) = aujourd_hui_1998(t);
    let reste = v34_net::tarif::jours(2000, 1, 1) - v34_net::tarif::jours(1998, m, j);
    l.cadre(&Cadre {
        titre: "COMMISSION AN 2000",
        sous_titre: "Numéro vert : appel gratuit d'un poste fixe".into(),
        fg: NOIR,
        fond: VERT,
        touches: "SOMMAIRE : le menu",
        invite: "Votre choix :",
    })?;
    l.touches(&[(Touche::Sommaire, "menu"), (Touche::Retour, "menu")]);
    l.guide("La Commission nationale de préparation au passage à l'an 2000 répond à vos questions. Tapez un chiffre de 1 à 4, puis ENVOI.");
    l.dire_style("COMMISSION NATIONALE DE PRÉPARATION AU PASSAGE À L'AN 2000", Style::fg(JAUNE))?;
    l.dire(&format!("Nous sommes le {}. Plus que {reste} jours avant l'an 2000.\n{MENU}", date_1998(t)))?;
    loop {
        let s = l.attendre()?;
        match s.trim().chars().next() {
            Some(c @ '1'..='4') => {
                l.ecran.vider_zone()?;
                l.dire(REPONSES[(c as u8 - b'1') as usize])?;
                l.dire_style("Autre chose ? Tapez un chiffre de 1 à 4.", Style::fg(JAUNE))?;
            }
            _ if s == "menu" => {
                l.ecran.vider_zone()?;
                l.dire(MENU)?;
            }
            _ => {
                l.ecran.vider_zone()?;
                l.dire(&format!("La Commission n'a pas compris.\n{MENU}"))?;
            }
        }
    }
}
