//! Journalisation minimale sur la sortie d'erreur.
//!
//! Sous systemd (variable `JOURNAL_STREAM` présente), chaque ligne est
//! préfixée par sa priorité syslog (`<3>`, `<4>`, `<6>`, `<7>`) : journald
//! la comprend et `journalctl -p warning` fonctionne. Sinon, horodatage UTC.

use std::fmt;
use std::io::Write;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
}

static LEVEL: AtomicU8 = AtomicU8::new(Level::Info as u8);
static JOURNAL: OnceLock<bool> = OnceLock::new();

impl Level {
    pub fn parse(s: &str) -> Option<Level> {
        match s.to_ascii_lowercase().as_str() {
            "error" | "erreur" => Some(Level::Error),
            "warn" | "warning" | "alerte" => Some(Level::Warn),
            "info" => Some(Level::Info),
            "debug" | "détail" | "detail" => Some(Level::Debug),
            _ => None,
        }
    }
}

pub fn set_level(l: Level) {
    LEVEL.store(l as u8, Ordering::Relaxed);
}

pub fn enabled(l: Level) -> bool {
    l as u8 <= LEVEL.load(Ordering::Relaxed)
}

pub fn write(l: Level, args: fmt::Arguments) {
    if !enabled(l) {
        return;
    }
    let journal = *JOURNAL.get_or_init(|| std::env::var_os("JOURNAL_STREAM").is_some());
    let line = if journal {
        let prio = match l {
            Level::Error => 3,
            Level::Warn => 4,
            Level::Info => 6,
            Level::Debug => 7,
        };
        format!("<{prio}>{args}\n")
    } else {
        let tag = match l {
            Level::Error => "ERREUR",
            Level::Warn => "ALERTE",
            Level::Info => "info  ",
            Level::Debug => "détail",
        };
        format!("{} {tag} {args}\n", timestamp())
    };
    let _ = std::io::stderr().lock().write_all(line.as_bytes());
}

/// « AAAA-MM-JJ hh:mm:ss » UTC, sans dépendance.
fn timestamp() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, rem) = ((secs / 86400) as i64, secs % 86400);
    // Algorithme « civil_from_days » de H. Hinnant.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", rem / 3600, rem / 60 % 60, rem % 60)
}

#[macro_export]
macro_rules! error {
    ($($t:tt)*) => { $crate::log::write($crate::log::Level::Error, format_args!($($t)*)) };
}
#[macro_export]
macro_rules! warn {
    ($($t:tt)*) => { $crate::log::write($crate::log::Level::Warn, format_args!($($t)*)) };
}
#[macro_export]
macro_rules! info {
    ($($t:tt)*) => { $crate::log::write($crate::log::Level::Info, format_args!($($t)*)) };
}
#[macro_export]
macro_rules! debug {
    ($($t:tt)*) => { $crate::log::write($crate::log::Level::Debug, format_args!($($t)*)) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn niveaux() {
        assert_eq!(Level::parse("DEBUG"), Some(Level::Debug));
        assert_eq!(Level::parse("erreur"), Some(Level::Error));
        assert_eq!(Level::parse("bavard"), None);
        assert!(Level::Error < Level::Debug);
    }

    #[test]
    fn horodatage() {
        let t = timestamp();
        assert_eq!(t.len(), 19, "{t}");
        assert!(t.starts_with("20"), "{t}");
    }
}
