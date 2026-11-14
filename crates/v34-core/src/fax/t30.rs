//! Trames de la procédure T.30 : identification (DIS, CSI), commande
//! (DCS, TSI), confirmations (CFR, FTT, MCF, RTN), fin de page (EOP, MPS)
//! et déconnexion (DCN).
//!
//! Une trame : adresse 0xFF, contrôle 0x03 (0x13 pour la dernière trame
//! d'une salve), FCF (code de la trame), FIF (paramètres). Les codes sont
//! donnés tels qu'ils partent (bit de poids faible en premier) ; le bit X
//! (poids faible du FCF) vaut 1 dans les trames de la station qui a reçu
//! le DIS (l'appelant, ici l'émetteur de la page).
//!
//! FIF du DIS et du DCS, bits numérotés à partir de 1 (bit 1 = premier
//! émis) : 10 réception de fax, 11 à 14 débit, 15 résolution fine, 17-18
//! largeur (00 : 215 mm), 19-20 longueur (00 : A4), 21 à 23 temps minimal
//! de ligne (111 : 0 ms), 24 extension.
//!
//! Écart connu : notre repli à 4800 bit/s est du V.29 à 4800 (le T.30 prévoit
//! du V.27ter à ce débit), ce qui suffit entre deux appareils de ce logiciel.

use super::v29::Debit;

pub const ADRESSE: u8 = 0xFF;
pub const CONTROLE: u8 = 0x03;
pub const CONTROLE_FINAL: u8 = 0x13;

pub const DIS: u8 = 0x80;
pub const CSI: u8 = 0x40;
pub const DCS: u8 = 0x82;
pub const TSI: u8 = 0x42;
pub const CFR: u8 = 0x84;
pub const FTT: u8 = 0x44;
pub const EOP: u8 = 0x2E;
pub const MPS: u8 = 0x4E;
pub const MCF: u8 = 0x8C;
pub const RTN: u8 = 0x4C;
pub const DCN: u8 = 0xFA;

/// Nom d'une trame (pour les journaux et l'affichage).
pub fn nom(fcf: u8) -> &'static str {
    match fcf & 0xFE {
        DIS => "DIS",
        CSI => "CSI",
        DCS => "DCS",
        TSI => "TSI",
        CFR => "CFR",
        FTT => "FTT",
        EOP => "EOP",
        MPS => "MPS",
        MCF => "MCF",
        RTN => "RTN",
        DCN => "DCN",
        _ => "?",
    }
}

/// Une trame décodée.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trame {
    pub fcf: u8,
    pub fif: Vec<u8>,
    pub finale: bool,
}

impl Trame {
    pub fn code(&self) -> u8 {
        self.fcf & 0xFE
    }
}

/// Octets d'une trame à émettre.
pub fn construire(fcf: u8, fif: &[u8], finale: bool) -> Vec<u8> {
    let mut v = vec![ADRESSE, if finale { CONTROLE_FINAL } else { CONTROLE }, fcf];
    v.extend_from_slice(fif);
    v
}

/// Lit une trame reçue (FCS déjà vérifié).
pub fn lire(octets: &[u8]) -> Option<Trame> {
    if octets.len() < 3 || octets[0] != ADRESSE || (octets[1] & 0xEF) != CONTROLE {
        return None;
    }
    Some(Trame { fcf: octets[2], fif: octets[3..].to_vec(), finale: octets[1] == CONTROLE_FINAL })
}

fn mettre(fif: &mut [u8], bit: usize, v: bool) {
    if v {
        fif[(bit - 1) / 8] |= 1 << ((bit - 1) % 8);
    }
}

fn lire_bit(fif: &[u8], bit: usize) -> bool {
    fif.get((bit - 1) / 8).is_some_and(|o| o & (1 << ((bit - 1) % 8)) != 0)
}

/// Bits 11 à 14 (débit)
fn code_debit(fif: &[u8]) -> [bool; 4] {
    [lire_bit(fif, 11), lire_bit(fif, 12), lire_bit(fif, 13), lire_bit(fif, 14)]
}

fn fif_commun(fif: &mut [u8], debit: [bool; 4], fine: bool) {
    mettre(fif, 10, true);
    for (i, &b) in debit.iter().enumerate() {
        mettre(fif, 11 + i, b);
    }
    mettre(fif, 15, fine);
    // 0 ms par ligne
    mettre(fif, 21, true);
    mettre(fif, 22, true);
    mettre(fif, 23, true);
}

/// DIS : réception de fax, V.27ter et V.29, résolution fine.
pub fn fif_dis() -> Vec<u8> {
    let mut fif = vec![0u8; 3];
    fif_commun(&mut fif, [true, true, false, false], true);
    fif
}

/// Le correspondant sait-il faire du V.29 ?
pub fn dis_v29(fif: &[u8]) -> bool {
    lire_bit(fif, 11)
}

/// DCS : débit choisi, résolution.
pub fn fif_dcs(debit: Debit, fine: bool) -> Vec<u8> {
    let mut fif = vec![0u8; 3];
    let code = match debit {
        Debit::B9600 => [true, false, false, false],
        Debit::B7200 => [true, true, false, false],
        Debit::B4800 => [false, true, false, false],
    };
    fif_commun(&mut fif, code, fine);
    fif
}

/// Débit et résolution d'un DCS.
pub fn dcs_lire(fif: &[u8]) -> Option<(Debit, bool)> {
    let d = match code_debit(fif) {
        [true, false, false, false] => Debit::B9600,
        [true, true, false, false] => Debit::B7200,
        [false, true, false, false] => Debit::B4800,
        _ => return None,
    };
    Some((d, lire_bit(fif, 15)))
}

/// Identifiant (CSI, TSI) : 20 caractères, chiffres, espaces et « + »,
/// émis à l'envers (le dernier caractère d'abord).
pub fn fif_ident(numero: &str) -> Vec<u8> {
    let s: String = numero.chars().filter(|c| c.is_ascii_digit() || *c == '+' || *c == ' ').take(20).collect();
    let mut v: Vec<u8> = format!("{s:<20}").bytes().collect();
    v.reverse();
    v
}

pub fn ident_lire(fif: &[u8]) -> String {
    let mut v: Vec<u8> = fif.iter().copied().filter(|c| c.is_ascii_digit() || *c == b'+' || *c == b' ').collect();
    v.reverse();
    String::from_utf8_lossy(&v).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dcs_aller_retour() {
        for d in [Debit::B9600, Debit::B7200, Debit::B4800] {
            for fine in [false, true] {
                assert_eq!(dcs_lire(&fif_dcs(d, fine)), Some((d, fine)));
            }
        }
        assert!(dis_v29(&fif_dis()));
    }

    #[test]
    fn identifiants() {
        let f = fif_ident("0142424242");
        assert_eq!(f.len(), 20);
        assert_eq!(f[19], b'0');
        assert_eq!(ident_lire(&f), "0142424242");
    }

    #[test]
    fn trames() {
        let o = construire(DCS | 1, &fif_dcs(Debit::B9600, true), true);
        let t = lire(&o).unwrap();
        assert_eq!((t.code(), t.finale, nom(t.fcf)), (DCS, true, "DCS"));
    }
}
