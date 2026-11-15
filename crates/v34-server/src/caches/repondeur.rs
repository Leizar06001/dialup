//! 01 43 33 12 12 : le répondeur de Gérard, abonné parti sans résilier.
//! On y laisse un message après le bip, et on peut écouter la bande : les
//! messages des autres abonnés (les 50 derniers, dans `repondeur.json`),
//! après un vieux message de 1997 que le chef de service n'aurait jamais dû
//! laisser (le mot de passe du ministère, niveau 2).

use serde::{Deserialize, Serialize};

use v34_net::videotex::{Touche, BLANC, JAUNE, ROUGE};

use super::{modifier, unix_now, Cadre, Fin, Ligne, Style};

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

/// Le tout premier message de la bande, jamais effacé.
pub const MESSAGE_DU_CHEF: &str = "Message 0 · le 03/02/1997 à 9 h 12, du chef : « Gérard, c'est le chef. J'ai encore changé mon mot de passe au ministère : c'est le nom de mon chat, Minouche. Notez-le quelque part, moi je vais l'oublier. »";

fn ecouter(l: &mut Ligne) -> Result<(), Fin> {
    let bande: Vec<Message> = super::lire(l.donnees, FICHIER);
    let mut t = format!("*rembobinage* {} message{} sur la bande.\n{MESSAGE_DU_CHEF}", bande.len() + 1, if bande.is_empty() { "" } else { "s" });
    if bande.is_empty() {
        t.push_str("\nFin des messages. *bip*");
        return l.dire(&t);
    }
    let debut = bande.len().saturating_sub(ECOUTES);
    t.push_str("\nLes plus récents :");
    for (k, m) in bande[debut..].iter().enumerate() {
        let h = v34_net::tarif::paris(m.quand);
        t.push_str(&format!("\nMessage {} · le {:02}/{:02} à {} h {:02}, de {} : {}", debut + k + 1, h.jour, h.mois, h.heure, h.minute, m.de, m.texte));
    }
    t.push_str("\nFin des messages. *bip*");
    l.dire(&t)
}

pub fn lancer(l: &mut Ligne) -> Result<(), Fin> {
    l.cadre(&Cadre {
        titre: "REPONDEUR DE GERARD",
        sous_titre: "01 43 33 12 12 · laissez un message".into(),
        fg: BLANC,
        fond: ROUGE,
        touches: "message   SUITE : la bande",
        invite: "Message :",
    })?;
    l.touches(&[(Touche::Suite, "/ecouter"), (Touche::Repetition, "/ecouter")]);
    l.guide("Le répondeur de Gérard enregistre votre message (ENVOI). SUITE : écouter la bande. CONNEXION/FIN : raccrocher.");
    // (plus le vieux message du chef, toujours là)
    let n = super::lire::<Vec<Message>>(l.donnees, FICHIER).len() + 1;
    l.dire("*clic* Bonjour, vous êtes bien chez Gérard. Je ne suis pas là pour le moment : sûrement parti chercher du pain, ou alors je suis sous la douche. Laissez-moi un message après le bip, je vous rappellerai. Peut-être.")?;
    l.dire_style("BIIIIIIP", Style::fg(JAUNE).clignotant())?;
    l.ecran.bip()?;
    l.dire(&format!(
        "(La bande contient {n} message{}. Tapez le vôtre, puis ENVOI. SUITE pour écouter la bande : Gérard ne l'a jamais fait.)",
        if n > 1 { "s" } else { "" }
    ))?;
    loop {
        let s = l.attendre()?;
        if s.to_lowercase().trim_start_matches('/').starts_with("ecout") || s.to_lowercase().trim_start_matches('/').starts_with("écout") {
            ecouter(l)?;
            l.dire("Votre message après le bip, ou CONNEXION/FIN pour raccrocher. BIIIP")?;
            l.ecran.bip()?;
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
