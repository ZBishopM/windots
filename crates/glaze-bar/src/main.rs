#![windows_subsystem = "windows"] // no console window

use eframe::egui;
use serde::Deserialize;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rice_common::ui::{col, draw_icon};
use rice_common::{config, theme, win};

// Single-instance per monitor: hold a named mutex keyed by --x. A second bar for
// the same monitor (supervisor race, stray manual launch) finds it already held
// and exits immediately, so bars can never duplicate.
fn claim_single_instance(x: i32) {
    win::single_instance_or_exit(&format!("Global\\glaze-bar-{x}"));
}

// ---- Auto click-through: when a fullscreen app (a game) covers this bar's monitor,
// make the bar transparent to mouse input so clicks reach the game; otherwise keep
// it clickable (workspaces). ----
#[cfg(windows)]
#[repr(C)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}
#[cfg(windows)]
#[repr(C)]
struct MonInfo {
    cb: u32,
    rc_monitor: Rect,
    rc_work: Rect,
    flags: u32,
}
#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn EnumWindows(cb: extern "system" fn(isize, isize) -> i32, lparam: isize) -> i32;
    fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
    fn IsWindowVisible(hwnd: isize) -> i32;
    fn GetForegroundWindow() -> isize;
    fn GetWindowRect(hwnd: isize, r: *mut Rect) -> i32;
    fn MonitorFromWindow(hwnd: isize, flags: u32) -> isize;
    fn GetMonitorInfoW(mon: isize, mi: *mut MonInfo) -> i32;
    fn GetWindowLongPtrW(hwnd: isize, idx: i32) -> isize;
    fn SetWindowLongPtrW(hwnd: isize, idx: i32, new: isize) -> isize;
}
#[cfg(windows)]
extern "system" {
    fn GetCurrentProcessId() -> u32;
}
#[cfg(windows)]
#[repr(C)]
struct CursorPos {
    x: i32,
    y: i32,
}
#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn GetCursorPos(p: *mut CursorPos) -> i32;
    fn GetAsyncKeyState(vk: i32) -> i16;
}

// The island's vertical panel needs the bar's window to be taller than the bar
// strip. Growing a full-width window would swallow every click in the empty
// space beside the bubble, so the window's INPUT+PAINT region is clipped to
// exactly (bar strip) + (bubble): anything outside stays click-through to the
// desktop. This is what makes a drop-down panel possible without a second
// always-on-top window fighting the tiling WM for focus.
#[cfg(windows)]
#[link(name = "gdi32")]
extern "system" {
    fn CreateRectRgn(l: i32, t: i32, r: i32, b: i32) -> isize;
    fn CreateRoundRectRgn(l: i32, t: i32, r: i32, b: i32, w: i32, h: i32) -> isize;
    fn CombineRgn(dst: isize, a: isize, b: isize, mode: i32) -> i32;
    fn DeleteObject(o: isize) -> i32;
}
#[cfg(windows)]
extern "system" {
    fn SetWindowRgn(hwnd: isize, rgn: isize, redraw: i32) -> i32;
    fn SetWindowPos(hwnd: isize, after: isize, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32;
}

/// Clip the window to the bar strip plus an optional bubble.
///
/// Coordinates are client-relative. Passing `bubble = None` restores the plain
/// full-width bar. The region handle is owned by the system after SetWindowRgn,
/// so it must not be freed here.
#[cfg(windows)]
unsafe fn set_window_shape(hwnd: isize, width: i32, bar_h: i32, bubble: Option<(i32, i32, i32)>) {
    const RGN_OR: i32 = 2;
    let strip = CreateRectRgn(0, 0, width, bar_h);
    match bubble {
        Some((left, right, bottom)) if bottom > bar_h => {
            // Rounded, and overlapping the strip by a few px so the bubble reads
            // as growing out of the bar rather than as a detached box.
            let bub = CreateRoundRectRgn(left, bar_h - 6, right, bottom, 26, 26);
            CombineRgn(strip, strip, bub, RGN_OR);
            DeleteObject(bub);
        }
        _ => {}
    }
    // redraw = FALSE. With TRUE, Windows repaints the whole window frame as part
    // of applying the region, and that repaint briefly exposes the non-client
    // area -- a full-width light title bar flashing across the top of the screen
    // for one frame. Isolated by clicking: a click on empty bar never flashed, a
    // click that opened the panel did, and it survived removing the resize, which
    // left this call as the only thing that changes on open. egui repaints the
    // client area on the same frame anyway, so nothing is lost.
    SetWindowRgn(hwnd, strip, 0);
}
// Identify OUR bar window by geometry, not by "first visible window of this
// process". winit/eframe also keep small helper windows alive -- there are two
// visible 16x16 ones at (0,0) -- and EnumWindows walks in Z-order, so the naive
// version could hand back a helper. Click-through was then applied to a 16x16
// window while the real bar kept swallowing clicks, which is why it worked only
// sometimes.
#[cfg(windows)]
struct FindCtx {
    want_x: i32,
    want_w: i32,
    found: isize,
}

#[cfg(windows)]
extern "system" fn find_cb(hwnd: isize, lparam: isize) -> i32 {
    unsafe {
        let ctx = &mut *(lparam as *mut FindCtx);
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid != GetCurrentProcessId() || IsWindowVisible(hwnd) == 0 {
            return 1;
        }
        let mut r = Rect { left: 0, top: 0, right: 0, bottom: 0 };
        if GetWindowRect(hwnd, &mut r) == 0 {
            return 1;
        }
        // The bar: our x, our width (a few px of slack for DPI rounding), and
        // sitting at the top of the screen.
        let w = r.right - r.left;
        if (r.left - ctx.want_x).abs() <= 4 && (w - ctx.want_w).abs() <= 8 && r.top < 60 {
            ctx.found = hwnd;
            return 0;
        }
    }
    1
}

#[cfg(windows)]
fn find_own_window(x: i32, width: i32) -> isize {
    let mut ctx = FindCtx { want_x: x, want_w: width, found: 0 };
    unsafe { EnumWindows(find_cb, &mut ctx as *mut FindCtx as isize) };
    ctx.found
}
#[cfg(windows)]
unsafe fn fullscreen_on_monitor(my: isize) -> bool {
    let mon = MonitorFromWindow(my, 2 /* NEAREST */);
    let mut mi = MonInfo {
        cb: std::mem::size_of::<MonInfo>() as u32,
        rc_monitor: Rect { left: 0, top: 0, right: 0, bottom: 0 },
        rc_work: Rect { left: 0, top: 0, right: 0, bottom: 0 },
        flags: 0,
    };
    if GetMonitorInfoW(mon, &mut mi) == 0 {
        return false;
    }
    let fg = GetForegroundWindow();
    if fg == 0 || fg == my {
        return false;
    }
    let mut r = Rect { left: 0, top: 0, right: 0, bottom: 0 };
    if GetWindowRect(fg, &mut r) == 0 {
        return false;
    }
    // A tiled/maximised window sits BELOW the bar; only a true fullscreen window
    // covers the monitor's top strip too. A few pixels of slack, because some
    // borderless windows land a hair inside the monitor rect and an exact
    // comparison then misses them.
    const SLACK: i32 = 4;
    r.left <= mi.rc_monitor.left + SLACK
        && r.top <= mi.rc_monitor.top + SLACK
        && r.right >= mi.rc_monitor.right - SLACK
        && r.bottom >= mi.rc_monitor.bottom - SLACK
}

/// Should the bar ignore mouse input right now?
///
/// Geometry alone isn't enough: a game in *borderless* mode can sit short of the
/// monitor rect or keep a child window focused, so the focused executable is
/// also checked against `clickthrough_apps` in ~/.config/rice.json. The bar
/// stays fully visible either way -- only hit-testing changes.
#[cfg(windows)]
unsafe fn should_clickthrough(my: isize) -> bool {
    if fullscreen_on_monitor(my) {
        return true;
    }
    let apps = &rice_common::settings::Settings::get().clickthrough_apps;
    if apps.is_empty() {
        return false;
    }
    // Only the bar on the game's own monitor goes click-through. Without this,
    // focusing the game would also disarm the bar on the second screen, where
    // there is nothing to click through to.
    let fg = GetForegroundWindow();
    if fg == 0 || fg == my || MonitorFromWindow(fg, 2) != MonitorFromWindow(my, 2) {
        return false;
    }
    match win::foreground_process_name() {
        Some(name) => apps.iter().any(|a| name.contains(&a.to_lowercase())),
        None => false,
    }
}
/// Take the native frame off for good. See the note at the call site: the
/// styles survive with_decorations(false) and flash during resizes.
#[cfg(windows)]
unsafe fn strip_native_frame(hwnd: isize) {
    const WS_CAPTION: isize = 0x00C0_0000;
    const WS_THICKFRAME: isize = 0x0004_0000;
    const WS_SYSMENU: isize = 0x0008_0000;
    const WS_MINIMIZEBOX: isize = 0x0002_0000;
    const WS_MAXIMIZEBOX: isize = 0x0001_0000;
    const GWL_STYLE: i32 = -16;
    const UNWANTED: isize = WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX;
    let cur = GetWindowLongPtrW(hwnd, GWL_STYLE);
    if cur & UNWANTED == 0 {
        return;
    }
    SetWindowLongPtrW(hwnd, GWL_STYLE, cur & !UNWANTED);
    // SWP_FRAMECHANGED, or Windows keeps using the old non-client metrics.
    const SWP: u32 = 0x0002 | 0x0001 | 0x0004 | 0x0020 | 0x0010; // NOMOVE NOSIZE NOZORDER FRAMECHANGED NOACTIVATE
    SetWindowPos(hwnd, 0, 0, 0, 0, 0, SWP);
    // DWMWA_NCRENDERING_POLICY = DWMNCRP_DISABLED.
    const DWMWA_NCRENDERING_POLICY: u32 = 2;
    const DWMNCRP_DISABLED: i32 = 1;
    let policy = DWMNCRP_DISABLED;
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_NCRENDERING_POLICY,
        &policy as *const i32 as *const core::ffi::c_void,
        core::mem::size_of::<i32>() as u32,
    );
}

#[cfg(windows)]
#[link(name = "dwmapi")]
extern "system" {
    fn DwmSetWindowAttribute(hwnd: isize, attr: u32, val: *const core::ffi::c_void, len: u32) -> i32;
}

#[cfg(windows)]
unsafe fn set_clickthrough(hwnd: isize, on: bool) {
    const GWL_EXSTYLE: i32 = -20;
    const WS_EX_TRANSPARENT: isize = 0x20;
    let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
    let new = if on { ex | WS_EX_TRANSPARENT } else { ex & !WS_EX_TRANSPARENT };
    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new);
}

// Environment flags are read once, not per call / per frame: dlog ran an
// env::var_os on every invocation and the icon-test check ran one every frame.
fn env_flag(name: &'static str, cell: &'static std::sync::OnceLock<bool>) -> bool {
    *cell.get_or_init(|| std::env::var_os(name).is_some())
}
static LOG_ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
static ICONTEST_ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

// Debug log to %TEMP%\glaze-bar.log when GLAZEBAR_LOG is set.
fn dlog(msg: &str) {
    if env_flag("GLAZEBAR_LOG", &LOG_ON) {
        if let Ok(dir) = std::env::var("TEMP") {
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(format!("{dir}\\glaze-bar.log"))
            {
                let _ = writeln!(f, "{msg}");
            }
        }
    }
}

