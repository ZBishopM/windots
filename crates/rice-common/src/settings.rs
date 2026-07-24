//! `~/.config/rice.json`: the handful of values that were compiled into binaries
//! or duplicated across files.
//!
//! Two of these caused real problems. The bar height was written in three places
//! across two binaries, so changing it made the ws-slide animation tear by the
//! delta. The mic allowlist was a `const` in micswitch, so new hardware meant a
//! recompile.
//!
//! Missing file or missing key falls back to the defaults below, so the rice
//! runs with no rice.json at all.

use serde::{Deserialize, Serialize};

use crate::config::config_path;

pub const FILE: &str = "rice.json";

fn default_bar_height() -> i32 { 34 }
fn default_mics() -> Vec<String> { vec!["hyperx".into(), "snowball".into()] }
fn default_ipc() -> String { "ws://127.0.0.1:6123".into() }

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Settings {
    /// Height of the status bar in pixels. The slide animation leaves this many
    /// rows untouched so the bar doesn't move with the desktop.
    #[serde(default = "default_bar_height")]
    pub bar_height: i32,

    /// Substrings matched against capture-device friendly names; `micswitch`
    /// cycles between the devices that match, in this order.
    #[serde(default = "default_mics")]
    pub mics: Vec<String>,

    #[serde(default = "default_ipc")]
    pub ipc_url: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            bar_height: default_bar_height(),
            mics: default_mics(),
            ipc_url: default_ipc(),
        }
    }
}

impl Settings {
    /// Read `~/.config/rice.json`, falling back to defaults on any problem --
    /// a malformed config must not stop the desktop from starting.
    pub fn load() -> Self {
        std::fs::read_to_string(config_path(FILE))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Cached for the process lifetime.
    pub fn get() -> &'static Settings {
        static S: std::sync::OnceLock<Settings> = std::sync::OnceLock::new();
        S.get_or_init(Settings::load)
    }

    pub fn write_default_if_missing() -> std::io::Result<()> {
        let p = config_path(FILE);
        if p.exists() {
            return Ok(());
        }
        let json = serde_json::to_string_pretty(&Settings::default()).unwrap_or_default();
        std::fs::write(p, json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_previously_compiled_in_values() {
        let s = Settings::default();
        assert_eq!(s.bar_height, 34);
        assert_eq!(s.mics, vec!["hyperx", "snowball"]);
    }

    #[test]
    fn partial_json_keeps_other_defaults() {
        let s: Settings = serde_json::from_str(r#"{"bar_height": 40}"#).unwrap();
        assert_eq!(s.bar_height, 40);
        assert_eq!(s.mics, vec!["hyperx", "snowball"]);
    }
}
