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
// ---------------------------------------------------------------- launcher
fn d_index_store() -> bool { false }
fn d_index_files() -> bool { true }

/// Empty means "the user profile plus every fixed drive", worked out at runtime.
/// Spelling them out here is for narrowing the index, not widening it.
fn d_file_roots() -> Vec<String> { Vec::new() }

/// Directory paths the walk refuses to descend into, matched as case-insensitive
/// suffixes of the directory's full path -- so `\node_modules` catches it at any
/// depth while `\.cargo\registry` catches only the one below `.cargo`.
///
/// These are not arbitrary. Counted on this machine, they are 500,079 of the
/// 1,394,364 entries under the profile: 182,390 in node_modules, 96,078 in
/// vscode extensions, 67,166 in .rustup, 64,613 in AppData\Local\Microsoft and
/// 53,789 in the cargo registry. None of it is a file anyone searches for by
/// name, and all of it would be paid for in RAM on every keystroke.
fn d_file_skip() -> Vec<String> {
    [
        r"\node_modules",
        r"\.git",
        r"\.rustup",
        r"\.cargo\registry",
        r"\.nuget",
        r"\.gradle",
        r"\.m2",
        r"\.vscode\extensions",
        r"\.vscode-insiders\extensions",
        r"\__pycache__",
        r"\.venv",
        r"\node_modules.bin",
        r"\target\debug",
        r"\target\release",
        r"\appdata\local\temp",
        r"\appdata\local\packages",
        r"\appdata\local\microsoft",
        r"\appdata\locallow",
        r"\$recycle.bin",
        r"\system volume information",
        r"\windows\winsxs",
        r"\windows\servicing",
        r"\windows\assembly",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// A ceiling, not a target. Reached only if the roots are much larger than this
/// machine's ~1.7M entries; it exists so a network drive or a runaway junction
/// cannot turn the index into an out-of-memory bug.
fn d_file_limit() -> usize { 4_000_000 }

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Launcher {
    /// Include Store/UWP apps in the search index.
    ///
    /// Off by default. They drag in a long tail that is not really applications
    /// -- settings pages, help links, "Visit <vendor>" web shortcuts -- and each
    /// can only be started indirectly through its AUMID, which also means it can
    /// never be launched elevated.
    ///
    /// Turning it off has a real cost: anything that ships ONLY as a packaged
    /// app becomes unfindable. Here that includes Claude and Windows Terminal.
    #[serde(default = "d_index_store")]
    pub index_store_apps: bool,

    /// Search files, not just applications.
    #[serde(default = "d_index_files")]
    pub index_files: bool,

    /// What the file index covers. Empty = profile + every fixed drive.
    #[serde(default = "d_file_roots")]
    pub file_roots: Vec<String>,

    /// Path suffixes the walk will not descend into.
    #[serde(default = "d_file_skip")]
    pub file_skip: Vec<String>,

    /// Hard cap on indexed entries.
    #[serde(default = "d_file_limit")]
    pub file_limit: usize,
}

impl Default for Launcher {
    fn default() -> Self {
        Self {
            index_store_apps: d_index_store(),
            index_files: d_index_files(),
            file_roots: d_file_roots(),
            file_skip: d_file_skip(),
            file_limit: d_file_limit(),
        }
    }
}

fn default_launcher() -> Launcher { Launcher::default() }

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
fn default_notification_style() -> String { "toast".into() }
fn default_ipc() -> String { "ws://127.0.0.1:6123".into() }

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Consumo {
    /// Precio de la electricidad por kWh, del recibo. En 0 el medidor enseña
    /// energia pero no dinero, a proposito: es mejor no dar una cifra que dar
    /// una inventada.
    #[serde(default)]
    pub precio_kwh: f64,

    /// Simbolo con el que se muestra el dinero.
    #[serde(default = "d_moneda")]
    pub moneda: String,

    /// Vatios del resto del equipo: placa, RAM, discos, ventiladores, USB, RGB.
    ///
    /// Ningun sensor lo publica, asi que es una constante -- y no es un detalle
    /// menor: en reposo puede ser MAS de la mitad del consumo, porque la GPU
    /// baja a ~30 W y la CPU a poco. Sin este termino el medidor se quedaria
    /// corto justo en el estado donde el equipo pasa la mayor parte del dia.
    /// El valor por defecto es el tipico de un sobremesa; calibralo contra un
    /// enchufe medidor si quieres que cuadre con el recibo.
    #[serde(default = "d_base_w")]
    pub base_w: f64,

    /// Rendimiento de la fuente, 0..1. Lo que se factura entra por el enchufe,
    /// no por los rieles: una fuente al 90% tira 111 W de la pared para dar 100.
    #[serde(default = "d_eficiencia_psu")]
    pub eficiencia_psu: f64,

    /// Vatios de los monitores. No salen en ningun sensor y tienen su propia
    /// fuente, asi que van aparte y no pasan por la eficiencia de la del PC.
    /// En 0 el numero es "solo la torre".
    #[serde(default)]
    pub monitores_w: f64,

    /// Cada cuantos segundos se muestrea.
    #[serde(default = "d_intervalo_s")]
    pub intervalo_s: u64,

    /// JSON de LibreHardwareMonitor, de donde sale la potencia de CPU. Vacio
    /// para no intentarlo. Ojo: LHM tiene que correr ELEVADO o ni el servidor
    /// web arranca ni los sensores de potencia existen.
    #[serde(default = "d_lhm_url")]
    pub lhm_url: String,
}

fn d_moneda() -> String { "S/".into() }
fn d_base_w() -> f64 { 45.0 }
fn d_eficiencia_psu() -> f64 { 0.90 }
fn d_intervalo_s() -> u64 { 10 }
fn d_lhm_url() -> String { "http://localhost:8085/data.json".into() }

fn default_consumo() -> Consumo {
    Consumo {
        precio_kwh: 0.0,
        moneda: d_moneda(),
        base_w: d_base_w(),
        eficiencia_psu: d_eficiencia_psu(),
        monitores_w: 0.0,
        intervalo_s: d_intervalo_s(),
        lhm_url: d_lhm_url(),
    }
}

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

    /// Which microphone ShadowPlay records, as a substring of the device's
    /// friendly name ("hyperx"). Empty = whatever Windows has as default.
    ///
    /// Exists because "whatever Windows has as default" quietly stopped being
    /// the right answer: this machine has six capture endpoints, and when the
    /// wireless headset is off Windows falls back to the condenser mic across
    /// the desk — which keeps recording, just of the room, ~25 dB below
    /// speech. From outside that is indistinguishable from a dead microphone,
    /// and it ate the voice track of several clips before anyone noticed.
    ///
    /// Deliberately NOT reusing `mics`: that list is micswitch's cycling
    /// order, and a device being in it doesn't mean it's the one to record.
    /// Deliberately empty by default so nothing changes for a machine where
    /// the default endpoint was already correct.
    ///
    /// There is no matching setting for the loopback side on purpose —
    /// following the default *render* device is right there, since the point
    /// is to capture whatever is actually playing.
    #[serde(default)]
    pub record_mic: String,

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

    /// How notifyd surfaces a Windows notification: `toast` (a popup, the direct
    /// replacement for the Windows banner), `island` (inline in the bar, quieter),
    /// or `both`.
    ///
    /// It used to do both unconditionally, which meant every notification arrived
    /// twice -- and three times counting the Windows banner underneath.
    #[serde(default = "default_notification_style")]
    pub notification_style: String,

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

    /// Hide the bar entirely while a fullscreen application covers its monitor.
    ///
    /// The bar used to only become click-through there: still drawn, still on
    /// top of the game, just not clickable. Notifications are the exception --
    /// the island still appears over a game, because the point of a notification
    /// is to be seen. Anything else (workspaces, clock, metrics) is noise over
    /// something running fullscreen.
    #[serde(default = "default_hide_on_fs")]
    pub hide_bar_on_fullscreen: bool,

    /// Motion tuning. Edit and save; the bar picks it up on the next frame.
    #[serde(default = "default_animation")]
    pub animation: Animation,

    /// Win+Space search box.
    #[serde(default = "default_launcher")]
    pub launcher: Launcher,

    /// Estimador de gasto electrico.
    #[serde(default = "default_consumo")]
    pub consumo: Consumo,
}

fn default_hide_on_fs() -> bool { true }

fn default_clickthrough() -> Vec<String> {
    vec!["league of legends".into(), "valorant".into()]
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            bar_height: default_bar_height(),
            mics: default_mics(),
            record_mic: String::new(), // vacio = el predeterminado de Windows
            outputs: default_outputs(),
            notification_style: default_notification_style(),
            ipc_url: default_ipc(),
            clickthrough_apps: default_clickthrough(),
            hide_bar_on_fullscreen: default_hide_on_fs(),
            animation: Animation::default(),
            launcher: Launcher::default(),
            consumo: default_consumo(),
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