// ---- GlazeWM IPC types ----
#[derive(Deserialize, Clone, Default)]
struct Workspace {
    #[serde(default)]
    name: String,
    #[serde(default, rename = "displayName")]
    display_name: Option<String>,
    #[serde(default, rename = "hasFocus")]
    has_focus: bool,
    #[serde(default, rename = "isDisplayed")]
    is_displayed: bool,
}
#[derive(Deserialize, Default)]
struct Monitor {
    #[serde(default)]
    x: i32,
    #[serde(default)]
    children: Vec<Workspace>,
}
#[derive(Deserialize)]
struct MonData {
    monitors: Vec<Monitor>,
}
#[derive(Deserialize)]
struct MonResp {
    data: Option<MonData>,
}
#[derive(Deserialize)]
struct TdData {
    #[serde(rename = "tilingDirection")]
    tiling_direction: Option<String>,
}
#[derive(Deserialize)]
struct TdResp {
    data: Option<TdData>,
}
#[derive(Deserialize, Default)]
struct BMode {
    #[serde(default)]
    name: String,
    #[serde(default, rename = "displayName")]
    display_name: Option<String>,
}
#[derive(Deserialize)]
struct BmData {
    #[serde(rename = "bindingModes")]
    binding_modes: Vec<BMode>,
}
#[derive(Deserialize)]
struct BmResp {
    data: Option<BmData>,
}

// ---- warm palette (shared with the toast via rice_common::theme) ----
const BAR_BG: egui::Color32 = col(theme::BAR_BG);
const ISL_SURFACE: egui::Color32 = col(theme::SURFACE); // raised warm pill
const ISL_HI: egui::Color32 = col(theme::HIGHLIGHT); // top highlight edge
const WARM_TEXT: egui::Color32 = col(theme::TEXT);
const WARM_SUB: egui::Color32 = col(theme::SUBTEXT);
const WARM_ACCENT: egui::Color32 = col(theme::ACCENT); // amber

// A transient context the dynamic island morphs to show (written to the event
// file by the save step / mic command, same content model as the toast).
#[derive(Clone, Default)]
struct IslandEvent {
    icon: String,
    title: String,
    body: String,
    accent: [u8; 3],
}

// ---- Shared state, written by worker threads, read by the UI ----
#[derive(Default)]
struct Shared {
    workspaces: Vec<Workspace>,
    tiling: String,
    mode: String,
    cpu: f32,
    mem: f32,
    gpu: String, // "44° 11%" (temp + utilization, from nvidia-smi)
    net: String, // throughput "↓1.2M ↑0.3M"
    island: Option<IslandEvent>,
    island_serial: u64, // bumps on each new event so the UI notices
}

// parse_hex, the opacity helpers, the icon table and draw_icon all live in
// rice_common now (theme / config / ui) so the bar and the toast share one copy.
use rice_common::theme::parse_hex;
use rice_common::ui::icon_glyph;

fn read_opacity(name: &str, default: f32) -> f32 {
    config::read_opacity(name, default)
}
fn write_opacity(name: &str, v: f32) {
    config::write_opacity(name, v)
}

// Quick-access buttons shown when the island is expanded: (action, glyph, accent).
// Monitor brightness over DDC/CI (what Twinkle Tray does). Each set() talks to
// the display's controller over the cable and takes tens of milliseconds, so it
// runs on a worker thread and only the LATEST value is applied -- dragging a
// slider produces far more updates than a monitor can absorb, and queueing them
// would make the panel lag seconds behind the handle.
enum BrightMsg {
    /// Re-read every display's current brightness.
    Refresh,
    /// Apply a 0..1 fraction to one display.
    Set(isize, f32),
}

struct BrightCtl {
    tx: std::sync::mpsc::Sender<BrightMsg>,
    /// Latest known state, published by the worker: (hmonitor, label, 0..1).
    state: Arc<Mutex<Vec<(isize, String, f32)>>>,
}

impl BrightCtl {
    fn spawn(ctx: egui::Context) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<BrightMsg>();
        let state = Arc::new(Mutex::new(Vec::new()));
        let out = state.clone();
        std::thread::spawn(move || {
            let mut displays: Vec<rice_common::brightness::Display> = Vec::new();
            while let Ok(first) = rx.recv() {
                // Coalesce: a slider drag emits far more updates than a monitor can
                // absorb, and queueing them would leave the panel seconds behind the
                // handle. Keep only the newest value per display.
                let mut refresh = matches!(first, BrightMsg::Refresh);
                let mut pending = std::collections::HashMap::new();
                if let BrightMsg::Set(h, v) = first {
                    pending.insert(h, v);
                }
                while let Ok(m) = rx.try_recv() {
                    match m {
                        BrightMsg::Refresh => refresh = true,
                        BrightMsg::Set(h, v) => {
                            pending.insert(h, v);
                        }
                    }
                }
                if refresh || displays.is_empty() {
                    displays = rice_common::brightness::displays();
                    *out.lock().unwrap() = displays
                        .iter()
                        .enumerate()
                        .map(|(i, d)| (d.hmonitor, format!("{}", i + 1), d.fraction()))
                        .collect();
                    ctx.request_repaint();
                }
                for (hmon, v) in pending {
                    if let Some(d) = displays.iter_mut().find(|d| d.hmonitor == hmon) {
                        rice_common::brightness::set(d, v);
                        let span = (d.max.saturating_sub(d.min)) as f32;
                        d.current = d.min + (v.clamp(0.0, 1.0) * span).round() as u32;
                    }
                }
            }
        });
        Self { tx, state }
    }
    fn refresh(&self) {
        let _ = self.tx.send(BrightMsg::Refresh);
    }
    fn set(&self, hmonitor: isize, fraction: f32) {
        let _ = self.tx.send(BrightMsg::Set(hmonitor, fraction));
    }
}

// Master + per-application volume, and the output-device toggle. Same shape as
// BrightCtl: every Core Audio call is a COM round trip costing milliseconds, so
// it lives on a worker and the UI only ever reads a published snapshot.
enum VolMsg {
    Refresh,
    /// Empty name = master.
    Set(String, f32),
    /// Flip the default output between the two devices actually in use.
    SwapOutput,
}

/// One row of the widget: label, 0..1 level. The first row is always master.
type VolRow = (String, f32);

struct VolCtl {
    tx: std::sync::mpsc::Sender<VolMsg>,
    state: Arc<Mutex<Vec<VolRow>>>,
}

impl VolCtl {
    fn spawn(ctx: egui::Context) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<VolMsg>();
        let state = Arc::new(Mutex::new(Vec::new()));
        let out = state.clone();
        std::thread::spawn(move || {
            while let Ok(first) = rx.recv() {
                // Coalesce a drag into one write per target, newest wins.
                let mut refresh = matches!(first, VolMsg::Refresh);
                let mut swap = matches!(first, VolMsg::SwapOutput);
                let mut pending: std::collections::HashMap<String, f32> = Default::default();
                if let VolMsg::Set(n, v) = first {
                    pending.insert(n, v);
                }
                while let Ok(m) = rx.try_recv() {
                    match m {
                        VolMsg::Refresh => refresh = true,
                        VolMsg::SwapOutput => swap = true,
                        VolMsg::Set(n, v) => {
                            pending.insert(n, v);
                        }
                    }
                }
                for (name, v) in pending {
                    if name.is_empty() {
                        rice_common::audio::set_master_volume(v);
                    } else {
                        rice_common::audio::set_app_volume(&name, v);
                    }
                }
                if swap {
                    // micswitch owns the IPolicyConfig dance; reuse it rather
                    // than duplicating that undocumented COM call here.
                    let exe = win::sibling_exe("micswitch.exe");
                    let cur = rice_common::audio::current_output_name().unwrap_or_default();
                    let target = if cur.to_lowercase().contains("hyperx") { "VG270" } else { "HyperX" };
                    use std::os::windows::process::CommandExt;
                    let _ = std::process::Command::new(exe)
                        .args(["--output", "--set", target])
                        .creation_flags(win::CREATE_NO_WINDOW)
                        .output();
                    refresh = true;
                }
                if refresh || swap {
                    let mut rows: Vec<VolRow> =
                        vec![("master".into(), rice_common::audio::master_volume().unwrap_or(0.0))];
                    for s in rice_common::audio::sessions().into_iter().take(4) {
                        rows.push((s.name.trim_end_matches(".exe").to_string(), s.volume));
                    }
                    *out.lock().unwrap() = rows;
                    ctx.request_repaint();
                }
            }
        });
        Self { tx, state }
    }
    fn refresh(&self) {
        let _ = self.tx.send(VolMsg::Refresh);
    }
    fn set(&self, name: &str, v: f32) {
        let _ = self.tx.send(VolMsg::Set(name.to_string(), v));
    }
    fn swap_output(&self) {
        let _ = self.tx.send(VolMsg::SwapOutput);
    }
}

/// One row on the device page: either a plain playback endpoint or a Bluetooth
/// device that may need connecting first.
#[derive(Clone)]
struct DevRow {
    label: String,
    /// Endpoint to make default. None for a Bluetooth device that is offline --
    /// it has no usable endpoint until it connects.
    endpoint: Option<String>,
    /// Set for Bluetooth devices, so the row can offer connect/disconnect.
    bt_container: Option<u128>,
    connected: bool,
    is_default: bool,
    /// An operation is in flight; the row is greyed and ignores clicks so a
    /// second press cannot queue another multi-second connect.
    busy: bool,
}

enum DevMsg {
    Refresh,
    /// Make this endpoint the default output.
    Select(String),
    /// Connect a Bluetooth device, then make it default once it is really up.
    Connect(u128),
    Disconnect(u128),
}

/// Device list + switching, off the render thread. Connecting a Bluetooth
/// headset takes seconds and the endpoint only becomes usable asynchronously
/// afterwards, so none of this can happen inline.
struct DevCtl {
    tx: std::sync::mpsc::Sender<DevMsg>,
    state: Arc<Mutex<Vec<DevRow>>>,
}

impl DevCtl {
    fn new(ctx: egui::Context) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<DevMsg>();
        let state: Arc<Mutex<Vec<DevRow>>> = Arc::new(Mutex::new(Vec::new()));
        let out = state.clone();
        std::thread::spawn(move || {
            while let Ok(msg) = rx.recv() {
                match msg {
                    DevMsg::Refresh => {}
                    DevMsg::Select(id) => {
                        rice_common::audio::set_default_output(&id);
                    }
                    DevMsg::Connect(container) => {
                        mark_busy(&out, container, true);
                        ctx.request_repaint();
                        let asked = rice_common::bluetooth::devices()
                            .iter()
                            .find(|d| d.container == container)
                            .map(|d| d.connect())
                            .unwrap_or(false);
                        // A successful call only means the driver tried, so wait
                        // for the endpoint to actually come up before selecting
                        // it -- setting a still-unplugged device as default is
                        // silently a no-op.
                        if asked
                            && rice_common::bluetooth::wait_connected(
                                container,
                                Duration::from_secs(12),
                            )
                        {
                            if let Some(id) = rice_common::bluetooth::devices()
                                .iter()
                                .find(|d| d.container == container)
                                .and_then(|d| d.output_id.clone())
                            {
                                rice_common::audio::set_default_output(&id);
                            }
                        }
                    }
                    DevMsg::Disconnect(container) => {
                        mark_busy(&out, container, true);
                        ctx.request_repaint();
                        if let Some(d) = rice_common::bluetooth::devices()
                            .iter()
                            .find(|d| d.container == container)
                        {
                            d.disconnect();
                        }
                    }
                }
                *out.lock().unwrap() = build_device_rows();
                ctx.request_repaint();
            }
        });
        Self { tx, state }
    }
    fn refresh(&self) {
        let _ = self.tx.send(DevMsg::Refresh);
    }
    fn rows(&self) -> Vec<DevRow> {
        self.state.lock().unwrap().clone()
    }
    fn select(&self, id: String) {
        let _ = self.tx.send(DevMsg::Select(id));
    }
    fn connect(&self, container: u128) {
        let _ = self.tx.send(DevMsg::Connect(container));
    }
    fn disconnect(&self, container: u128) {
        let _ = self.tx.send(DevMsg::Disconnect(container));
    }
}

