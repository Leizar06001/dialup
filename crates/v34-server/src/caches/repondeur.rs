//! 01 43 33 12 12 : le répondeur de Gérard, abonné parti sans résilier.
//! On y laisse un message après le bip, et on peut écouter la bande : les
//! messages des autres abonnés (les 50 derniers, dans `repondeur.json`).

use serde::{Deserialize, Serialize};

use super::{modifier, unix_now, Fin, Ligne};

const FICHIER: &str = "repondeur.json";
const GARDES: usize = 50;
const ECOUTES: usize = 10;
const MESSAGE_MAX: usize = 200;

#[derive(Clone, Serialize, Deserialize)]
pub struct Message {
    pub de: String,
    pub quand: u64,
    pub texte: String,
}

fn ecouter(l: &mut Ligne) -> Result<(), Fin> {
    let bande: Vec<Message> = super::lire(l.donnees, FICHIER);
    if bande.is_empty() {
        return l.dire("*bzzz* La bande est vierge. Gérard n'a pas d'amis. Pas encore.");
    }
    let debut = bande.len().saturating_sub(ECOUTES);
    let mut t = format!("*rembobinage* {} message{} sur la bande. Les plus récents :", bande.len(), if bande.len() > 1 { "s" } else { "" });
    for (k, m) in bande[debut..].iter().enumerate() {
        let h = v34_net::tarif::paris(m.quand);
        t.push_str(&format!("\nMessage {} · le {:02}/{:02} à {} h {:02}, de {} : {}", debut + k + 1, h.jour, h.mois, h.heure, h.minute, m.de, m.texte));
    }
    t.push_str("\nFin des messages. *bip*");
    l.dire(&t)
}

pub fn lancer(l: &mut Ligne) -> Result<(), Fin> {
    let n = super::lire::<Vec<Message>>(l.donnees, FICHIER).len();
    l.dire(&format!(
        "*clic* Bonjour, vous êtes bien chez Gérard. Je ne suis pas là pour le moment : sûrement parti chercher du pain, ou alors je suis sous la douche. Laissez-moi un message après le bip, je vous rappellerai. Peut-être.\n\
         BIIIIIIP\n\
         (La bande contient {n} message{}. Tapez le vôtre, ou /ecouter pour écouter la bande : Gérard ne l'a jamais fait.)",
        if n > 1 { "s" } else { "" }
    ))?;
    loop {
        let s = l.attendre()?;
        if s.to_lowercase().trim_start_matches('/').starts_with("ecout") || s.to_lowercase().trim_start_matches('/').starts_with("écout") {
            ecouter(l)?;
            l.dire("Votre message après le bip, ou /fin pour raccrocher. BIIIP")?;
            continue;
        }
        let texte: String = s.chars().take(MESSAGE_MAX).collect();
        let m = Message { de: l.abonne.to_string(), quand: unix_now(), texte };
        modifier(l.donnees, FICHIER, |bande: &mut Vec<Message>| {
            bande.push(m);
            let trop = bande.len().saturating_sub(GARDES);
            bande.drain(..trop);
        });
        l.dire("*bip* Message enregistré. Gérard l'écoutera… un jour. *clic*")?;
        return Ok(());
    }
}
