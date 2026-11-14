//! Les succès des abonnés, partagés par le central (qui les tient à jour),
//! l'appli (qui les annonce, et signale ceux qu'elle seule peut constater)
//! et le site (qui les affiche dans l'espace abonné).
//!
//! Les succès « cachés » s'affichent « ??? » tant qu'on ne les a pas.

/// Qui constate le succès.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Le central, d'après les tickets de communication et les connexions
    Central,
    /// L'appli (débit, ligne, fichiers, visiophone…) : elle le signale au
    /// central, qui le note
    Appli,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct Succes {
    pub id: &'static str,
    pub titre: &'static str,
    pub description: &'static str,
    pub cache: bool,
    pub source: Source,
}

const fn c(id: &'static str, titre: &'static str, description: &'static str) -> Succes {
    Succes { id, titre, description, cache: false, source: Source::Central }
}
const fn a(id: &'static str, titre: &'static str, description: &'static str) -> Succes {
    Succes { id, titre, description, cache: false, source: Source::Appli }
}
const fn cache(s: Succes) -> Succes {
    Succes { cache: true, ..s }
}

/// Seuils des succès du central
pub const SOCIABLE: usize = 3;
pub const STANDARDISTE: usize = 5;
pub const FIDELE_JOURS: usize = 7;
pub const MARATHON_S: u32 = 3600;
pub const ACCRO_S: u64 = 10 * 3600;
pub const DISCIPLE_S: u64 = 30 * 60;
pub const TEMPETE_S: f32 = 600.0;
pub const GROS_COLIS: u64 = 1 << 20;
pub const TENACE_RETRAINS: u32 = 3;

pub const LISTE: &[Succes] = &[
    c("premier-appel", "Allô ?", "Premier appel abouti."),
    c("recu", "Ça sonne !", "Recevoir l'appel d'un abonné."),
    c("fai", "Bienvenue sur le Net", "Première connexion au FAI TINYVERSE."),
    c("oracle", "Consultation", "Premier appel à 3615 ORACLE."),
    c("test", "Un, deux, un, deux", "Appeler la ligne de test (numéro vert)."),
    c("sociable", "Carnet d'adresses", "Appeler 3 abonnés différents."),
    c("standardiste", "Standardiste", "Appeler 5 abonnés différents."),
    c("marathon", "Marathon", "Un appel d'une heure ou plus."),
    cache(c("nuit-blanche", "Nuit blanche", "Être en ligne à 3 h du matin.")),
    c("fidele", "Abonné fidèle", "Se connecter au central 7 jours différents."),
    c("accro", "Accro", "10 heures en ligne au total."),
    c("facture-10", "Premier relevé", "Une facture de 10 F dans le mois."),
    c("facture-100", "Gros consommateur", "Une facture de 100 F dans le mois."),
    cache(c("facture-1000", "Ruine", "Une facture de 1 000 F dans le mois. Heureusement, personne ne paie.")),
    cache(c("disciple", "Disciple", "30 minutes au total avec l'oracle.")),
    a("v34", "Plein pot", "CONNECT à 33 600 bit/s en V.34."),
    a("v90", "56K", "CONNECT en 56K (V.90)."),
    a("tempete", "Dans la tempête", "10 minutes en ligne sur la ligne Orage."),
    a("fichier", "Facteur", "Envoyer un fichier à un abonné."),
    a("gros-colis", "Colis encombrant", "Envoyer un fichier de 1 Mo ou plus."),
    a("visio", "Souriez !", "Un appel en visiophone, chacun sa caméra."),
    cache(a("tenace", "Tenace", "Rester en ligne malgré 3 réentraînements dans le même appel.")),
];

pub fn trouver(id: &str) -> Option<&'static Succes> {
    LISTE.iter().find(|s| s.id == id)
}

/// Identifiant acceptable sur le réseau (1 à 32 caractères a-z, 0-9, -).
pub fn id_valide(id: &str) -> bool {
    (1..=32).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiants_uniques_et_valides() {
        for (i, s) in LISTE.iter().enumerate() {
            assert!(id_valide(s.id), "{}", s.id);
            assert!(LISTE[i + 1..].iter().all(|t| t.id != s.id), "doublon {}", s.id);
        }
        assert!(trouver("v90").is_some_and(|s| s.source == Source::Appli));
        assert!(!id_valide("Majuscule"));
    }
}