fn mark_busy(state: &Arc<Mutex<Vec<DevRow>>>, container: u128, busy: bool) {
    for r in state.lock().unwrap().iter_mut() {
        if r.bt_container == Some(container) {
            r.busy = busy;
        }
    }
}

/// The configured outputs, plus every Bluetooth audio device.
fn build_device_rows() -> Vec<DevRow> {
    let cfg = rice_common::settings::Settings::live();
    let current = rice_common::audio::current_output_id().unwrap_or_default();
    let endpoints = rice_common::audio::outputs(true);
    let bt = rice_common::bluetooth::devices();
    let bt_containers: Vec<u128> = bt.iter().map(|d| d.container).collect();

    let mut rows: Vec<DevRow> = Vec::new();

    // Configured wired/virtual outputs, in the order they are listed.
    for want in &cfg.outputs {
        let want = want.to_lowercase();
        let matches: Vec<&rice_common::audio::Endpoint> = endpoints
            .iter()
            .filter(|e| {
                e.name.to_lowercase().contains(&want)
                    // Bluetooth devices get their own row below; don't list twice.
                    && !e.container.map(|c| bt_containers.contains(&c)).unwrap_or(false)
            })
            .collect();
        // One name can match several endpoints -- this machine reports the same
        // NVIDIA HDMI output half a dozen times. Prefer the one that is actually
        // the current default, then any live one, so the tick lands on the right
        // row instead of on an identically named dead instance.
        let Some(e) = matches
            .iter()
            .find(|e| e.id == current)
            .or_else(|| matches.iter().find(|e| e.active))
            .or_else(|| matches.first())
        else {
            continue;
        };
        rows.push(DevRow {
            label: short_output_name(&e.name, &want),
            endpoint: Some(e.id.clone()),
            bt_container: None,
            connected: e.active,
            is_default: e.id == current,
            busy: false,
        });
    }

    for d in bt {
        let is_default = d.output_id.as_deref() == Some(current.as_str());
        rows.push(DevRow {
            label: d.name.clone(),
            endpoint: d.output_id.clone(),
            bt_container: Some(d.container),
            connected: d.connected,
            is_default,
            busy: false,
        });
    }
    rows
}

/// Trim a Windows endpoint name down to the part a human would say.
///
/// Which half that is depends on the device, so the configured match string
/// decides: `Altavoces (HyperX Cloud II Wireless)` matched on "hyperx" keeps the
/// text in brackets, while `VG270 V (NVIDIA High Definition Audio)` matched on
/// "vg270" keeps the text before them -- always taking the bracketed half would
/// label the monitor "NVIDIA High Definition Audio".
fn short_output_name(raw: &str, want: &str) -> String {
    let (Some(a), Some(b)) = (raw.find('('), raw.rfind(')')) else {
        return raw.trim().to_string();
    };
    if b <= a + 1 {
        return raw.trim().to_string();
    }
    let inside = raw[a + 1..b].trim();
    let outside = raw[..a].trim();
    if !want.is_empty() && !inside.to_lowercase().contains(want) && outside.to_lowercase().contains(want) {
        return outside.to_string();
    }
    if inside.is_empty() { outside.to_string() } else { inside.to_string() }
}

const ACTIONS: [(&str, &str, [u8; 3]); 8] = [
    ("mic", "\u{f130}", [224, 163, 92]),      // switch mic
    ("save", "\u{f03d}", [169, 181, 106]),    // save a replay clip
    ("term", "\u{f120}", [206, 150, 112]),    // open a terminal
    ("opacity", "\u{f1de}", [200, 172, 150]), // fa-sliders -> opacity widget
    ("bright", "\u{f185}", [230, 190, 120]),  // fa-sun -> monitor brightness (DDC/CI)
    ("vol", "\u{f028}", [150, 190, 200]),     // fa-volume-up -> master + per-app volume
    ("timer", "\u{f017}", [205, 150, 170]),   // fa-clock-o -> pomodoro
    ("devices", "\u{f025}", [150, 200, 190]), // fa-headphones -> outputs + bluetooth
];

// Draw an opacity slider track (bg + accent fill + handle) at a normalized value t.
fn draw_track(p: &egui::Painter, tl: f32, tw: f32, cy: f32, t: f32, acc: egui::Color32) {
    let track = egui::Rect::from_min_max(egui::pos2(tl, cy - 3.0), egui::pos2(tl + tw, cy + 3.0));
    p.rect_filled(track, egui::Rounding::same(3.0), egui::Color32::from_rgba_unmultiplied(WARM_SUB.r(), WARM_SUB.g(), WARM_SUB.b(), 55));
    p.rect_filled(
        egui::Rect::from_min_max(track.min, egui::pos2(tl + tw * t, track.max.y)),
        egui::Rounding::same(3.0),
        acc,
    );
    p.circle_filled(egui::pos2(tl + tw * t, cy), 5.5, acc);
}

// Run a quick-action off the UI thread. "mic" also pushes its result to the island.
fn run_action(kind: &str, shared: Arc<Mutex<Shared>>, ctx: egui::Context) {
    use std::os::windows::process::CommandExt;
    const NOWIN: u32 = 0x0800_0000;
    let kind = kind.to_string();
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    let dir = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf()));
    std::thread::spawn(move || match kind.as_str() {
        "mic" => {
            let Some(d) = dir else { return };
            if let Ok(out) = std::process::Command::new(d.join("micswitch.exe"))
                .creation_flags(NOWIN)
                .output()
            {
                let name = String::from_utf8_lossy(&out.stdout);
                let body = name
                    .trim()
                    .trim_start_matches("Micrófono (")
                    .trim_end_matches(')')
                    .to_string();
                if !body.is_empty() {
                    let mut s = shared.lock().unwrap();
                    s.island = Some(IslandEvent {
                        icon: "mic".into(),
                        title: "Micrófono".into(),
                        body,
                        accent: [224, 163, 92],
                    });
                    s.island_serial += 1;
                    drop(s);
                    ctx.request_repaint();
                }
            }
        }
        "save" => {
            let _ = std::process::Command::new("pwsh")
                .args([
                    "-NoProfile",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-WindowStyle",
                    "Hidden",
                    "-File",
                    &format!("{home}\\.config\\shadowplay-wgc-save.ps1"),
                ])
                .creation_flags(NOWIN)
                .spawn();
        }
        "term" => {
            let _ = std::process::Command::new("C:\\Program Files\\WezTerm\\wezterm-gui.exe")
                .arg("start")
                .spawn();
        }
        _ => {}
    });
}

