//! L'organiste : un poste complet (émetteur, récepteur, annuleur d'écho)
//! qui suit le protocole d'appel.
//!
//! ```text
//! appelant : [motif A]─────────silence──────────[mélodie (données) ...
//! appelé   :   (écoute) ─écart─[motif B]─2·Rmax─[mélodie (données) ...
//! ```
//! Chacun se tait pendant le motif de l'autre : l'annuleur d'écho apprend
//! sur son propre motif sans être gêné. Les données d'en face commencent à
//! un instant connu d'après son motif (aucune autre synchronisation).

use super::chargement::Chargement;
use super::emetteur::Emetteur;
use super::partition::{debut_motif, duree_motif, note_du_motif, EXTINCTION, MOTIF};
use super::recepteur::{Etat, Recepteur};
use super::{decalage_donnees, Duplex, Mode, Voix, ECART, RETARD_MAX};
use crate::dsp::echo::EchoCanceller;

/// À la fin du motif, le pas de l'annuleur est descendu à ce facteur.
const FIN_DE_PAS: f32 = 0.1;

/// Pas de l'annuleur pendant notre motif (`t` depuis son début) : plein
/// pendant les notes seules (il apprend vite), puis décroissant
/// exponentiellement pendant les accords agités jusqu'à `mu × FIN_DE_PAS`
/// (il moyenne le bruit de ligne qui, sinon, le désajuste).
pub fn pas_annuleur(mu: f32, t: usize, duree: usize) -> f32 {
    let accords = (0..MOTIF.len()).find(|&i| note_du_motif(i).is_none()).map(debut_motif).unwrap_or(duree);
    if t < accords {
        mu
    } else if t < duree {
        mu * FIN_DE_PAS.powf((t - accords) as f32 / (duree - accords) as f32)
    } else if t < duree + EXTINCTION {
        mu * FIN_DE_PAS
    } else {
        0.0
    }
}

pub struct Organiste {
    pub voix: Voix,
    pub tx: Emetteur,
    pub rx: Recepteur,
    pub ec: EchoCanceller,
    /// Temps absolu (échantillons traités)
    pub t: usize,
    /// Notre motif : début, et début de nos données
    motif: Option<usize>,
    donnees: Option<usize>,
    /// Pas de l'annuleur pendant que l'autre se tait / pendant le duplex
    pub mu_rapide: f32,
    pub mu_lent: f32,
    /// Atténuation d'écho obtenue à la fin de notre motif (dB)
    pub erle_motif: Option<f32>,
    /// L'appelé n'a pas pu caler à temps pour répondre
    pub echec: bool,
    bloc: Vec<f32>,
    /// Puissance glissante de l'erreur de l'annuleur, et gel après un clic
    puissance_erreur: f32,
    gel: usize,
}

impl Organiste {
    pub fn new(voix: Voix, duplex: Duplex, mode: Mode) -> Self {
        let mut tx = Emetteur::new(voix, duplex, mode);
        let mut rx = Recepteur::new(voix.autre(), duplex, mode);
        let (mut motif, mut donnees) = (None, None);
        if voix == Voix::Appelant {
            tx.programmer_motif(0);
            let d = duree_motif() + decalage_donnees(Voix::Appelant);
            tx.programmer_donnees(d);
            motif = Some(0);
            donnees = Some(d);
            // on n'écoute qu'une fois notre motif éteint
            rx.armer_a(duree_motif() + EXTINCTION + 200);
        }
        Self { voix, tx, rx, ec: EchoCanceller::new(), t: 0, motif, donnees, mu_rapide: 0.3, mu_lent: 0.0, erle_motif: None, echec: false, bloc: Vec::new(), puissance_erreur: 0.0, gel: 0 }
    }

    /// Chargement mesuré par notre récepteur, pour l'émetteur d'en face.
    pub fn chargement_pour_l_autre(&self) -> Option<Chargement> {
        self.rx.chargement().cloned()
    }

