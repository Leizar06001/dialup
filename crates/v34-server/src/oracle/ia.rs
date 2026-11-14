//! Requêtes à l'IA : API « chat completions » compatible OpenAI, chez
//! OpenRouter. Réponse d'un bloc (pas de flux) : l'oracle la vérifie et la
//! coupe proprement avant de la « taper » lui-même à la cadence du Minitel.
//!
//! Pour le cache des fournisseurs, chaque requête commence par le même
//! préfixe : la consigne [`super::persona::SYSTEME`], identique au caractère
//! près pour tous les abonnés, puis les échanges de l'appel dans l'ordre,
//! renvoyés tels quels, et la nouvelle question en dernier.

use std::fmt;
use std::io::{self, Read};
use std::time::Duration;

use serde::Serialize;

use super::persona::SYSTEME;

/// Routage « collant » d'OpenRouter : toutes les consultations vont au même
/// fournisseur tant qu'il répond, et le cache de la consigne reste chaud.
/// Constant : rien sur l'abonné.
pub const SESSION: &str = "v34-3615-oracle";
/// Taille maximale d'une réponse HTTP lue (octets)
const CORPS_MAX: u64 = 256 * 1024;
const TEMPERATURE: f32 = 0.9;

#[derive(Serialize)]
struct Message<'a> {
    role: &'a str,
    content: &'a str,
}

/// Pas de jetons de « réflexion » : forme unifiée d'OpenRouter (`enabled`)
/// et effort nul (forme des « chat completions »), les deux disent pareil.
#[derive(Serialize)]
struct Raisonnement {
    enabled: bool,
    effort: &'static str,
}

#[derive(Serialize)]
struct Requete<'a> {
    model: &'a str,
    messages: Vec<Message<'a>>,
    max_tokens: u32,
    temperature: f32,
    reasoning: Raisonnement,
    session_id: &'static str,
    stream: bool,
}

/// Corps JSON de la requête : consigne, échanges précédents (question,
/// réponse) tels qu'ils ont été envoyés et reçus, puis la question.
pub fn corps(modele: &str, jetons_max: u32, echanges: &[(String, String)], question: &str) -> String {
    let mut messages = Vec::with_capacity(2 + 2 * echanges.len());
    messages.push(Message { role: "system", content: SYSTEME });
    for (q, r) in echanges {
        messages.push(Message { role: "user", content: q });
        messages.push(Message { role: "assistant", content: r });
    }
    messages.push(Message { role: "user", content: question });
    let req = Requete {
        model: modele,
        messages,
        max_tokens: jetons_max,
        temperature: TEMPERATURE,
        reasoning: Raisonnement { enabled: false, effort: "none" },
        session_id: SESSION,
        stream: false,
    };
    serde_json::to_string(&req).expect("sérialisation d'une requête")
}

/// Jetons facturés (champ `usage` de la réponse).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub prompt: u64,
    /// dont lus dans le cache du fournisseur (`prompt_tokens_details.cached_tokens`)
    pub caches: u64,
    pub reponse: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Reponse {
    pub texte: String,
    /// Arrêtée par la limite de jetons (`finish_reason` = `length`)
    pub coupee: bool,
    pub usage: Option<Usage>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Erreur {
    /// Pas de réponse dans le délai
    Delai,
    /// Statut HTTP d'erreur (402 : plus de crédit, 429 : trop de requêtes…)
    Http(u16),
    /// Connexion impossible
    Reseau(String),
    /// Réponse illisible ou sans texte
    Invalide(&'static str),
}

impl fmt::Display for Erreur {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Erreur::Delai => f.write_str("délai dépassé"),
            Erreur::Http(c) => write!(f, "HTTP {c}"),
            Erreur::Reseau(e) => write!(f, "réseau : {e}"),
            Erreur::Invalide(e) => write!(f, "réponse invalide : {e}"),
        }
    }
}

/// Lit une réponse « chat completions ». Le champ `reasoning` éventuel est
/// ignoré : seul `content` compte.
pub fn lire(json: &[u8]) -> Result<Reponse, Erreur> {
    let v: serde_json::Value = serde_json::from_slice(json).map_err(|_| Erreur::Invalide("JSON"))?;
    if let Some(e) = v.get("error") {
        // OpenRouter peut répondre 200 avec une erreur du fournisseur
        let code = e.get("code").and_then(|c| c.as_u64()).and_then(|c| u16::try_from(c).ok());
        return Err(code.map_or(Erreur::Invalide("erreur du fournisseur"), Erreur::Http));
    }
    let choix = v.get("choices").and_then(|c| c.get(0)).ok_or(Erreur::Invalide("pas de choix"))?;
    let texte = choix.pointer("/message/content").and_then(|c| c.as_str()).unwrap_or("").trim().to_string();
    if texte.is_empty() {
        return Err(Erreur::Invalide("réponse vide"));
    }
    let coupee = choix.get("finish_reason").and_then(|f| f.as_str()) == Some("length");
    let usage = v.get("usage").map(|u| {
        let n = |p: &str| u.pointer(p).and_then(|x| x.as_u64()).unwrap_or(0);
        Usage { prompt: n("/prompt_tokens"), caches: n("/prompt_tokens_details/cached_tokens"), reponse: n("/completion_tokens") }
    });
    Ok(Reponse { texte, coupee, usage })
}