// Watch ~/.config/island.json; on change, push it as the island's current event.
fn island_watcher(shared: Arc<Mutex<Shared>>, ctx: egui::Context) {
    #[derive(Deserialize, Default)]
    struct F {
        #[serde(default)]
        icon: String,
        #[serde(default)]
        title: String,
        #[serde(default)]
        body: String,
        #[serde(default)]
        accent: String,
    }
    let path = std::env::var("USERPROFILE")
        .map(|h| format!("{h}\\.config\\island.json"))
        .unwrap_or_default();
    // Start from the file's current mtime so a stale event doesn't fire on launch.
    let mut last = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    loop {
        if let Ok(mt) = std::fs::metadata(&path).and_then(|m| m.modified()) {
            if mt != last {
                last = mt;
                if let Ok(f) = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|t| serde_json::from_str::<F>(&t).ok())
                    .ok_or(())
                {
                    let mut s = shared.lock().unwrap();
                    s.island = Some(IslandEvent {
                        icon: f.icon,
                        title: f.title,
                        body: f.body,
                        accent: parse_hex(&f.accent).unwrap_or([224, 163, 92]),
                    });
                    s.island_serial += 1;
                    drop(s);
                    ctx.request_repaint();
                }
            }
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

// Fire-and-forget IPC command (e.g. clicking a workspace pill to focus it).
fn ipc_command(cmd: String) {
    std::thread::spawn(move || {
        if let Ok((mut sock, _)) = tungstenite::connect("ws://127.0.0.1:6123") {
            let _ = sock.send(tungstenite::Message::Text(cmd.into()));
            let _ = sock.read(); // wait for the ack so it's processed
            let _ = sock.close(None);
        }
    });
}

fn human_rate(bytes_per_sec: f64) -> String {
    if bytes_per_sec >= 1_000_000.0 {
        format!("{:.1}M", bytes_per_sec / 1_000_000.0)
    } else if bytes_per_sec >= 1_000.0 {
        format!("{:.0}K", bytes_per_sec / 1_000.0)
    } else {
        format!("{:.0}B", bytes_per_sec)
    }
}

// Send a query, return the first text response (no subscriptions => next text
// message is the response).
fn query<S: Read + Write>(sock: &mut tungstenite::WebSocket<S>, msg: &str) -> Option<String> {
    sock.send(tungstenite::Message::Text(msg.into())).ok()?;
    loop {
        match sock.read().ok()? {
            tungstenite::Message::Text(t) => return Some(t.to_string()),
            tungstenite::Message::Close(_) => return None,
            _ => continue,
        }
    }
}

fn ipc_thread(shared: Arc<Mutex<Shared>>, my_x: i32, ctx: egui::Context) {
    loop {
        match tungstenite::connect("ws://127.0.0.1:6123") {
            Ok((mut sock, _)) => loop {
                // Workspaces for the monitor this bar lives on.
                let Some(txt) = query(&mut sock, "query monitors") else { break };
                if let Ok(r) = serde_json::from_str::<MonResp>(&txt) {
                    if let Some(d) = r.data {
                        if let Some(mon) = d
                            .monitors
                            .into_iter()
                            .min_by_key(|m| (m.x - my_x).abs())
                        {
                            shared.lock().unwrap().workspaces = mon.children;
                        }
                    }
                }
                if let Some(txt) = query(&mut sock, "query tiling-direction") {
                    if let Ok(r) = serde_json::from_str::<TdResp>(&txt) {
                        if let Some(d) = r.data {
                            shared.lock().unwrap().tiling = d.tiling_direction.unwrap_or_default();
                        }
                    }
                }
                if let Some(txt) = query(&mut sock, "query binding-modes") {
                    if let Ok(r) = serde_json::from_str::<BmResp>(&txt) {
                        if let Some(d) = r.data {
                            shared.lock().unwrap().mode = d
                                .binding_modes
                                .first()
                                .map(|m| m.display_name.clone().unwrap_or_else(|| m.name.clone()))
                                .unwrap_or_default();
                        }
                    }
                }
                ctx.request_repaint();
                std::thread::sleep(Duration::from_millis(300));
            },
            Err(_) => std::thread::sleep(Duration::from_secs(2)),
        }
    }
}

fn sys_thread(shared: Arc<Mutex<Shared>>, ctx: egui::Context) {
    let mut sys = sysinfo::System::new();
    let mut nets = sysinfo::Networks::new_with_refreshed_list();
    let mut last = Instant::now();
    loop {
        sys.refresh_cpu_usage();
        std::thread::sleep(Duration::from_millis(500));
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        let cpu = sys.global_cpu_usage();
        let total = sys.total_memory();
        let mem = if total > 0 {
            sys.used_memory() as f32 / total as f32 * 100.0
        } else {
            0.0
        };

        // Network throughput: bytes since the last refresh / elapsed time.
        nets.refresh();
        let now = Instant::now();
        let secs = now.duration_since(last).as_secs_f64().max(0.001);
        last = now;
        let (mut rx, mut tx) = (0u64, 0u64);
        for (_iface, data) in &nets {
            rx += data.received();
            tx += data.transmitted();
        }
        let net = format!(
            "↓{} ↑{}",
            human_rate(rx as f64 / secs),
            human_rate(tx as f64 / secs)
        );

        {
            let mut s = shared.lock().unwrap();
            s.cpu = cpu;
            s.mem = mem;
            s.net = net;
        }
        ctx.request_repaint();
        std::thread::sleep(Duration::from_millis(1500));
    }
}

// GPU temperature + utilization via nvidia-smi (no admin needed).
//
// Every sample is a process spawn, so the interval is 10s rather than 3s: that
// is 8,640 spawns a day instead of 28,800, for a reading whose useful resolution
// is nowhere near 3 seconds. Repaint only when the string actually changed --
// the bar was being woken up 20 times a minute to redraw identical text.
fn gpu_thread(shared: Arc<Mutex<Shared>>, ctx: egui::Context) {
    loop {
        if let Some(g) = fetch_gpu() {
            let mut s = shared.lock().unwrap();
            if s.gpu != g {
                s.gpu = g;
                drop(s);
                ctx.request_repaint();
            }
        }
        std::thread::sleep(Duration::from_secs(10));
    }
}
fn fetch_gpu() -> Option<String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = win::CREATE_NO_WINDOW;
        let out = std::process::Command::new("nvidia-smi")
            .args([
                "--query-gpu=temperature.gpu,utilization.gpu",
                "--format=csv,noheader,nounits",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()?;
        let s = String::from_utf8_lossy(&out.stdout);
        let line = s.lines().next()?;
        let mut parts = line.split(',').map(|x| x.trim());
        let temp = parts.next()?;
        let util = parts.next()?;
        let g = format!("{temp}° {util}%");
        dlog(&format!("gpu = {g}"));
        return Some(g);
    }
    #[allow(unreachable_code)]
    None
}

// JetBrainsMono Nerd Font loading lives in rice_common::ui (shared with the toast).
use rice_common::ui::load_nerd_font as load_font;

struct BarApp {
    shared: Arc<Mutex<Shared>>,
    width: f32,
    /// Left edge of this bar's monitor (the --x argument), used to identify our
    /// own window among the process's several.
    x: i32,
    sized: bool,
    frame: u32,
    // dynamic island animation state
    isl_w: f32,                            // current (animated) pill width
    isl_h: f32,                            // current (animated) pill height
    isl_serial: u64,                       // last event serial consumed
    isl_notif: Option<(IslandEvent, Instant)>, // active notification + shown time
    isl_expanded: bool,                    // quick-action buttons shown
    isl_interact: Instant,                 // last hover/click (for auto-collapse)
    last_frame: Instant,
    ws_ind: Option<egui::Rect>,            // animated focused-workspace highlight (slides on switch)
    // auto click-through when a fullscreen game covers this monitor
    hwnd: isize,
    clickthrough: bool,
    last_ct: Instant,
    // live-adjustable translucency (island opacity widget)
    bar_opacity: f32,
    term_opacity: f32,
    isl_opacity: bool, // opacity-adjust widget shown
    // Monitor brightness (DDC/CI). `bright` is queried when the widget opens --
    // never per frame, since each query costs tens of ms per display.
    isl_bright: bool,
    bright: Vec<(isize, String, f32)>, // (hmonitor, label, 0..1)
    bright_ctl: BrightCtl,
    dev_ctl: DevCtl,
    // Volume: master + up to four apps, queried when the widget opens.
    isl_vol: bool,
    vol: Vec<VolRow>,
    vol_ctl: VolCtl,
    // ---- vertical drop-down panel (the "bubble") ----
    // Height animates with a spring so it overshoots and settles, the way the
    // real Dynamic Island does; `panel_v` is that spring's velocity.
    panel_h: f32,
    panel_p: f32,            // monotonic 0..1 open progress (no overshoot) for text
    panel_v: f32,
    panel_shape: (i32, i32), // last shape applied (height, bubble width) -> avoid redundant Win32 calls
    // ---- media: what is playing, plus a live spectrum for the pill ----
    spectrum: rice_common::spectrum::Spectrum,
    media: Option<rice_common::media::NowPlaying>,
    // Written by a background poller: SMTC calls are far too slow to make from
    // the render thread.
    media_rx: Arc<Mutex<Option<rice_common::media::NowPlaying>>>,
    isl_media: bool,   // bubble is showing the media view
    // ---- pomodoro ----
    isl_timer: bool,
    isl_devices: bool,
    timer_left: Duration,
    timer_total: Duration,
    timer_running: bool,
    timer_tick: Instant,
    win_h: i32,              // last window height requested
    panel_rect: Option<egui::Rect>, // where the bubble is, for outside-click hit testing
    last_opacity_write: Instant,
}

const ISL_HOLD: f32 = 4.0; // seconds a notification stays before collapsing


impl BarApp {
    /// Which sub-view the panel is showing.
    fn panel_mode(&self) -> u8 {
        if self.isl_vol {
            1
        } else if self.isl_bright {
            2
        } else if self.isl_opacity {
            3
        } else if self.isl_media {
            4
        } else if self.isl_timer {
            5
        } else if self.isl_devices {
            6
        } else {
            0
        }
    }

    fn panel_width(&self) -> f32 {
        match self.panel_mode() {
            1 => (self.vol.len().max(1) as f32 * 46.0 + 70.0).max(200.0),
            2 => (self.bright.len().max(1) as f32 * 46.0 + 40.0).max(160.0),
            3 => 160.0,
            4 => 300.0,
            5 => 230.0,
            6 => 300.0,
            _ => 3.0 * 52.0 + 24.0,
        }
    }

    /// Resting height of the bubble for the current sub-view.
    fn panel_target_h(&self) -> f32 {
        match self.panel_mode() {
            1 | 2 | 3 => 140.0,
            4 => 132.0,
            5 => 138.0,
            6 => {
                // One row per device, plus the header line.
                let n = self.dev_ctl.rows().len().max(1) as f32;
                (n * 38.0 + 34.0).min(230.0)
            }
            _ => (ACTIONS.len() as f32 / 3.0).ceil() * 52.0 + 22.0,
        }
    }

    fn bar_h(&self) -> f32 {
        rice_common::settings::Settings::get().bar_height as f32
    }

    fn close_panel(&mut self) {
        self.isl_expanded = false;
        self.isl_vol = false;
        self.isl_bright = false;
        self.isl_opacity = false;
        self.isl_media = false;
        self.isl_timer = false;
        self.isl_devices = false;
        self.panel_rect = None;
        self.vol.clear();
        self.bright.clear();
    }

    /// A vertical slider. Returns Some(0..1) while it is being dragged.
    fn vslider(
        ui: &mut egui::Ui,
        p: &egui::Painter,
        id: impl std::hash::Hash,
        cx: f32,
        top: f32,
        h: f32,
        value: f32,
        accent: egui::Color32,
    ) -> Option<f32> {
        let w = 8.0;
        let track = egui::Rect::from_min_max(egui::pos2(cx - w / 2.0, top), egui::pos2(cx + w / 2.0, top + h));
        p.rect_filled(track, egui::Rounding::same(w / 2.0), ISL_HI);
        let fill_top = top + h * (1.0 - value.clamp(0.0, 1.0));
        p.rect_filled(
            egui::Rect::from_min_max(egui::pos2(cx - w / 2.0, fill_top), egui::pos2(cx + w / 2.0, top + h)),
            egui::Rounding::same(w / 2.0),
            accent,
        );
        p.circle_filled(egui::pos2(cx, fill_top), 6.0, WARM_TEXT);
        let hit = egui::Rect::from_min_max(egui::pos2(cx - 11.0, top - 6.0), egui::pos2(cx + 11.0, top + h + 6.0));
        let r = ui
            .interact(hit, egui::Id::new(id), egui::Sense::click_and_drag())
            .on_hover_cursor(egui::CursorIcon::ResizeVertical);
        r.interact_pointer_pos().map(|pos| (1.0 - (pos.y - top) / h).clamp(0.0, 1.0))
    }

    /// Draw the drop-down bubble and everything inside it.
    fn draw_panel(&mut self, ui: &mut egui::Ui, rect: egui::Rect, now: Instant, ctx: &egui::Context) {
        // Clip the window so only the bar strip and this bubble take input; the
        // rest of the enlarged window stays click-through to the desktop.
        #[cfg(windows)]
        {
            let want = (rect.bottom().ceil() as i32, rect.width().ceil() as i32);
            if self.hwnd != 0 && want != self.panel_shape {
                self.panel_shape = want;
                let bar_h = rice_common::settings::Settings::get().bar_height;
                unsafe {
                    set_window_shape(
                        self.hwnd,
                        self.width as i32,
                        bar_h,
                        Some((
                            (rect.left() - self.x as f32) as i32,
                            (rect.right() - self.x as f32) as i32,
                            rect.bottom() as i32,
                        )),
                    )
                };
            }
        }

        // No background here: the island above draws one continuous rounded
        // shape that already covers this area.
        let p = ui.painter().with_clip_rect(rect.expand(10.0));

        // Hold the contents back until the bubble is most of the way open, so
        // nothing renders squashed while the spring is still travelling.
        if self.panel_h / self.panel_target_h().max(1.0) < 0.55 {
            return;
        }

        match self.panel_mode() {
            1 => self.panel_volume(ui, &p, rect, now),
            2 => self.panel_bright(ui, &p, rect, now),
            3 => self.panel_opacity(ui, &p, rect, now),
            4 => self.panel_media(ui, &p, rect, now),
            5 => self.panel_timer(ui, &p, rect, now),
            6 => self.panel_devices(ui, &p, rect, now),
            _ => self.panel_actions(ui, &p, rect, now, ctx),
        }

        let timeout = rice_common::settings::Settings::live().animation.panel_timeout_secs;
        if now.duration_since(self.isl_interact).as_secs_f32() > timeout {
            self.close_panel();
        }
    }

    fn panel_actions(&mut self, ui: &mut egui::Ui, p: &egui::Painter, rect: egui::Rect, now: Instant, ctx: &egui::Context) {
        let slot = 52.0;
        let cols = 3usize;
        let x0 = rect.center().x - cols as f32 * slot / 2.0 + slot / 2.0;
        let y0 = rect.top() + 14.0 + slot / 2.0;
        for (i, (kind, glyph, col)) in ACTIONS.iter().enumerate() {
            let c = egui::pos2(x0 + (i % cols) as f32 * slot, y0 + (i / cols) as f32 * slot);
            let r = ui
                .interact(
                    egui::Rect::from_center_size(c, egui::vec2(slot - 8.0, slot - 8.0)),
                    egui::Id::new(("pnl-btn", i)),
                    egui::Sense::click(),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if r.hovered() {
                self.isl_interact = now;
            }
            let acc = egui::Color32::from_rgb(col[0], col[1], col[2]);
            let a = if r.hovered() { 90 } else { 42 };
            p.circle_filled(c, 17.0, egui::Color32::from_rgba_unmultiplied(acc.r(), acc.g(), acc.b(), a));
            draw_icon(p, c, glyph, 17.0, acc);
            if r.clicked() {
                self.isl_interact = now;
                match *kind {
                    "opacity" => self.isl_opacity = true,
                    "vol" => {
                        self.vol_ctl.refresh();
                        self.isl_vol = true;
                    }
                    "bright" => {
                        self.bright_ctl.refresh();
                        self.isl_bright = true;
                    }
                    "timer" => self.isl_timer = true,
                    "devices" => {
                        self.dev_ctl.refresh();
                        self.isl_devices = true;
                    }
                    k => {
                        run_action(k, self.shared.clone(), ctx.clone());
                        self.isl_expanded = false;
                        self.isl_vol = false;
                        self.isl_bright = false;
                        self.isl_opacity = false;
                    }
                }
            }
        }
    }

    fn panel_volume(&mut self, ui: &mut egui::Ui, p: &egui::Painter, rect: egui::Rect, now: Instant) {
        if self.vol.is_empty() {
            self.vol = self.vol_ctl.state.lock().unwrap().clone();
        }
        let top = rect.top() + 16.0;
        let h = 78.0;
        let n = self.vol.len().max(1) as f32;
        let step = 46.0;
        let x0 = rect.center().x - (n - 1.0) * step / 2.0 - 16.0;
        let mut changed: Option<(String, f32)> = None;
        for (i, (label, val)) in self.vol.iter_mut().enumerate() {
            let cx = x0 + i as f32 * step;
            if let Some(v) = Self::vslider(ui, p, ("pvol", i), cx, top, h, *val, WARM_ACCENT) {
                *val = v;
                changed = Some((if i == 0 { String::new() } else { label.clone() }, v));
            }
            p.text(
                egui::pos2(cx, top + h + 12.0),
                egui::Align2::CENTER_CENTER,
                label.chars().take(7).collect::<String>(),
                egui::FontId::proportional(8.5),
                WARM_SUB,
            );
        }
        if let Some((name, v)) = changed {
            self.isl_interact = now;
            self.vol_ctl.set(&name, v);
        }
        let c = egui::pos2(rect.right() - 24.0, top + h / 2.0);
        let r = ui
            .interact(egui::Rect::from_center_size(c, egui::vec2(30.0, 30.0)), egui::Id::new("pnl-swap"), egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if r.hovered() {
            self.isl_interact = now;
        }
        let a = if r.hovered() { 90 } else { 42 };
        p.circle_filled(c, 13.0, egui::Color32::from_rgba_unmultiplied(WARM_ACCENT.r(), WARM_ACCENT.g(), WARM_ACCENT.b(), a));
        draw_icon(p, c, "\u{f0ec}", 13.0, WARM_ACCENT);
        if r.clicked() {
            self.vol_ctl.swap_output();
            self.vol.clear();
            self.isl_interact = now;
        }
    }

    fn panel_bright(&mut self, ui: &mut egui::Ui, p: &egui::Painter, rect: egui::Rect, now: Instant) {
        if self.bright.is_empty() {
            self.bright = self.bright_ctl.state.lock().unwrap().clone();
        }
        let top = rect.top() + 16.0;
        let h = 78.0;
        let n = self.bright.len().max(1) as f32;
        let step = 46.0;
        let x0 = rect.center().x - (n - 1.0) * step / 2.0;
        let mut changed: Option<(isize, f32)> = None;
        for (i, (hmon, label, val)) in self.bright.iter_mut().enumerate() {
            let cx = x0 + i as f32 * step;
            if let Some(v) = Self::vslider(ui, p, ("pbr", i), cx, top, h, *val, WARM_ACCENT) {
                *val = v;
                changed = Some((*hmon, v));
            }
            p.text(
                egui::pos2(cx, top + h + 12.0),
                egui::Align2::CENTER_CENTER,
                format!("mon {label}"),
                egui::FontId::proportional(8.5),
                WARM_SUB,
            );
        }
        if let Some((hm, v)) = changed {
            self.isl_interact = now;
            self.bright_ctl.set(hm, v);
        }
    }


    /// Media view: what is playing plus transport controls.
    fn panel_media(&mut self, ui: &mut egui::Ui, p: &egui::Painter, rect: egui::Rect, now: Instant) {
        let Some(m) = self.media.clone() else {
            p.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "nada reproduciendose",
                egui::FontId::proportional(11.5),
                WARM_SUB,
            );
            return;
        };

        // Cover placeholder on the left. The artwork itself is fetched and
        // decoded separately; until then this keeps the layout stable.
        let art = egui::Rect::from_min_size(egui::pos2(rect.left() + 14.0, rect.top() + 14.0), egui::vec2(56.0, 56.0));
        p.rect_filled(art, egui::Rounding::same(8.0), ISL_HI);
        draw_icon(p, art.center(), "\u{f001}", 22.0, WARM_SUB); // fa-music

        let tx = art.right() + 12.0;
        p.text(
            egui::pos2(tx, rect.top() + 26.0),
            egui::Align2::LEFT_CENTER,
            m.title.chars().take(28).collect::<String>(),
            egui::FontId::proportional(12.5),
            WARM_TEXT,
        );
        p.text(
            egui::pos2(tx, rect.top() + 44.0),
            egui::Align2::LEFT_CENTER,
            m.artist.chars().take(32).collect::<String>(),
            egui::FontId::proportional(10.5),
            WARM_SUB,
        );

        // Transport row.
        let by = rect.top() + 92.0;
        let bx = rect.center().x;
        let btns: [(&str, f32, bool); 3] = [
            ("\u{f048}", bx - 46.0, m.can_prev),                              // prev
            (if m.playing { "\u{f04c}" } else { "\u{f04b}" }, bx, true),      // pause / play
            ("\u{f051}", bx + 46.0, m.can_next),                              // next
        ];
        for (i, (glyph, x, enabled)) in btns.iter().enumerate() {
            let c = egui::pos2(*x, by);
            let r = ui
                .interact(
                    egui::Rect::from_center_size(c, egui::vec2(34.0, 34.0)),
                    egui::Id::new(("media-btn", i)),
                    egui::Sense::click(),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if r.hovered() {
                self.isl_interact = now;
            }
            let acc = if *enabled { WARM_ACCENT } else { WARM_SUB };
            let a = if r.hovered() && *enabled { 90 } else { 42 };
            let rad = if i == 1 { 16.0 } else { 13.0 };
            p.circle_filled(c, rad, egui::Color32::from_rgba_unmultiplied(acc.r(), acc.g(), acc.b(), a));
            draw_icon(p, c, glyph, if i == 1 { 15.0 } else { 12.0 }, acc);
            if r.clicked() && *enabled {
                self.isl_interact = now;
                // Each of these is a blocking WinRT call; off the render thread.
                std::thread::spawn(move || match i {
                    0 => rice_common::media::previous(),
                    1 => rice_common::media::toggle_play(),
                    _ => rice_common::media::next(),
                });
                // Flip the glyph immediately rather than waiting for the poller
                // to notice: a play button that stays on "play" for over a
                // second after being pressed reads as not having worked.
                if i == 1 {
                    if let Some(m) = self.media.as_mut() {
                        m.playing = !m.playing;
                    }
                }
            }
        }
    }


    /// Pomodoro: a countdown with presets, start/pause and reset.
    fn panel_timer(&mut self, ui: &mut egui::Ui, p: &egui::Painter, rect: egui::Rect, now: Instant) {
        let secs = self.timer_left.as_secs();
        let label = format!("{:02}:{:02}", secs / 60, secs % 60);
        // Ring showing how much of the block is left.
        let ring_c = egui::pos2(rect.center().x, rect.top() + 44.0);
        let frac = if self.timer_total.as_secs_f32() > 0.0 {
            (self.timer_left.as_secs_f32() / self.timer_total.as_secs_f32()).clamp(0.0, 1.0)
        } else {
            0.0
        };
        p.circle_stroke(ring_c, 30.0, egui::Stroke::new(4.0, ISL_HI));
        // egui has no arc primitive; draw the remaining portion as short segments.
        let steps = 48;
        let lit = (frac * steps as f32).round() as usize;
        for i in 0..lit {
            let a0 = -std::f32::consts::FRAC_PI_2 + (i as f32 / steps as f32) * std::f32::consts::TAU;
            let a1 = -std::f32::consts::FRAC_PI_2 + ((i + 1) as f32 / steps as f32) * std::f32::consts::TAU;
            p.line_segment(
                [
                    ring_c + egui::vec2(a0.cos(), a0.sin()) * 30.0,
                    ring_c + egui::vec2(a1.cos(), a1.sin()) * 30.0,
                ],
                egui::Stroke::new(4.0, WARM_ACCENT),
            );
        }
        p.text(ring_c, egui::Align2::CENTER_CENTER, &label, egui::FontId::proportional(17.0), WARM_TEXT);

        // Controls: presets on the left/right of play-pause and reset.
        let by = rect.top() + 100.0;
        let cx = rect.center().x;
        // (glyph, x, action) -- action: 0 = toggle, 1 = reset, else = preset minutes
        let items: [(&str, f32, u64); 4] = [
            ("\u{f01e}", cx - 78.0, 1),                                        // reset
            (if self.timer_running { "\u{f04c}" } else { "\u{f04b}" }, cx - 26.0, 0),
            ("25", cx + 26.0, 25),
            ("5", cx + 78.0, 5),
        ];
        for (i, (glyph, x, action)) in items.iter().enumerate() {
            let c = egui::pos2(*x, by);
            let r = ui
                .interact(
                    egui::Rect::from_center_size(c, egui::vec2(40.0, 32.0)),
                    egui::Id::new(("timer-btn", i)),
                    egui::Sense::click(),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if r.hovered() {
                self.isl_interact = now;
            }
            let a = if r.hovered() { 90 } else { 42 };
            p.circle_filled(c, 14.0, egui::Color32::from_rgba_unmultiplied(WARM_ACCENT.r(), WARM_ACCENT.g(), WARM_ACCENT.b(), a));
            if *action >= 5 {
                // Preset: draw the number rather than an icon.
                p.text(c, egui::Align2::CENTER_CENTER, *glyph, egui::FontId::proportional(11.5), WARM_ACCENT);
            } else {
                draw_icon(p, c, glyph, 13.0, WARM_ACCENT);
            }
            if r.clicked() {
                self.isl_interact = now;
                match action {
                    0 => {
                        // Starting from zero restarts the block instead of doing nothing.
                        if self.timer_left.is_zero() {
                            self.timer_left = self.timer_total;
                        }
                        self.timer_running = !self.timer_running;
                        self.timer_tick = now;
                    }
                    1 => {
                        self.timer_running = false;
                        self.timer_left = self.timer_total;
                    }
                    m => {
                        self.timer_total = Duration::from_secs(m * 60);
                        self.timer_left = self.timer_total;
                        self.timer_running = true;
                        self.timer_tick = now;
                    }
                }
            }
        }
    }


    /// Playback outputs and Bluetooth devices in one list. Tapping a row makes
    /// it the default output; a Bluetooth device that is offline connects first
    /// and is then selected once it is really up.
    fn panel_devices(&mut self, ui: &mut egui::Ui, p: &egui::Painter, rect: egui::Rect, now: Instant) {
        let rows = self.dev_ctl.rows();
        p.text(
            egui::pos2(rect.center().x, rect.top() + 18.0),
            egui::Align2::CENTER_CENTER,
            "Dispositivos",
            egui::FontId::proportional(12.5),
            WARM_SUB,
        );
        if rows.is_empty() {
            p.text(
                egui::pos2(rect.center().x, rect.top() + 52.0),
                egui::Align2::CENTER_CENTER,
                "ninguno",
                egui::FontId::proportional(12.0),
                WARM_SUB,
            );
            return;
        }

        let mut y = rect.top() + 38.0;
        for (i, r) in rows.iter().enumerate() {
            let row_rect = egui::Rect::from_min_size(
                egui::pos2(rect.left() + 12.0, y),
                egui::vec2(rect.width() - 24.0, 32.0),
            );
            let resp = ui
                .interact(row_rect, egui::Id::new(("dev-row", i)), egui::Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if resp.hovered() {
                self.isl_interact = now;
            }
            if resp.hovered() || r.is_default {
                let a = if r.is_default { 46 } else { 24 };
                p.rect_filled(
                    row_rect,
                    egui::Rounding::same(8.0),
                    egui::Color32::from_rgba_unmultiplied(
                        WARM_ACCENT.r(),
                        WARM_ACCENT.g(),
                        WARM_ACCENT.b(),
                        a,
                    ),
                );
            }

            // Status dot: filled when connected, hollow when the device is known
            // but offline. Bluetooth rows are the only ones that can be offline
            // and still worth showing.
            let dot = egui::pos2(row_rect.left() + 12.0, row_rect.center().y);
            if r.busy {
                // Simple spinner: a dot that pulses while the connect is in flight.
                let t = now.elapsed().as_secs_f32();
                let pulse = 3.0 + 2.0 * (t * 6.0).sin().abs();
                p.circle_filled(dot, pulse, WARM_ACCENT);
            } else if r.connected {
                p.circle_filled(dot, 4.5, col(theme::ACCENT_OK));
            } else {
                p.circle_stroke(dot, 4.5, egui::Stroke::new(1.5, WARM_SUB));
            }

            let fg = if r.connected { WARM_TEXT } else { WARM_SUB };
            p.text(
                egui::pos2(row_rect.left() + 28.0, row_rect.center().y),
                egui::Align2::LEFT_CENTER,
                &r.label,
                egui::FontId::proportional(12.5),
                fg,
            );

            if r.bt_container.is_some() {
                draw_icon(
                    p,
                    egui::pos2(row_rect.right() - 32.0, row_rect.center().y),
                    "\u{f294}", // fa-bluetooth
                    11.0,
                    if r.connected { col(theme::ACCENT_OK) } else { WARM_SUB },
                );
            }
            if r.is_default {
                draw_icon(
                    p,
                    egui::pos2(row_rect.right() - 12.0, row_rect.center().y),
                    "\u{f00c}", // fa-check
                    11.0,
                    WARM_ACCENT,
                );
            }

            if resp.clicked() && !r.busy {
                self.isl_interact = now;
                match (r.bt_container, r.connected, &r.endpoint) {
                    // Offline Bluetooth device: wake it up, then select it.
                    (Some(c), false, _) => self.dev_ctl.connect(c),
                    // Connected Bluetooth device that is already the default:
                    // a second tap disconnects, which is the only way to hand
                    // the headset back to a phone.
                    (Some(c), true, _) if r.is_default => self.dev_ctl.disconnect(c),
                    (_, _, Some(id)) => self.dev_ctl.select(id.clone()),
                    _ => {}
                }
            }
            y += 38.0;
        }
    }

    fn panel_opacity(&mut self, ui: &mut egui::Ui, p: &egui::Painter, rect: egui::Rect, now: Instant) {
        let top = rect.top() + 16.0;
        let h = 78.0;
        let step = 46.0;
        let x0 = rect.center().x - step / 2.0;
        let mut wrote = false;
        for i in 0..2 {
            let cx = x0 + i as f32 * step;
            let cur = if i == 0 { self.bar_opacity } else { self.term_opacity };
            if let Some(v) = Self::vslider(ui, p, ("pop", i), cx, top, h, config::opacity_to_slider(cur), WARM_ACCENT) {
                let o = config::slider_to_opacity(v);
                if i == 0 {
                    self.bar_opacity = o;
                } else {
                    self.term_opacity = o;
                }
                self.isl_interact = now;
                wrote = true;
            }
            p.text(
                egui::pos2(cx, top + h + 12.0),
                egui::Align2::CENTER_CENTER,
                if i == 0 { "barra" } else { "term" },
                egui::FontId::proportional(8.5),
                WARM_SUB,
            );
        }
        // Throttled: each terminal write triggers a WezTerm config hot-reload.
        if wrote && now.duration_since(self.last_opacity_write).as_secs_f32() > 0.12 {
            self.last_opacity_write = now;
            write_opacity("bar-opacity.txt", self.bar_opacity);
            write_opacity("term-opacity.txt", self.term_opacity);
        }
    }
}

impl eframe::App for BarApp {
    fn clear_color(&self, _v: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.sized {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(self.width, 34.0)));
            self.sized = true;
        }

        // Icon-centring rig: draw the icons in chips at fixed, known centres (no text
        // nearby) so their real ink centre can be measured against a known point.
        if env_flag("GLAZEBAR_ICONTEST", &ICONTEST_ON) {
            // Render each glyph 10x large (white on black, no chip) at known centres
            // so the ink centroid can be measured with 10x sub-pixel precision.
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(1000.0, 300.0)));
            egui::CentralPanel::default()
                .frame(egui::Frame::none().fill(egui::Color32::from_rgb(16, 16, 16)))
                .show(ctx, |ui| {
                    for (g, cxp) in [("\u{f03d}", 200.0f32), ("\u{f120}", 500.0), ("\u{f130}", 800.0)] {
                        draw_icon(ui.painter(), egui::pos2(cxp, 150.0), g, 140.0, egui::Color32::WHITE);
                    }
                });
            return;
        }

        // Toggle click-through when a fullscreen game covers this monitor, so its
        // clicks reach the game (and normal clicks reach the workspaces otherwise).
        #[cfg(windows)]
        {
            let now = Instant::now();
            if now.duration_since(self.last_ct).as_secs_f32() > 0.5 {
                self.last_ct = now;
                // Re-resolve every tick, not just once: eframe can recreate the
                // window, and a stale handle means click-through silently stops
                // being applied to anything.
                self.hwnd = find_own_window(self.x, self.width as i32);
                if self.hwnd != 0 {
                    // Cheap no-op once done, but re-asserted because eframe can
                    // recreate the window under us.
                    unsafe { strip_native_frame(self.hwnd) };
                    let want = unsafe { should_clickthrough(self.hwnd) };
                    self.clickthrough = want;
                    // Assert the style unconditionally rather than only on a
                    // transition. set_clickthrough is a no-op when the bit already
                    // matches, and this way anything that clears the ex-style
                    // behind our back is corrected on the next tick instead of
                    // leaving the bar permanently solid.
                    unsafe { set_clickthrough(self.hwnd, want) };
                }
                // Live-reload bar opacity from the file (so editing it directly also
                // updates in real time), except while the slider owns the value.
                if !self.isl_opacity {
                    self.bar_opacity = read_opacity("bar-opacity.txt", self.bar_opacity);
                }
            }
        }

        let now_tick = Instant::now();

        // Pomodoro countdown. Driven off wall-clock deltas rather than a frame
        // count, so it stays correct even when the bar throttles its repaints.
        if self.timer_running {
            let dt = now_tick.saturating_duration_since(self.timer_tick);
            self.timer_tick = now_tick;
            self.timer_left = self.timer_left.saturating_sub(dt);
            if self.timer_left.is_zero() {
                self.timer_running = false;
                // Announce through the same island/toast path everything else uses.
                let ev = rice_common::event::IslandEvent::new(
                    "check",
                    "Temporizador",
                    "tiempo cumplido",
                    "#a9b56a",
                );
                let _ = ev.publish();
                let exe = win::sibling_exe("shadowplay-notify.exe");
                std::thread::spawn(move || {
                    use std::os::windows::process::CommandExt;
                    let _ = std::process::Command::new(exe)
                        .args([
                            "--title", "Temporizador", "--body", "tiempo cumplido",
                            "--icon", "check", "--accent", "#a9b56a", "--hold", "8",
                        ])
                        .creation_flags(win::CREATE_NO_WINDOW)
                        .spawn();
                });
            }
            ctx.request_repaint_after(Duration::from_millis(250));
        } else {
            self.timer_tick = now_tick;
        }
        // Pick up whatever the poller last saw. Just a lock and a compare; the
        // expensive part happens on its thread. Polling unconditionally (rather
        // than only while audible, as before) is what lets a *paused* session be
        // discovered at all -- previously nothing was detected until sound came
        // out, so a paused track offered nothing to press play on.
        {
            let m = self.media_rx.lock().unwrap().clone();
            if m != self.media {
                self.media = m;
                ctx.request_repaint();
            }
        }

        // Precomputed before the lock: calling a &self method inside the render
        // closure would force a whole-struct borrow and clash with `s`.
        let panel_rest_h = self.panel_target_h();
        let panel_w_now = self.panel_width();
        // Re-read on every frame so rice.json can be tuned with the bar running.
        // The call is a cached Arc clone; it only touches the disk once a second.
        let anim = rice_common::settings::Settings::live().animation.clone();
        let anim_ws = anim.workspace_ease;
        let bar_strip_h = self.bar_h();
        let s = self.shared.lock().unwrap();
        // Translucent bar (live-adjustable) so the desktop / a borderless game shows through.
        // Derived from BAR_BG rather than re-typing its channels, so the palette
        // stays the single source of truth for the bar's colour.
        let bar_bg = egui::Color32::from_rgba_unmultiplied(
            BAR_BG.r(),
            BAR_BG.g(),
            BAR_BG.b(),
            (self.bar_opacity * 255.0) as u8,
        );
        egui::CentralPanel::default()
            .frame(egui::Frame::none().inner_margin(egui::Margin::symmetric(10.0, 5.0)))
            .show(ctx, |ui| {
                // The window is TALLER than the bar whenever the bubble is open, so
                // ui.max_rect() is not the bar. Everything here is positioned against
                // `full`, so pin that to the top strip: using the whole window pushed
                // the workspaces, clock and metrics down into the enlarged area, where
                // the window region clips them away and they vanished.
                let win = ui.max_rect();
                let full = egui::Rect::from_min_size(
                    win.min,
                    egui::vec2(win.width(), (bar_strip_h - 10.0).max(1.0)),
                );
                // Only the strip gets the bar colour. Anything below it belongs to
                // the bubble and must stay transparent so the island is the only
                // thing drawn there.
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(
                        win.min - egui::vec2(10.0, 5.0),
                        egui::vec2(win.width() + 20.0, bar_strip_h),
                    ),
                    egui::Rounding::ZERO,
                    bar_bg,
                );

                // Frame delta, shared by every in-bar animation (island + workspace indicator).
                let now_i = Instant::now();
                let dt = (now_i - self.last_frame).as_secs_f32().clamp(0.0, 0.05);
                self.last_frame = now_i;

                ui.allocate_ui_at_rect(full, |ui| {
                ui.horizontal_centered(|ui| {
                    // ---- left: workspaces (clickable -> focus that workspace) ----
                    // The focused-workspace highlight is one pill that SLIDES between
                    // workspaces on switch instead of the fill snapping. Draw it first (at
                    // last frame's animated rect) so it sits *behind* the labels; the
                    // focused pill is itself transparent and this pill is its fill.
                    if let Some(r) = self.ws_ind {
                        ui.painter().rect_filled(r, egui::Rounding::same(5.0), WARM_ACCENT);
                    }
                    let mut focus_rect: Option<egui::Rect> = None;
                    for ws in &s.workspaces {
                        let label = ws
                            .display_name
                            .as_deref()
                            .filter(|t| !t.is_empty())
                            .unwrap_or(&ws.name);
                        let (bg, fg) = if ws.has_focus {
                            // transparent: the sliding indicator is this pill's highlight.
                            // Dark text, because the indicator behind it is now amber.
                            (egui::Color32::TRANSPARENT, BAR_BG)
                        } else if ws.is_displayed {
                            (ISL_HI, WARM_TEXT)
                        } else {
                            (egui::Color32::TRANSPARENT, WARM_SUB)
                        };
                        let resp = egui::Frame::none()
                            .fill(bg)
                            .rounding(5.0)
                            .inner_margin(egui::Margin::symmetric(9.0, 2.0))
                            .show(ui, |ui| {
                                ui.colored_label(fg, label);
                            })
                            .response
                            .interact(egui::Sense::click())
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        if ws.has_focus {
                            focus_rect = Some(resp.rect);
                        }
                        if resp.clicked() {
                            ipc_command(format!("command focus --workspace {}", ws.name));
                        }
                        ui.add_space(5.0);
                    }
                    // Ease the indicator toward the focused pill. First sighting snaps (no
                    // slide-in from nowhere); after that it springs and we keep repainting
                    // until it has essentially arrived. No focused pill on this monitor
                    // (focus is on the other monitor) -> hide it, matching the old look.
                    match focus_rect {
                        Some(target) => match self.ws_ind {
                            None => self.ws_ind = Some(target),
                            Some(cur) => {
                                let k = 1.0 - (-dt * anim_ws).exp();
                                let ni = egui::Rect::from_min_max(
                                    cur.min + (target.min - cur.min) * k,
                                    cur.max + (target.max - cur.max) * k,
                                );
                                self.ws_ind = Some(ni);
                                if (ni.min - target.min).length() + (ni.max - target.max).length() > 0.5 {
                                    ui.ctx().request_repaint();
                                }
                            }
                        },
                        None => self.ws_ind = None,
                    }

                    // ---- right: metrics ----
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(4.0);
                        let dim = egui::Color32::from_rgb(180, 180, 195);
                        if !s.gpu.is_empty() {
                            ui.colored_label(egui::Color32::from_rgb(255, 205, 120), format!("GPU {}", s.gpu));
                            ui.add_space(12.0);
                        }
                        let cpu_col = if s.cpu > 85.0 {
                            egui::Color32::from_rgb(255, 120, 120)
                        } else {
                            dim
                        };
                        ui.colored_label(cpu_col, format!("CPU {:>2.0}%", s.cpu));
                        ui.add_space(12.0);
                        ui.colored_label(dim, format!("RAM {:>2.0}%", s.mem));
                        ui.add_space(12.0);
                        if !s.net.is_empty() {
                            ui.colored_label(egui::Color32::from_rgb(130, 200, 150), &s.net);
                            ui.add_space(12.0);
                        }
                        let dir = if s.tiling == "vertical" { "|" } else { "—" };
                        ui.colored_label(egui::Color32::from_rgb(140, 160, 210), dir);
                        if !s.mode.is_empty() {
                            ui.add_space(12.0);
                            egui::Frame::none()
                                .fill(egui::Color32::from_rgb(200, 130, 60))
                                .rounding(5.0)
                                .inner_margin(egui::Margin::symmetric(8.0, 2.0))
                                .show(ui, |ui| {
                                    ui.colored_label(egui::Color32::WHITE, &s.mode);
                                });
                        }
                    });
                });
                });

                // ---- center: dynamic island (morphs to show context) ----
                // pick up a new event; expire an old one after the hold window
                if s.island_serial != self.isl_serial {
                    self.isl_serial = s.island_serial;
                    if let Some(ev) = s.island.clone() {
                        self.isl_notif = Some((ev, now_i));
                    }
                }
                if let Some((_, t)) = &self.isl_notif {
                    if now_i.duration_since(*t).as_secs_f32() > anim.notification_hold_secs {
                        self.isl_notif = None;
                    }
                }

                // ---- dynamic island: the clock is ALWAYS shown; the pill extends to
                // the right (bar height unchanged) for quick-actions / notifications ----
                let expanded = self.isl_expanded && self.isl_notif.is_none();
                let has_extra = self.isl_notif.is_some() || expanded;
                let clock = chrono::Local::now().format("%H:%M").to_string();
                let clock_w = ui
                    .painter()
                    .layout_no_wrap(clock.clone(), egui::FontId::proportional(14.0), WARM_TEXT)
                    .size()
                    .x;

                // width of the content shown to the right of the clock
                let extra_w = if let Some((ev, _)) = self.isl_notif.clone() {
                    let tw = ui.painter().layout_no_wrap(ev.title.clone(), egui::FontId::proportional(12.5), WARM_TEXT).size().x;
                    let bw = if ev.body.is_empty() {
                        0.0
                    } else {
                        ui.painter().layout_no_wrap(ev.body.clone(), egui::FontId::proportional(11.0), WARM_SUB).size().x
                    };
                    let icon_w = if icon_glyph(&ev.icon).is_empty() { 0.0 } else { 26.0 };
                    icon_w + tw.max(bw)
                } else {
                    // Everything interactive now lives in the vertical panel
                    // below: growing the pill sideways for six buttons plus
                    // sliders was crowding the bar, and it does not scale as
                    // more controls arrive.
                    0.0
                };

                let pad = 14.0;
                let div_gap = 22.0; // clock -> divider -> extra spacing (divider centred in it)
                let has_sound = (self.media.is_some() || self.spectrum.active()) && self.isl_notif.is_none();
                let spec_reserve = if has_sound { 48.0 } else { 0.0 };
                let timer_reserve = if self.timer_running && self.isl_notif.is_none() { 44.0 } else { 0.0 };
                let idle_w = pad + clock_w + timer_reserve + spec_reserve + pad;
                let notif_open = self.isl_notif.is_some();
                let target_w = if notif_open { pad + clock_w + div_gap + extra_w + pad } else { idle_w };
                let target_h = if has_extra { 30.0 } else { 24.0 };

                if self.isl_w <= 1.0 {
                    self.isl_w = target_w;
                    self.isl_h = target_h;
                }
                let pill_k = 1.0 - (-dt * anim.pill_ease).exp();
                self.isl_w += (target_w - self.isl_w) * pill_k;
                self.isl_h += (target_h - self.isl_h) * pill_k;

                let h = self.isl_h;
                let (cx, cy) = (full.center().x, full.center().y);
                // Anchor the left edge to the idle-centred position so the clock stays
                // put while the pill unfurls rightward.
                let left = cx - idle_w / 2.0;
                let pill = egui::Rect::from_min_size(egui::pos2(left, cy - h / 2.0), egui::vec2(self.isl_w, h));

                // ---- vertical panel spring -------------------------------------
                // Critically damped would just glide; this is deliberately
                // under-damped so it overshoots and settles back -- the bounce
                // that makes it read as a bubble growing out of the bar rather
                // than a menu appearing.
                let panel_target = if expanded { panel_rest_h } else { 0.0 };
                self.panel_p += ((if expanded { 1.0 } else { 0.0 }) - self.panel_p)
                    * (1.0 - (-dt * anim.text_ease).exp());
                {
                    // Both live in rice.json now -- see the notes on the fields
                    // there for what the damping ratio does to the bounce.
                    let (k, d) = (anim.spring_stiffness, anim.spring_damping);
                    let a = (panel_target - self.panel_h) * k - self.panel_v * d;
                    self.panel_v += a * dt;
                    self.panel_h += self.panel_v * dt;
                    if !expanded && self.panel_h < 0.4 && self.panel_v.abs() < 4.0 {
                        self.panel_h = 0.0;
                        self.panel_v = 0.0;
                    }
                    // Keep repainting while the spring is still moving.
                    if (self.panel_h - panel_target).abs() > 0.3 || self.panel_v.abs() > 1.0 {
                        ctx.request_repaint();
                    }
                }

                // ---- morph the pill into the panel --------------------------
                // 0 while closed, 1 once the bubble has reached its resting
                // height. Everything below interpolates on it, so there is only
                // ever ONE rounded rectangle on screen: the pill grows into the
                // big one instead of a second box appearing under it.
                let morph = (self.panel_h / panel_rest_h.max(1.0)).clamp(0.0, 1.0);
                let m_w = self.isl_w + (panel_w_now - self.isl_w) * morph;
                // Centred when open, left-anchored when closed (a notification
                // still unfurls rightward from the clock).
                let m_left = pill.left() + ((cx - m_w / 2.0) - pill.left()) * morph;
                // The panel's own rect starts just under the strip; grow down to
                // meet it, overshoot included, so the bounce carries the whole shape.
                let pill_bottom = cy + h / 2.0;
                let panel_bottom = bar_strip_h - 2.0 + self.panel_h;
                let m_bottom = pill_bottom + (panel_bottom - pill_bottom) * morph;
                let rect = egui::Rect::from_min_max(
                    egui::pos2(m_left, cy - h / 2.0),
                    egui::pos2(m_left + m_w, m_bottom),
                );
                let round = egui::Rounding::same(h / 2.0 + (20.0 - h / 2.0) * morph);

                // click to expand/dismiss; when expanded only sense hover (keeps it
                // open) so the buttons own the clicks (no z-order race).
                let pill_sense = if expanded { egui::Sense::hover() } else { egui::Sense::click() };
                let pill = ui
                    .interact(rect, egui::Id::new("isl-pill"), pill_sense)
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if pill.hovered() {
                    self.isl_interact = now_i;
                }

                // neumorphic: layered soft drop shadow + raised surface + top highlight
                for i in 1..=5u8 {
                    let o = i as f32;
                    ui.painter().rect_filled(
                        rect.translate(egui::vec2(0.0, o * 0.8)).expand2(egui::vec2(-o * 0.25, -o * 0.12)),
                        round,
                        egui::Color32::from_rgba_unmultiplied(6, 4, 3, (32 - i as i32 * 5) as u8),
                    );
                }
                ui.painter().rect_filled(rect, round, ISL_SURFACE);
                ui.painter().rect_stroke(
                    rect.shrink(0.5),
                    round,
                    egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(ISL_HI.r(), ISL_HI.g(), ISL_HI.b(), 110)),
                );

                // Live spectrum, drawn inside the pill to the right of the clock when
                // something is audible. This is the "it knows music is playing" cue.
                let spec_w = if has_sound { 42.0 } else { 0.0 };
                // A running countdown is worth seeing without opening anything.
                let timer_w = if self.timer_running && self.isl_notif.is_none() { 44.0 } else { 0.0 };

                // ---- content, CLIPPED to the animated pill so it wipes in/out as the
                // pill grows/shrinks (the clock is at the left, always inside) ----
                let cp = ui.painter().with_clip_rect(rect);
                // Left edge of the spectrum slot, filled in below; clicking there
                // means "media", clicking anywhere else on the pill means "controls".
                let mut spec_x0 = f32::INFINITY;
                // The clock scales with the expansion and slides to the centre of
                // the open shape, so it reads as the header of the panel rather
                // than a leftover from the collapsed pill.
                let clock_fs = 14.0 + 5.0 * self.panel_p;
                let clock_w2 = ui
                    .painter()
                    .layout_no_wrap(clock.clone(), egui::FontId::proportional(clock_fs), WARM_TEXT)
                    .size()
                    .x;
                let clock_left = rect.left() + pad;
                let mut tx = clock_left + ((cx - clock_w2 / 2.0) - clock_left) * self.panel_p;
                cp.text(
                    egui::pos2(tx, cy),
                    egui::Align2::LEFT_CENTER,
                    &clock,
                    egui::FontId::proportional(clock_fs),
                    WARM_TEXT,
                );
                tx += clock_w2;
                let fade = |c: egui::Color32| {
                    egui::Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), ((1.0 - self.panel_p) * 255.0) as u8)
                };
                if timer_w > 0.0 {
                    let secs = self.timer_left.as_secs();
                    cp.text(
                        egui::pos2(tx + 8.0, cy),
                        egui::Align2::LEFT_CENTER,
                        format!("{:02}:{:02}", secs / 60, secs % 60),
                        egui::FontId::proportional(11.5),
                        fade(col(theme::ACCENT_OK)),
                    );
                    tx += timer_w;
                }
                if spec_w > 0.0 {
                    spec_x0 = tx;
                    // The bars are an animation: at the bar's idle 1 fps they
                    // looked broken rather than slow. Ask for animation rates
                    // while they are actually moving. 30fps rather than 60 --
                    // measured, 60 costs ~8% CPU for the whole (2560px) bar and
                    // looks no different on bars this small.
                    if self.spectrum.active() {
                        let ms = (1000 / anim.spectrum_fps.clamp(1, 240)) as u64;
                        ctx.request_repaint_after(Duration::from_millis(ms));
                    }
                    let levels = self.spectrum.levels();
                    let n = levels.len().max(1);
                    let bw = 3.0;
                    let gap = (spec_w - n as f32 * bw) / (n as f32 - 1.0).max(1.0);
                    let base = cy + 7.0;
                    let max_h = 13.0;
                    for (i, v) in levels.iter().enumerate() {
                        let h = (v * max_h).clamp(1.0, max_h);
                        let x = tx + 6.0 + i as f32 * (bw + gap);
                        // Colour follows the level: amber low, lime at the top, so
                        // it matches the rest of the rice rather than being a
                        // separate palette.
                        let c = fade(if *v > 0.66 { col(theme::ACCENT_OK) } else { WARM_ACCENT });
                        cp.rect_filled(
                            egui::Rect::from_min_max(egui::pos2(x, base - h), egui::pos2(x + bw, base)),
                            egui::Rounding::same(1.5),
                            c,
                        );
                    }
                }
                if notif_open {
                    let dx = tx + div_gap / 2.0;
                    cp.line_segment(
                        [egui::pos2(dx, cy - 7.0), egui::pos2(dx, cy + 7.0)],
                        egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(WARM_SUB.r(), WARM_SUB.g(), WARM_SUB.b(), 70)),
                    );
                    tx += div_gap;
                }

                if let Some((ev, _)) = self.isl_notif.clone() {
                    let accent = egui::Color32::from_rgb(ev.accent[0], ev.accent[1], ev.accent[2]);
                    let icon = icon_glyph(&ev.icon);
                    if !icon.is_empty() {
                        let c = egui::pos2(tx + 9.0, cy);
                        cp.circle_filled(c, 9.5, egui::Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 42));
                        draw_icon(&cp, c, icon, 13.0, accent);
                        tx += 26.0;
                    }
                    if ev.body.is_empty() {
                        cp.text(egui::pos2(tx, cy), egui::Align2::LEFT_CENTER, &ev.title, egui::FontId::proportional(13.0), WARM_TEXT);
                    } else {
                        cp.text(egui::pos2(tx, cy - 6.0), egui::Align2::LEFT_CENTER, &ev.title, egui::FontId::proportional(12.5), WARM_TEXT);
                        cp.text(egui::pos2(tx, cy + 6.0), egui::Align2::LEFT_CENTER, &ev.body, egui::FontId::proportional(11.0), WARM_SUB);
                    }
                    if pill.clicked() {
                        self.isl_notif = None; // click dismisses early
                    }
                } else if expanded {
                    // The pill itself stays compact while expanded; every control
                    // now lives in the vertical panel drawn below.
                    let clock_hit = egui::Rect::from_min_max(rect.left_top(), rect.right_bottom());
                    if ui
                        .interact(clock_hit, egui::Id::new("isl-clock"), egui::Sense::click())
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        // Inlined rather than close_panel(): a method call here
                        // would borrow all of self and clash with the lock.
                        self.isl_expanded = false;
                        self.isl_vol = false;
                        self.isl_bright = false;
                        self.isl_opacity = false;
                    }
                } else if pill.clicked() {
                    // Which part of the pill was hit decides what opens. Opening
                    // the player whenever *anything* was playing meant the media
                    // controls sat on top of the bubble permanently and every
                    // other control became unreachable while listening to music.
                    // Only the spectrum -- the thing advertising the playback --
                    // opens the player now.
                    let on_spec = pill.interact_pointer_pos().map(|p| p.x >= spec_x0).unwrap_or(false);
                    self.isl_expanded = true;
                    self.isl_media = on_spec && self.media.is_some();
                    self.isl_interact = now_i;
                }

                // keep animating while morphing, or while a notification / the menu is up
                if (self.isl_w - target_w).abs() > 0.3
                    || (self.isl_h - target_h).abs() > 0.3
                    || self.isl_notif.is_some()
                    || self.isl_expanded
                {
                    ctx.request_repaint();
                }
            });
        drop(s);

        // A click anywhere outside the bar/bubble should dismiss the panel. Those
        // clicks never arrive as egui events: the window region excludes that
        // area, so the press goes straight to whatever is underneath. Poll the
        // physical button and the cursor instead.
        #[cfg(windows)]
        if self.isl_expanded {
            let outside = unsafe {
                let mut pt = CursorPos { x: 0, y: 0 };
                if GetCursorPos(&mut pt) == 0 {
                    false
                } else {
                    let bar = self.bar_h() as i32;
                    let in_strip = pt.y >= 0
                        && pt.y < bar
                        && pt.x >= self.x
                        && pt.x < self.x + self.width as i32;
                    let in_bubble = match self.panel_rect {
                        Some(r) => {
                            (pt.x as f32) >= r.left()
                                && (pt.x as f32) <= r.right()
                                && (pt.y as f32) >= r.top()
                                && (pt.y as f32) <= r.bottom()
                        }
                        None => false,
                    };
                    !(in_strip || in_bubble)
                }
            };
            let down = unsafe { (GetAsyncKeyState(0x01) as u16 & 0x8000) != 0 }; // VK_LBUTTON
            if down && outside {
                self.close_panel();
                ctx.request_repaint();
            }
        }

        // ---- the bubble: drawn after the shared lock is released (draw_panel
        // needs &mut self, which the lock would block), in its own Area so it can
        // extend below the bar strip. ----
        // The window has to be tall enough to contain the bubble: anything drawn
        // past its height is simply not on screen. Its REGION (set in draw_panel)
        // is what keeps the enlarged area from eating clicks.
        {
            let bar = self.bar_h();
            let want = (bar + self.panel_h.max(0.0) + 12.0).ceil() as i32;
            let want = if self.panel_h > 0.5 { want } else { bar as i32 };
            if want != self.win_h {
                self.win_h = want;
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(self.width, want as f32)));
                #[cfg(windows)]
                if self.panel_h <= 0.5 && self.hwnd != 0 {
                    // Collapsed: drop the custom region so the bar is a plain strip again.
                    self.panel_shape = (0, 0);
                    unsafe { set_window_shape(self.hwnd, self.width as i32, bar as i32, None) };
                }
            }
        }

        if self.panel_h > 0.5 {
            let screen = ctx.screen_rect();
            let pw = self.panel_width();
            let prect = egui::Rect::from_min_size(
                egui::pos2(screen.center().x - pw / 2.0, screen.top() + self.bar_h() - 2.0),
                egui::vec2(pw, self.panel_h),
            );
            self.panel_rect = Some(prect);
            let now_p = Instant::now();
            let ctx2 = ctx.clone();
            egui::Area::new(egui::Id::new("isl-panel"))
                .fixed_pos(prect.min)
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    self.draw_panel(ui, prect, now_p, &ctx2);
                });
        }

        self.frame = self.frame.wrapping_add(1);
        if self.frame % 15 == 5 {
            win::trim_ram();
        }
        ctx.request_repaint_after(Duration::from_millis(1000));
    }
}

