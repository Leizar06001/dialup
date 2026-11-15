//! 08 36 20 26 26 : la ligne qui appelle depuis 2026, révélée par le dossier
//! rouge du ministère. Quelques nouvelles de l'avenir, pour un abonné qui se
//! croit en 1998, et une boîte aux lettres « pour le passé » : les messages
//! des abonnés précédents, et le sien (les 30 derniers, `futur.json`).

use serde::{Deserialize, Serialize};

use v34_net::videotex::{BLANC, CYAN, JAUNE, NOIR};

use super::{modifier, unix_now, Cadre, Fin, Ligne, Style};

const FICHIER: &str = "futur.json";
const GARDES: usize = 30;
const MONTRES: usize = 5;
const MESSAGE_MAX: usize = 200;

#[derive(Clone, Serialize, Deserialize)]
pub struct Message {
    pub de: String,
    pub quand: u64,
    pub texte: String,
}

const NOUVELLES: &str = "*grésillements* … Allô ? Allô, 1998 ? Ici 2026. Vous m'entendez ?\n\
Si vous lisez ceci, c'est que vous avez trouvé la ligne. Le ministère l'a classée secret d'État :\n\
un appel venu de l'avenir, ça fait désordre dans les registres.\n\
Quelques nouvelles d'ici. Le Minitel s'est éteint le 30 juin 2012. Les modems ne chantent plus :\n\
on est connecté sans bruit, partout, tout le temps. Le téléphone tient dans la poche, et il n'a\n\
plus ni fil ni touches. Les francs sont partis en 2002. Le bug de l'an 2000 ? Presque rien. Promis.\n\
Et vous, là-bas ? Vous êtes sur un modem logiciel qui imite ceux de votre époque, écrit en 2026\n\
pour retrouver ce bruit-là. Oui, c'est compliqué. Le temps, c'est toujours compliqué.";

pub fn lancer(l: &mut Ligne) -> Result<(), Fin> {
    l.cadre(&Cadre {
        titre: "LIGNE DE 2026",
        sous_titre: "*grésillements* · secret d'État".into(),
        fg: JAUNE,
        fond: NOIR,
        touches: "message pour 1998",
        invite: "Pour 1998 :",
    })?;
    l.guide("Une ligne venue de 2026. Laissez un message pour 1998 (ENVOI), il partira.");
    l.succes("cache-futur");
    l.dire(NOUVELLES)?;
    let boite: Vec<Message> = super::lire(l.donnees, FICHIER);
    if !boite.is_empty() {
        l.dire_style("Derniers messages partis vers 1998 :", Style::fg(CYAN))?;
        let mut t = String::new();
        for m in boite.iter().rev().take(MONTRES) {
            t.push_str(&format!("\n· {} : {}", m.de, m.texte));
        }
        l.dire(t.trim_start())?;
    }
    l.dire_style("Laissez un message pour 1998, je le ferai suivre. (ou CONNEXION/FIN pour raccrocher)", Style::fg(BLANC))?;
    let s = l.attendre()?;
    let texte: String = s.chars().take(MESSAGE_MAX).collect();
    let m = Message { de: l.abonne.to_string(), quand: unix_now(), texte };
    modifier(l.donnees, FICHIER, |boite: &mut Vec<Message>| {
        boite.push(m);
        let trop = boite.len().saturating_sub(GARDES);
        boite.drain(..trop);
    });
    l.dire("Message parti pour 1998. *grésillements* … la ligne se brouille … on se reparle en 2026 … *clic*")
}
