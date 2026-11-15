//! Verrou consultatif partagé entre le central et le site.
//!
//! Le central (service systemd) et le site (conteneur `v34-portail`)
//! écrivent parfois les mêmes petits fichiers JSON du dossier de données :
//! les Tinyblogs, que l'administration du site modère, et les messages
//! laissés sur les numéros cachés. Chaque écriture est atomique (fichier
//! temporaire puis renommage), mais une lecture-modification-écriture d'un
//! processus pourrait effacer celle de l'autre. Les deux prennent donc le
//! même verrou : le fichier `.verrou` du dossier, verrouillé (flock) le
//! temps de la modification.
//!
//! Le verrou est consultatif : un programme qui ne le prend pas n'est pas
//! bloqué. Si le fichier ne peut pas être créé (dossier en lecture seule),
//! on continue sans verrou, comme avant.

use std::fs::{File, OpenOptions};
use std::path::Path;

use crate::debug;

/// Nom du fichier de verrou, dans le dossier protégé
pub const FICHIER: &str = ".verrou";

/// Verrou pris ; relâché quand il est lâché (`drop`), ou si le processus
/// s'arrête.
#[must_use = "le verrou est relâché aussitôt s'il n'est pas gardé"]
pub struct Verrou(#[allow(dead_code)] Option<File>);

/// Attend puis prend le verrou exclusif du dossier `dossier` (créé au
/// besoin).
pub fn exclusif(dossier: &Path) -> Verrou {
    let ouvrir = || -> std::io::Result<File> {
        std::fs::create_dir_all(dossier)?;
        let mut o = OpenOptions::new();
        o.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o640);
        }
        let chemin = dossier.join(FICHIER);
        // (créé par un autre utilisateur ? flock marche aussi en lecture seule)
        let f = o.open(&chemin).or_else(|_| File::open(&chemin))?;
        f.lock()?;
        Ok(f)
    };
    match ouvrir() {
        Ok(f) => Verrou(Some(f)),
        Err(e) => {
            debug!("verrou de {} indisponible ({e}) : modification sans verrou", dossier.display());
            Verrou(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn un_seul_a_la_fois() {
        let d = std::env::temp_dir().join(format!("v34-verrou-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let v = exclusif(&d);
        assert!(d.join(FICHIER).exists());
        // Un autre « processus » (une autre ouverture du fichier) attend.
        let pris = Arc::new(AtomicBool::new(false));
        let (d2, p2) = (d.clone(), pris.clone());
        let t = std::thread::spawn(move || {
            let _v = exclusif(&d2);
            p2.store(true, Ordering::SeqCst);
        });
        std::thread::sleep(Duration::from_millis(150));
        assert!(!pris.load(Ordering::SeqCst), "le second attend le premier");
        drop(v);
        t.join().unwrap();
        assert!(pris.load(Ordering::SeqCst));
        let _ = std::fs::remove_dir_all(&d);
    }
}