fn arg_val(flag: &str, default: f32) -> f32 {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn main() -> eframe::Result<()> {
    let x = arg_val("--x", 0.0);
    let width = arg_val("--width", 1920.0);
    #[cfg(windows)]
    claim_single_instance(x as i32);

    let shared = Arc::new(Mutex::new(Shared::default()));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_resizable(false)
            .with_transparent(true) // per-pixel alpha so the bar can be translucent
            .with_inner_size([width, 34.0])
            .with_position([x, 0.0])
            .with_title("glaze-bar"),
        ..Default::default()
    };
    eframe::run_native(
        "glaze-bar",
        options,
        Box::new(move |cc| {
            load_font(&cc.egui_ctx);
            let ctx = cc.egui_ctx.clone();
            let s1 = shared.clone();
            std::thread::spawn(move || ipc_thread(s1, x as i32, ctx.clone()));
            let s2 = shared.clone();
            let ctx2 = cc.egui_ctx.clone();
            std::thread::spawn(move || sys_thread(s2, ctx2));
            let s3 = shared.clone();
            let ctx3 = cc.egui_ctx.clone();
            std::thread::spawn(move || gpu_thread(s3, ctx3));
            let s4 = shared.clone();
            let ctx4 = cc.egui_ctx.clone();
            std::thread::spawn(move || island_watcher(s4, ctx4));
            Ok(Box::new(BarApp {
                shared,
                width,
                x: x as i32,
                sized: false,
                frame: 0,
                isl_w: 0.0,
                isl_h: 0.0,
                isl_serial: 0,
                isl_notif: None,
                isl_expanded: false,
                isl_interact: Instant::now(),
                last_frame: Instant::now(),
                ws_ind: None,
                hwnd: 0,
                clickthrough: false,
                last_ct: Instant::now(),
                bar_opacity: read_opacity("bar-opacity.txt", 0.78),
                term_opacity: read_opacity("term-opacity.txt", 0.85),
                isl_opacity: false,
                isl_bright: false,
                bright: Vec::new(),
                bright_ctl: BrightCtl::spawn(cc.egui_ctx.clone()),
                dev_ctl: DevCtl::new(cc.egui_ctx.clone()),
                isl_vol: false,
                vol: Vec::new(),
                vol_ctl: VolCtl::spawn(cc.egui_ctx.clone()),
                panel_h: 0.0,
                panel_p: 0.0,
                panel_v: 0.0,
                panel_shape: (0, 0),
                // Eight bands is what fits legibly at pill size.
                spectrum: rice_common::spectrum::Spectrum::start(8),
                media: None,
                media_rx: {
                    let cell: Arc<Mutex<Option<rice_common::media::NowPlaying>>> =
                        Arc::new(Mutex::new(None));
                    let w = cell.clone();
                    std::thread::spawn(move || loop {
                        let m = rice_common::media::now_playing();
                        *w.lock().unwrap() = m;
                        std::thread::sleep(Duration::from_millis(1500));
                    });
                    cell
                },
                isl_media: false,
                isl_timer: false,
                isl_devices: false,
                timer_left: Duration::from_secs(25 * 60),
                timer_total: Duration::from_secs(25 * 60),
                timer_running: false,
                timer_tick: Instant::now(),
                win_h: 0,
                panel_rect: None,
                last_opacity_write: Instant::now(),
            }))
        }),
    )
}
