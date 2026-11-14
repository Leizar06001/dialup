//! Tarifs de « Tinyverse Télécom », en francs de 1998 (jamais facturés).
//!
//! Partagés par l'appli (le compteur qui tourne pendant l'appel), le central
//! (les tickets de communication) et le site (la facture) : tout le monde
//! compte pareil.
//!
//! - **Communication locale** (abonné à abonné, FAI) : crédit temps de
//!   3 minutes pour 0,74 F, puis 0,28 F la minute en heures pleines et
//!   0,14 F en heures creuses (22 h – 8 h, samedi et dimanche, heure de
//!   Paris), à la seconde.
//! - **Numéro vert** (ligne de test) : gratuit.
//! - **Kiosque 08 36** (3615 ORACLE) : 2,23 F la minute dès la première
//!   seconde, heures creuses ou pas.
//!
//! Seul l'appelant paie, comme au téléphone.

/// Crédit temps de la communication locale (indivisible)
pub const CREDIT_TEMPS_S: u32 = 180;
/// Prix du crédit temps (centimes)
pub const CREDIT_TEMPS_CENTIMES: u32 = 74;
/// Communication locale au-delà du crédit temps, centimes par minute
pub const LOCAL_PLEIN: u32 = 28;
pub const LOCAL_CREUX: u32 = 14;
/// Kiosque, centimes par minute
pub const KIOSQUE: u32 = 223;
/// Parité fixée au 1er janvier 1999
pub const FRANCS_PAR_EURO: f64 = 6.55957;

/// Catégorie tarifaire d'un appel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Categorie {
    Local,
    Vert,
    Kiosque,
}

impl Categorie {
    /// D'après le service du central qui répond (`None` : un abonné).
    pub fn du_service(service: Option<&str>) -> Categorie {
        match service {
            Some("echo") => Categorie::Vert,
            Some("oracle") => Categorie::Kiosque,
            _ => Categorie::Local,
        }
    }

    /// D'après le service qui répond, et pour les autres services (numéros
    /// cachés) d'après le numéro : 0 800 gratuit, 08 36 au kiosque. Les
    /// abonnés (`None`) sont toujours en communication locale.
    pub fn de_l_appel(numero: &str, service: Option<&str>) -> Categorie {
        match service {
            None | Some("fai") => Categorie::Local,
            Some("echo" | "oracle") => Categorie::du_service(service),
            Some(_) if numero.starts_with("0800") => Categorie::Vert,
            Some(_) if numero.starts_with("0836") => Categorie::Kiosque,
            Some(_) => Categorie::Local,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Categorie::Local => "local",
            Categorie::Vert => "vert",
            Categorie::Kiosque => "kiosque",
        }
    }

    pub fn depuis_code(c: &str) -> Option<Categorie> {
        match c {
            "local" => Some(Categorie::Local),
            "vert" => Some(Categorie::Vert),
            "kiosque" => Some(Categorie::Kiosque),
            _ => None,
        }
    }

    pub fn libelle(self) -> &'static str {
        match self {
            Categorie::Local => "Communication locale",
            Categorie::Vert => "Numéro vert",
            Categorie::Kiosque => "Kiosque 08 36",
        }
    }
}

/// Coût d'un appel de `duree_s` secondes commencé à `debut` (secondes Unix),
/// en centimes.
pub fn cout_centimes(cat: Categorie, debut: u64, duree_s: u32) -> u32 {
    match cat {
        Categorie::Vert => 0,
        Categorie::Kiosque => ((duree_s as u64 * KIOSQUE as u64 + 30) / 60) as u32,
        Categorie::Local if duree_s == 0 => 0,
        Categorie::Local => {
            // Au-delà du crédit temps, tranche par tranche : les heures
            // creuses commencent et finissent toujours à l'heure pile.
            let mut c = CREDIT_TEMPS_CENTIMES as f64;
            let fin = debut + duree_s as u64;
            let mut t = debut + CREDIT_TEMPS_S as u64;
            while t < fin {
                let e = ((t / 3600 + 1) * 3600).min(fin);
                let tarif = if heures_creuses(t) { LOCAL_CREUX } else { LOCAL_PLEIN };
                c += (e - t) as f64 * tarif as f64 / 60.0;
                t = e;
            }
            c.round() as u32
        }
    }
}

/// Tarif à la minute en vigueur à l'instant `t` (centimes), pour l'affichage.
pub fn tarif_minute(cat: Categorie, t: u64) -> u32 {
    match cat {
        Categorie::Vert => 0,
        Categorie::Kiosque => KIOSQUE,
        Categorie::Local if heures_creuses(t) => LOCAL_CREUX,
        Categorie::Local => LOCAL_PLEIN,
    }
}

/// Heures creuses : 22 h – 8 h en semaine, samedi et dimanche entiers.
pub fn heures_creuses(t: u64) -> bool {
    let h = paris(t);
    h.jour_semaine == 0 || h.jour_semaine == 6 || h.heure >= 22 || h.heure < 8
}

/// « 1 234,56 F »
pub fn francs(centimes: u64) -> String {
    let f = centimes / 100;
    let mut ent = f.to_string();
    let mut i = ent.len();
    while i > 3 {
        i -= 3;
        ent.insert(i, '\u{202f}');
    }
    format!("{ent},{:02} F", centimes % 100)
}