    /// Chargement choisi par le récepteur d'en face, pour notre émetteur.
    pub fn imposer_chargement(&mut self, c: Chargement) {
        self.tx.charger(c);
    }

    pub fn pousser_bits(&mut self, bits: impl IntoIterator<Item = u8>) {
        self.tx.pousser(bits);
    }

    pub fn prendre_bits(&mut self) -> Vec<u8> {
        self.rx.prendre()
    }

    /// Le dernier bloc reçu, écho retiré (diagnostic).
    pub fn nettoye(&self) -> &[f32] {
        &self.bloc
    }

    /// Début de nos données (temps absolu), une fois connu.
    pub fn debut_donnees(&self) -> Option<usize> {
        self.donnees
    }

    fn mu(&self) -> f32 {
        let t = self.t;
        if let Some(m) = self.motif {
            if t >= m && t < m + duree_motif() + EXTINCTION {
                return pas_annuleur(self.mu_rapide, t - m, duree_motif());
            }
        }
        match self.donnees {
            Some(d) if t >= d => {
                let en_face = match self.rx.etat {
                    Etat::Attente | Etat::Donnees => self.rx.t_donnees,
                    _ => usize::MAX,
                };
                if t < en_face {
                    self.mu_rapide * FIN_DE_PAS
                } else {
                    self.mu_lent
                }
            }
            _ => 0.0,
        }
    }

    /// Un bloc : `entree` vient de la ligne, `sortie` y part.
    pub fn traiter(&mut self, entree: &[f32], sortie: &mut [f32]) {
        assert_eq!(entree.len(), sortie.len());
        self.bloc.clear();
        for (x, y) in entree.iter().zip(sortie.iter_mut()) {
            let mu = self.mu();
            if mu == 0.0 && self.ec.mu > 0.0 && self.erle_motif.is_none() && self.motif.is_some() {
                self.erle_motif = Some(self.ec.erle_db());
            }
            // Un clic de la ligne ferait sauter les coefficients de
            // l'annuleur : on regarde d'abord l'erreur sans adapter
            // (`process` n'a alors aucun effet de bord), et on n'apprend
            // pas sur une valeur aberrante.
            self.ec.mu = 0.0;
            let e = self.ec.process(*x);
            let p = &mut self.puissance_erreur;
            if *p <= 0.0 {
                *p = e * e;
            }
            let aberrant = e * e > 16.0 * *p;
            // suivi écrêté : un clic n'y pèse pas, un vrai changement de
            // niveau s'y retrouve en quelques dizaines d'échantillons
            *p += 0.002 * ((e * e).min(16.0 * *p) - *p);
            if aberrant {
                self.gel = 64;
            }
            let e = if mu > 0.0 && self.gel == 0 {
                self.ec.mu = mu;
                self.ec.process(*x)
            } else {
                e
            };
            self.gel = self.gel.saturating_sub(1);
            self.bloc.push(e);
            let s = self.tx.echantillon();
            self.ec.push_tx(s);
            *y = s;
            self.t += 1;
        }
        let bloc = std::mem::take(&mut self.bloc);
        self.rx.traiter(&bloc);
        self.bloc = bloc;
        // L'appelé répond au motif une fois celui-ci vérifié et mesuré
        if self.voix == Voix::Appele && self.motif.is_none() && !self.echec && matches!(self.rx.etat, Etat::Attente | Etat::Donnees) {
            let s = self.rx.synchro.as_ref().unwrap();
            let m = s.debut_motif + duree_motif() + ECART;
            if m >= self.tx.t {
                let d = m + duree_motif() + 2 * RETARD_MAX;
                self.tx.programmer_motif(m);
                self.tx.programmer_donnees(d);
                self.motif = Some(m);
                self.donnees = Some(d);
            } else {
                self.echec = true;
            }
        }
    }
}
