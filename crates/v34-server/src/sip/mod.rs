//! # Passerelle SIP : de vrais appareils sur le central
//!
//! Un adaptateur téléphonique (ATA, par exemple un Cisco ATA 191/192) parle
//! SIP et RTP. Il s'enregistre auprès du central comme un abonné (un
//! « poste SIP » de l'annuaire, avec son numéro), et ce qu'on branche sur
//! sa prise téléphone (un vrai modem, un fax, un Minitel) peut alors
//! appeler les abonnés et les numéros spéciaux, et être appelé.
//!
//! Tout passe par le port UDP du central (3434) : le premier octet suffit à
//! distinguer les paquets audio du central (`V3`), sa signalisation (`VC`),
//! le RTP (version 2 : `0x80`…`0xBF`) et le SIP (texte : une méthode en
//! majuscules ou `SIP/2.0`).
//!
//! Ce module contient les briques du protocole, indépendantes du central :
//! - [`message`] : messages SIP (analyse, construction) ;
//! - [`digest`] : authentification « digest » (MD5) ;
//! - [`sdp`] : description de session (G.711 loi A, sinon loi µ) ;
//! - [`rtp`] : en-têtes RTP, recadrage en trames de 20 ms, numérotation.
//!
//! La logique d'appel (enregistrements, transactions, dialogues, relais de
//! l'audio) est dans le central (`central/sip.rs`), qui traite un poste SIP
//! comme un abonné : seule la façon de lui parler change.
//!
//! Pour un modem, la passerelle ne transcode pas (G.711 loi A de bout en
//! bout quand l'ATA l'accepte), n'envoie qu'une trame de 20 ms par paquet,
//! et ne recadence rien : l'horloge est celle de l'ATA, que les modems
//! logiciels suivent (voir [`v34_net::Pacer`]).

pub mod digest;
pub mod message;
pub mod rtp;
pub mod sdp;

/// Nom annoncé dans les en-têtes `User-Agent` / `Server`.
pub fn agent() -> String {
    format!("V34NET/{}", env!("CARGO_PKG_VERSION"))
}

/// Date au format HTTP/SIP (RFC 1123), en temps universel : les ATA s'en
/// servent pour mettre leur horloge à l'heure.
pub fn http_date(unix: u64) -> String {
    const JOURS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MOIS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let days = (unix / 86_400) as i64;
    let secs = unix % 86_400;
    // jours → date civile (algorithme de H. Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{}, {:02} {} {} {:02}:{:02}:{:02} GMT",
        JOURS[days.rem_euclid(7) as usize],
        d,
        MOIS[(m - 1) as usize],
        y,
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_http() {
        assert_eq!(http_date(0), "Thu, 01 Jan 1970 00:00:00 GMT");
        assert_eq!(http_date(951_782_400), "Tue, 29 Feb 2000 00:00:00 GMT");
        assert_eq!(http_date(1_794_830_405), "Mon, 16 Nov 2026 12:00:05 GMT");
    }
}
