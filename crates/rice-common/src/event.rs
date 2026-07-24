//! The island/toast notification payload.
//!
//! One logical event was previously encoded three ways by hand: a struct in the
//! bar, a second struct for the `island.json` on-disk form, and a set of CLI
//! flags for the toast -- with the save script writing the JSON *and* passing the
//! same four fields as flags in the next line.

use serde::{Deserialize, Serialize};

use crate::config::config_path;

pub const ISLAND_FILE: &str = "island.json";

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