/// Client HTTP(S) de l'oracle (rustls, comme le web 1998).
pub fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .user_agent(concat!("v34-server/", env!("CARGO_PKG_VERSION"), " (3615 ORACLE)"))
        .max_idle_connections(4)
        .build()
}

/// Pose la question. La clé ne sert qu'à l'en-tête `Authorization`.
pub fn appeler(agent: &ureq::Agent, url: &str, cle: &str, corps: &str, delai: Duration) -> Result<Reponse, Erreur> {
    let resp = agent
        .post(url)
        .timeout(delai)
        .set("Authorization", &format!("Bearer {cle}"))
        .set("Content-Type", "application/json")
        .set("X-Title", "3615 ORACLE")
        .send_string(corps)
        .map_err(classer)?;
    let mut body = Vec::new();
    resp.into_reader().take(CORPS_MAX).read_to_end(&mut body).map_err(|e| match e.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => Erreur::Delai,
        _ => Erreur::Reseau("lecture interrompue".into()),
    })?;
    lire(&body)
}

fn classer(e: ureq::Error) -> Erreur {
    let t = match e {
        ureq::Error::Status(code, _) => return Erreur::Http(code),
        ureq::Error::Transport(t) => t,
    };
    let mut src: Option<&(dyn std::error::Error + 'static)> = Some(&t);
    while let Some(s) = src {
        if let Some(io) = s.downcast_ref::<io::Error>() {
            if matches!(io.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) {
                return Erreur::Delai;
            }
        }
        src = s.source();
    }
    Erreur::Reseau(t.kind().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requete_au_prefixe_stable() {
        let a = corps("m", 200, &[], "Qui suis-je ?");
        let b = corps("m", 200, &[("Qui suis-je ?".into(), "Un mortel.".into())], "Et toi ?");
        let va: serde_json::Value = serde_json::from_str(&a).unwrap();
        let vb: serde_json::Value = serde_json::from_str(&b).unwrap();
        assert_eq!(va["messages"][0], serde_json::json!({ "role": "system", "content": SYSTEME }));
        assert_eq!(va["messages"][0], vb["messages"][0]);
        assert_eq!(va["messages"][1], vb["messages"][1], "l'échange précédent est renvoyé tel quel");
        assert_eq!(vb["messages"][2], serde_json::json!({ "role": "assistant", "content": "Un mortel." }));
        assert_eq!(vb["messages"][3]["content"], "Et toi ?", "la nouvelle question en dernier");
        assert_eq!(va["max_tokens"], 200);
        assert_eq!(va["reasoning"]["enabled"], false);
        assert_eq!(va["reasoning"]["effort"], "none");
        assert_eq!(va["stream"], false);
        // le préfixe du corps est le même octet pour octet jusqu'à la fin de la consigne
        let fin = a.find("\"},").unwrap();
        assert_eq!(a[..fin], b[..fin]);
    }

    #[test]
    fn lecture_des_reponses() {
        let r = lire(br#"{"choices":[{"message":{"content":" Les astres disent oui. ","reasoning":"je pense que..."},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":812,"completion_tokens":57,"prompt_tokens_details":{"cached_tokens":768}}}"#)
        .unwrap();
        assert_eq!(r.texte, "Les astres disent oui.", "le raisonnement éventuel est ignoré");
        assert!(!r.coupee);
        assert_eq!(r.usage, Some(Usage { prompt: 812, caches: 768, reponse: 57 }));
        let r = lire(br#"{"choices":[{"message":{"content":"Je vois"},"finish_reason":"length"}],"usage":{"prompt_tokens":5,"completion_tokens":200}}"#).unwrap();
        assert!(r.coupee);
        assert_eq!(r.usage.unwrap().caches, 0, "pas de détail : rien en cache");
        assert_eq!(lire(br#"{"choices":[{"message":{"content":null,"reasoning":"..."}}]}"#), Err(Erreur::Invalide("réponse vide")));
        assert_eq!(lire(br#"{"error":{"code":402,"message":"Insufficient credits"}}"#), Err(Erreur::Http(402)));
        assert_eq!(lire(b"<html>"), Err(Erreur::Invalide("JSON")));
    }
}
