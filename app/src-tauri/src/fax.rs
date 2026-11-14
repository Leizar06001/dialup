//! Télécopie côté appli : pages empaquetées pour l'IHM (216 octets par
//! ligne de 1728 points, bit de poids fort à gauche, 1 = noir) et page
//! reçue enregistrée en PNG (noir et blanc, 1 bit par point).

use v34_core::fax::t4::LARGEUR;

/// Une ligne en octets (1 = noir).
pub fn empaqueter(ligne: &[bool]) -> Vec<u8> {
    let mut v = vec![0u8; LARGEUR / 8];
    for (i, &b) in ligne.iter().take(LARGEUR).enumerate() {
        if b {
            v[i / 8] |= 0x80 >> (i % 8);
        }
    }
    v
}

/// Les lignes d'une page empaquetée (`hauteur` lignes de 216 octets).
pub fn depaqueter(octets: &[u8], hauteur: usize) -> Vec<Vec<bool>> {
    octets.chunks(LARGEUR / 8).take(hauteur).map(|l| (0..LARGEUR).map(|i| l.get(i / 8).is_some_and(|o| o & (0x80 >> (i % 8)) != 0)).collect()).collect()
}

/// PNG noir et blanc de la page. En résolution standard (3,85 lignes/mm),
/// chaque ligne est doublée pour garder les proportions.
pub fn png(lignes: &[Vec<bool>], fine: bool) -> Vec<u8> {
    let rep = if fine { 1 } else { 2 };
    let hauteur = (lignes.len() * rep).max(1);
    let mut donnees = Vec::with_capacity(hauteur * LARGEUR / 8);
    for l in lignes {
        // en niveaux de gris sur 1 bit, 1 = blanc
        let o: Vec<u8> = empaqueter(l).iter().map(|b| !b).collect();
        for _ in 0..rep {
            donnees.extend_from_slice(&o);
        }
    }
    if lignes.is_empty() {
        donnees.resize(LARGEUR / 8, 0xFF);
    }
    let mut sortie = Vec::new();
    {
        let mut e = png::Encoder::new(&mut sortie, LARGEUR as u32, hauteur as u32);
        e.set_color(png::ColorType::Grayscale);
        e.set_depth(png::BitDepth::One);
        if let Ok(mut w) = e.write_header() {
            let _ = w.write_image_data(&donnees);
        }
    }
    sortie
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aller_retour_et_png() {
        let l: Vec<bool> = (0..LARGEUR).map(|i| i % 3 == 0).collect();
        let o = empaqueter(&l);
        assert_eq!(o.len(), 216);
        assert_eq!(depaqueter(&o, 1), vec![l.clone()]);
        let p = png(&[l.clone(), l], false);
        assert_eq!(&p[1..4], b"PNG");
    }
}