/// « 1,88 € »
pub fn euros(centimes: u64) -> String {
    let e = (centimes as f64 / FRANCS_PAR_EURO).round() as u64;
    format!("{},{:02} €", e / 100, e % 100)
}

// ───────────────────────── heure de Paris ─────────────────────────

/// Date et heure de Paris (heure d'été comprise).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Heure {
    pub annee: i64,
    pub mois: u32,
    pub jour: u32,
    pub heure: u32,
    pub minute: u32,
    /// 0 = dimanche … 6 = samedi
    pub jour_semaine: u32,
}

impl Heure {
    /// Mois sous la forme AAAAMM (202611)
    pub fn cle_mois(&self) -> u32 {
        (self.annee as u32) * 100 + self.mois
    }
}

/// Mois de Paris de l'instant `t`, sous la forme AAAAMM.
pub fn mois(t: u64) -> u32 {
    paris(t).cle_mois()
}

/// Heure de Paris : UTC+1, UTC+2 du dernier dimanche de mars (1 h UTC) au
/// dernier dimanche d'octobre (1 h UTC).
pub fn paris(t: u64) -> Heure {
    let t = t as i64;
    let (annee, _, _) = civil(t.div_euclid(86_400));
    let ete = (dernier_dimanche(annee, 3) * 86_400 + 3600..dernier_dimanche(annee, 10) * 86_400 + 3600).contains(&t);
    let local = t + if ete { 7200 } else { 3600 };
    let jours = local.div_euclid(86_400);
    let s = local.rem_euclid(86_400);
    let (annee, mois, jour) = civil(jours);
    Heure { annee, mois, jour, heure: (s / 3600) as u32, minute: (s / 60 % 60) as u32, jour_semaine: (jours + 4).rem_euclid(7) as u32 }
}

/// Jours depuis le 1er janvier 1970 → (année, mois, jour) (H. Hinnant).
fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// (année, mois, jour) → jours depuis le 1er janvier 1970.
pub fn jours(annee: i64, mois: u32, jour: u32) -> i64 {
    let y = if mois <= 2 { annee - 1 } else { annee };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (mois as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + jour as i64 - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

/// Jour de la semaine d'une date (0 = dimanche … 6 = samedi).
pub fn jour_semaine(annee: i64, mois: u32, jour: u32) -> u32 {
    (jours(annee, mois, jour) + 4).rem_euclid(7) as u32
}

/// Dernier dimanche du mois (jours depuis 1970).
fn dernier_dimanche(annee: i64, mois: u32) -> i64 {
    let dernier = jours(annee, mois + 1, 1) - 1;
    dernier - (dernier + 4).rem_euclid(7)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heure_de_paris() {
        // 14 novembre 2026, 20:30 UTC = 21:30 à Paris (hiver), un samedi
        let h = paris(1_794_688_200);
        assert_eq!((h.annee, h.mois, h.jour, h.heure, h.minute, h.jour_semaine), (2026, 11, 14, 21, 30, 6));
        // 1er juillet 2026, 10:00 UTC = midi (été), un mercredi
        let h = paris(1_782_900_000);
        assert_eq!((h.mois, h.jour, h.heure, h.jour_semaine), (7, 1, 12, 3));
        // Passage à l'heure d'été : 29 mars 2026, 0:59 UTC = 1:59, 1:00 UTC = 3:00
        assert_eq!(paris(1_774_745_940).heure, 1);
        assert_eq!(paris(1_774_746_000).heure, 3);
        assert_eq!(mois(1_794_688_200), 202611);
    }

    #[test]
    fn communication_locale() {
        // Mardi 10 novembre 2026, 14:00 à Paris (13:00 UTC) : heures pleines
        let mardi = 1_794_315_600;
        assert!(!heures_creuses(mardi));
        assert_eq!(cout_centimes(Categorie::Local, mardi, 0), 0);
        assert_eq!(cout_centimes(Categorie::Local, mardi, 60), 74);
        assert_eq!(cout_centimes(Categorie::Local, mardi, 180), 74);
        // 10 minutes : 0,74 + 7 × 0,28
        assert_eq!(cout_centimes(Categorie::Local, mardi, 600), 74 + 7 * 28);
        // Commencé à 21:50 : 7 minutes pleines puis le reste en creuses
        let soir = mardi + 7 * 3600 + 50 * 60;
        assert_eq!(paris(soir).heure, 21);
        assert_eq!(cout_centimes(Categorie::Local, soir, 30 * 60), 74 + 7 * 28 + 20 * 14);
        // Le samedi, tout en heures creuses
        assert_eq!(cout_centimes(Categorie::Local, 1_794_688_200 - 9 * 3600, 600), 74 + 7 * 14);
    }

    #[test]
    fn kiosque_et_numero_vert() {
        assert_eq!(cout_centimes(Categorie::Kiosque, 0, 60), 223);
        assert_eq!(cout_centimes(Categorie::Kiosque, 0, 30), 112);
        assert_eq!(cout_centimes(Categorie::Vert, 0, 3600), 0);
        assert_eq!(Categorie::du_service(Some("oracle")), Categorie::Kiosque);
        assert_eq!(Categorie::du_service(None), Categorie::Local);
    }

    #[test]
    fn affichage() {
        assert_eq!(francs(1234), "12,34 F");
        assert_eq!(francs(123_456), "1\u{202f}234,56 F");
        assert_eq!(euros(656), "1,00 €");
    }
}
