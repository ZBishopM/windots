//! The island/toast notification payload.
//!
//! One logical event was previously encoded three ways by hand: a struct in the
//! bar, a second struct for the `island.json` on-disk form, and a set of CLI
//! flags for the toast -- with the save script writing the JSON *and* passing the
//! same four fields as flags in the next line.

use serde::{Deserialize, Serialize};

use crate::config::config_path;

pub const ISLAND_FILE: &str = "island.json";

/// Historial de notificaciones, para el centro de notificaciones de la barra.
pub const HISTORY_FILE: &str = "notifications.json";
/// Cuántas se guardan. Cincuenta son varios días de uso normal y el archivo
/// entero sigue siendo de unos pocos KB, así que la barra puede releerlo entero
/// cada vez que cambia sin pensárselo.
pub const HISTORY_MAX: usize = 50;
/// Cerrojo del historial. Lo escriben `notifyd` y `Set-RiceIsland`, así que dos
/// escribir-y-renombrar simultáneos perderían uno.
pub const HISTORY_LOCK: &str = "Global\\rice-notif-history";

/// Una notificación ya ocurrida.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct NotifRecord {
    pub icon: String,
    pub title: String,
    pub body: String,
    pub accent: String,
    /// Milisegundos desde epoch. Hace de identificador para descartarla: dos
    /// notificaciones distintas no caen en el mismo milisegundo, y si cayeran
    /// descartar las dos a la vez tampoco sorprendería a nadie.
    pub at: u64,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Lee el historial, de la más reciente a la más antigua.
pub fn history() -> Vec<NotifRecord> {
    std::fs::read_to_string(config_path(HISTORY_FILE))
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<NotifRecord>>(&t).ok())
        .unwrap_or_default()
}

/// Aplica un cambio al historial bajo el cerrojo, y lo deja escrito.
fn edit_history<F: FnOnce(&mut Vec<NotifRecord>)>(f: F) -> std::io::Result<()> {
    #[cfg(windows)]
    let _lock = crate::win::NamedLock::acquire(HISTORY_LOCK, 2000);
    let mut v = history();
    f(&mut v);
    v.truncate(HISTORY_MAX);
    let dst = config_path(HISTORY_FILE);
    let tmp = dst.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string(&v).unwrap_or_else(|_| "[]".into()))?;
    std::fs::rename(&tmp, &dst)
}

/// Quita una notificación por su marca de tiempo.
pub fn history_dismiss(at: u64) -> std::io::Result<()> {
    edit_history(|v| v.retain(|n| n.at != at))
}

/// Vacía el historial.
pub fn history_clear() -> std::io::Result<()> {
    edit_history(|v| v.clear())
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct IslandEvent {
    /// Icon keyword; see `ui::icon_glyph`.
    pub icon: String,
    pub title: String,
    pub body: String,
    /// `#rrggbb`; see `theme::parse_hex`.
    pub accent: String,
}

impl IslandEvent {
    pub fn new(icon: &str, title: &str, body: &str, accent: &str) -> Self {
        Self {
            icon: icon.into(),
            title: title.into(),
            body: body.into(),
            accent: accent.into(),
        }
    }

    /// Accent as RGB, falling back to the amber default.
    pub fn accent_rgb(&self) -> [u8; 3] {
        crate::theme::parse_hex(&self.accent).unwrap_or(crate::theme::ACCENT)
    }

    /// Read the current `~/.config/island.json`.
    pub fn load() -> Option<Self> {
        let raw = std::fs::read_to_string(config_path(ISLAND_FILE)).ok()?;
        serde_json::from_str(&raw).ok()
    }

    /// Publish to `~/.config/island.json` for the in-bar island to pick up.
    ///
    /// Written to a temp file and renamed: the bar polls this path, and a
    /// truncate-then-write could be observed half-written.
    pub fn publish(&self) -> std::io::Result<()> {
        let dst = config_path(ISLAND_FILE);
        let tmp = dst.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string(self).unwrap_or_default())?;
        std::fs::rename(&tmp, &dst)
    }

    /// Anota esto en el historial.
    ///
    /// Va aparte de `publish`: cómo se MUESTRA una notificación depende de
    /// `notification_style` (isla, toast, o las dos), pero el historial tiene que
    /// verlas todas. Con `style = toast` -- que es el valor por defecto -- la
    /// isla ni se entera, así que colgar el historial de `island.json` habría
    /// dejado fuera justo las notificaciones del sistema.
    pub fn record(&self) -> std::io::Result<()> {
        let n = NotifRecord {
            icon: self.icon.clone(),
            title: self.title.clone(),
            body: self.body.clone(),
            accent: self.accent.clone(),
            at: now_ms(),
        };
        edit_history(|v| v.insert(0, n))
    }

    /// The equivalent `shadowplay-notify` command line.
    pub fn to_notify_args(&self) -> Vec<String> {
        vec![
            "--title".into(),
            self.title.clone(),
            "--body".into(),
            self.body.clone(),
            "--icon".into(),
            self.icon.clone(),
            "--accent".into(),
            self.accent.clone(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_falls_back_to_amber() {
        let e = IslandEvent::new("info", "t", "b", "not-a-colour");
        assert_eq!(e.accent_rgb(), crate::theme::ACCENT);
    }

    #[test]
    fn accent_parses_when_valid() {
        let e = IslandEvent::new("replay", "t", "b", "#a9b56a");
        assert_eq!(e.accent_rgb(), crate::theme::ACCENT_OK);
    }

    #[test]
    fn notify_args_roundtrip_fields() {
        let e = IslandEvent::new("mic", "Micrófono", "HyperX", "#e0a35c");
        let a = e.to_notify_args();
        assert_eq!(a[1], "Micrófono");
        assert_eq!(a[5], "mic");
    }
}
