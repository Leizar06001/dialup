//! 08 36 15 19 98 : « Le Prix en Francs », le jeu où l'on devine ce que
//! coûtaient les choses en 1998. Cinq objets, trois essais chacun (« c'est
//! plus », « c'est moins »), des points selon la précision ; le tableau
//! d'honneur garde le meilleur score de chaque abonné (`prix.json`).

use std::collections::BTreeMap;

use super::{hasard, modifier, Fin, Ligne};

const FICHIER: &str = "prix.json";
const MANCHES: usize = 5;
const ESSAIS: usize = 3;

/// Objets et prix (en francs, prix de l'époque à peu près : c'est un jeu).
pub const OBJETS: &[(&str, u32)] = &[
    ("une baguette de pain", 4),
    ("un timbre pour une lettre", 3),
    ("une place de cinéma pour Titanic", 45),
    ("un litre de super sans plomb", 6),
    ("un paquet de 10 disquettes 3 pouces 1/2", 59),
    ("un Tamagotchi", 150),
    ("la cassette vidéo du Roi Lion", 149),
    ("un baladeur à cassettes", 299),
    ("le maillot de l'équipe de France", 399),
    ("une Game Boy Color", 599),
    ("un modem 56K externe", 990),
    ("une PlayStation", 990),
    ("un téléphone portable avec forfait", 1290),
    ("un ordinateur Pentium II avec écran 15 pouces", 9990),
];

/// « 1 290 F », « 1290,50 », « 150 francs » → 1290, 1290, 150
pub fn lire_prix(s: &str) -> Option<u32> {
    let entier = s.split([',', '.']).next().unwrap_or("");
    let chiffres: String = entier.chars().filter(char::is_ascii_digit).collect();
    chiffres.parse().ok().filter(|&p: &u32| p < 1_000_000)
}

/// Points d'une manche : 100 pour le prix exact (150 du premier coup),
/// sinon selon l'écart de la meilleure proposition.
pub fn points(prix: u32, meilleure: u32, exact_du_premier_coup: bool) -> u32 {
    if meilleure == prix {
        return if exact_du_premier_coup { 150 } else { 100 };
    }
    let ecart = (meilleure as f64 - prix as f64).abs() / prix as f64;
    (90.0 * (1.0 - ecart)).max(0.0).round() as u32
}

pub fn lancer(l: &mut Ligne) -> Result<(), Fin> {
    l.dire(
        "★ LE PRIX EN FRANCS ★\n\
         Le jeu où l'on devine ce que coûtaient les choses cette année. Cinq objets, trois essais chacun : je vous dirai si c'est plus ou moins.\n\
         Tapez vos prix en francs, sans les centimes. C'est parti !",
    )?;
    let mut tirage: Vec<usize> = (0..OBJETS.len()).collect();
    for i in (1..tirage.len()).rev() {
        tirage.swap(i, hasard(i + 1));
    }
    let mut total = 0;
    for (manche, &k) in tirage.iter().take(MANCHES).enumerate() {
        let (objet, prix) = OBJETS[k];
        l.dire(&format!("Objet n° {} : combien coûte {objet} ?", manche + 1))?;
        let mut meilleure: Option<u32> = None;
        let mut essai = 0;
        let mut trouve_premier = false;
        while essai < ESSAIS {
            let s = l.attendre()?;
            let Some(p) = lire_prix(&s) else {
                l.dire("Un prix en francs, s'il vous plaît : des chiffres !")?;
                continue;
            };
            essai += 1;
            if meilleure.is_none_or(|m| p.abs_diff(prix) < m.abs_diff(prix)) {
                meilleure = Some(p);
            }
            if p == prix {
                trouve_premier = essai == 1;
                break;
            }
            if essai < ESSAIS {
                l.dire(if p < prix { "C'est plus !" } else { "C'est moins !" })?;
            }
        }
        let pts = points(prix, meilleure.unwrap_or(0), trouve_premier);
        total += pts;
        let verdict = if meilleure == Some(prix) { "LE PRIX EXACT !".to_string() } else { format!("Eh non : c'était {prix} F.") };
        l.dire(&format!("{verdict} {pts} point{}.", if pts > 1 { "s" } else { "" }))?;
    }
    // Tableau d'honneur
    let abonne = l.abonne.to_string();
    let (record, tableau) = modifier(l.donnees, FICHIER, |scores: &mut BTreeMap<String, u32>| {
        let ancien = scores.get(&abonne).copied().unwrap_or(0);
        if total > ancien {
            scores.insert(abonne.clone(), total);
        }
        let mut t: Vec<(String, u32)> = scores.iter().map(|(n, s)| (n.clone(), *s)).collect();
        t.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        (total > ancien, t)
    });
    let mut fin = format!("Score final : {total} points{}", if record { ". C'est votre record !" } else { "." });
    fin.push_str("\nTABLEAU D'HONNEUR");
    for (i, (n, s)) in tableau.iter().take(5).enumerate() {
        fin.push_str(&format!("\n{}. {n} : {s} points", i + 1));
    }
    fin.push_str("\nMerci d'avoir joué au Prix en Francs ! *générique* *clic*");
    l.dire(&fin)
}
