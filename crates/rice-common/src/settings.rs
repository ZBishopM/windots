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
// ---------------------------------------------------------------- animation
// Everything the desktop's motion is made of, in one place. These were literals
// scattered through glaze-bar's render loop, which meant a rebuild to try a
// different feel -- so nobody ever tried a different feel.
//
// Reloaded from disk while running (see `live`), so editing rice.json shows up
// on the next frame without restarting anything.

fn d_spring_k() -> f32 { 300.0 }
fn d_spring_d() -> f32 { 23.0 }
fn d_pill_ease() -> f32 { 15.0 }
fn d_text_ease() -> f32 { 14.0 }
fn d_ws_ease() -> f32 { 16.0 }
fn d_notif_hold() -> f32 { 4.0 }
fn d_panel_timeout() -> f32 { 6.0 }
fn d_spectrum_fps() -> u32 { 30 }

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Animation {
    /// Stiffness of the island's vertical bubble spring. Higher = snappier.
    #[serde(default = "d_spring_k")]
    pub spring_stiffness: f32,

    /// Damping of that spring. The ratio that matters is
    /// `damping / (2 * sqrt(stiffness))`: below 1.0 it overshoots and bounces,
    /// at 1.0 it glides in without ever overshooting. The default works out to
    /// 0.66, which is one clear bounce. Around 0.6 it rings several times;
    /// around 0.75 the overshoot is a few pixels and reads as no bounce at all.
    #[serde(default = "d_spring_d")]
    pub spring_damping: f32,

    /// How fast the pill itself resizes, as an exponential rate. Higher =
    /// faster. This one does not overshoot.
    #[serde(default = "d_pill_ease")]
    pub pill_ease: f32,

    /// How fast text (the clock) travels and scales as the island opens. Kept
    /// separate from the spring on purpose: the shape is allowed to bounce, the
    /// text is not, because letters visibly wobbling looks like a glitch.
    #[serde(default = "d_text_ease")]
    pub text_ease: f32,

    /// How fast the focused-workspace highlight slides between workspaces.
    #[serde(default = "d_ws_ease")]
    pub workspace_ease: f32,

    /// Seconds a notification stays in the island before it collapses.
    #[serde(default = "d_notif_hold")]
    pub notification_hold_secs: f32,

    /// Seconds of no interaction before an open island panel closes itself.
    #[serde(default = "d_panel_timeout")]
    pub panel_timeout_secs: f32,

    /// Frames per second for the audio spectrum while sound is playing. It is
    /// the one thing in the bar that repaints continuously, so this is the main
    /// lever on the bar's CPU cost: measured on a 2560px bar, 60fps cost ~8% of
    /// a core and 30fps costs ~1% for no visible difference.
    #[serde(default = "d_spectrum_fps")]
    pub spectrum_fps: u32,
}

impl Default for Animation {
    fn default() -> Self {
        Self {
            spring_stiffness: d_spring_k(),
            spring_damping: d_spring_d(),
            pill_ease: d_pill_ease(),
            text_ease: d_text_ease(),
            workspace_ease: d_ws_ease(),
            notification_hold_secs: d_notif_hold(),
            panel_timeout_secs: d_panel_timeout(),
            spectrum_fps: d_spectrum_fps(),
        }
    }
}

fn default_animation() -> Animation { Animation::default() }


fn default_bar_height() -> i32 { 34 }
fn default_mics() -> Vec<String> { vec!["hyperx".into(), "snowball".into()] }
fn default_outputs() -> Vec<String> { vec!["hyperx".into(), "vg270".into(), "airpods".into()] }
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

    /// Substrings matched against playback-device names, in the order they
    /// should appear in the island's device page.
    ///
    /// A whitelist rather than the full list on purpose: this machine reports 29
    /// playback endpoints, of which 13 are "active", and nearly all of them are
    /// virtual -- six RODE UNIFY buses, VB-Cable, VoiceMeeter, two Steam
    /// Streaming devices, Oculus, NVIDIA Broadcast. Showing them all would bury
    /// the two or three real ones.
    ///
    /// Bluetooth audio devices are always listed too, whether or not they match
    /// anything here, so a newly paired headset shows up without editing this.
    #[serde(default = "default_outputs")]
    pub outputs: Vec<String>,

    #[serde(default = "default_ipc")]
    pub ipc_url: String,

    /// Executables that should always make the bar click-through while focused,
    /// matched case-insensitively as substrings of the exe name.
    ///
    /// The geometric test (does the focused window cover this monitor?) handles
    /// true fullscreen, but some games in *borderless* mode sit a few pixels
    /// short of the monitor rect, or place a child window on top, and slip past
    /// it. Naming the executable is unambiguous.
    #[serde(default = "default_clickthrough")]
    pub clickthrough_apps: Vec<String>,

    /// Motion tuning. Edit and save; the bar picks it up on the next frame.
    #[serde(default = "default_animation")]
    pub animation: Animation,
}

fn default_clickthrough() -> Vec<String> {
    vec!["league of legends".into(), "valorant".into()]
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            bar_height: default_bar_height(),
            mics: default_mics(),
            outputs: default_outputs(),
            ipc_url: default_ipc(),
            clickthrough_apps: default_clickthrough(),
            animation: Animation::default(),
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

    /// Like `get`, but re-reads the file when it changes on disk, so the values
    /// can be tuned from an editor with the desktop running. Only the mtime is
    /// checked, and only once a second, so the common path is one `stat`.
    ///
    /// `get` still exists and is still cached forever: things like the bar
    /// height are read once during layout and cannot change under the code that
    /// consumed them.
    pub fn live() -> std::sync::Arc<Settings> {
        use std::sync::{Arc, Mutex, OnceLock};
        use std::time::{Duration, Instant, SystemTime};
        struct Cell {
            value: Arc<Settings>,
            mtime: Option<SystemTime>,
            checked: Instant,
        }
        static CELL: OnceLock<Mutex<Cell>> = OnceLock::new();
        let cell = CELL.get_or_init(|| {
            Mutex::new(Cell {
                value: Arc::new(Settings::load()),
                mtime: std::fs::metadata(config_path(FILE)).ok().and_then(|m| m.modified().ok()),
                checked: Instant::now(),
            })
        });
        let mut c = cell.lock().unwrap();
        if c.checked.elapsed() >= Duration::from_secs(1) {
            c.checked = Instant::now();
            let m = std::fs::metadata(config_path(FILE)).ok().and_then(|m| m.modified().ok());
            if m != c.mtime {
                c.mtime = m;
                // A half-written file (the editor saving) parses as garbage and
                // load() falls back to defaults, which would flash the whole UI
                // back to stock. Keep the previous value unless the parse works.
                if let Some(next) = std::fs::read_to_string(config_path(FILE))
                    .ok()
                    .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
                {
                    c.value = Arc::new(next);
                }
            }
        }
        c.value.clone()
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
