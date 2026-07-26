// No console: this is fired from a hotkey and prints nothing anyone reads.
#![windows_subsystem = "windows"]

//! Show or hide the Windows taskbar.
//!
//!   taskbar --hide     hide it and reclaim the space
//!   taskbar --show     put it back
//!   taskbar --toggle   flip, based on whether it is currently visible
//!   taskbar --watch    hide, then keep it hidden (daemon)
//!
//! Two steps are needed, and the order matters:
//!
//! 1. Auto-hide, via SHAppBarMessage(ABM_SETSTATE). This is what makes Windows
//!    recompute the *work area* to the full monitor. Without it the taskbar's
//!    strip stays reserved, and GlazeWM -- which tiles inside the work area (see
//!    outer_gap.top in its config, which exists to clear our own bar) -- would
//!    leave an empty band along the bottom of every workspace.
//! 2. ShowWindow(SW_HIDE) on the taskbar windows themselves, so it does not even
//!    slide back in when the pointer reaches the screen edge.
//!
//! Doing only (2) hides it but wastes the space; doing only (1) reclaims the
//! space but the bar still appears on hover.
//!
//! (2) does not stick, though, which is why --watch exists. Explorer owns the
//! auto-hide reveal: when the pointer reaches the screen edge it slides the bar
//! back on and shows the window again, undoing our SW_HIDE. Measured -- after a
//! login the tray window read visible=true and merely sat at y=1078, sliding to
//! y=1032 on hover. A one-shot hide therefore decays into plain auto-hide within
//! seconds. --watch re-asserts the hide whenever Explorer reveals it.

#[cfg(windows)]
mod win {
    #[repr(C)]
    pub struct Rect {
        pub left: i32,
        pub top: i32,
        pub right: i32,
        pub bottom: i32,
    }

    #[repr(C)]
    pub struct AppBarData {
        pub cb_size: u32,
        pub hwnd: isize,
        pub callback_message: u32,
        pub edge: u32,
        pub rc: Rect,
        pub lparam: isize,
    }

    pub type WinEventProc = extern "system" fn(isize, u32, isize, i32, i32, u32, u32);

    #[repr(C)]
    #[derive(Default)]
    pub struct Msg {
        pub hwnd: isize,
        pub message: u32,
        pub wparam: usize,
        pub lparam: isize,
        pub time: u32,
        pub pt_x: i32,
        pub pt_y: i32,
    }

    #[link(name = "user32")]
    extern "system" {
        pub fn FindWindowW(class: *const u16, window: *const u16) -> isize;
        pub fn FindWindowExW(parent: isize, after: isize, class: *const u16, window: *const u16) -> isize;
        pub fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
        pub fn IsWindowVisible(hwnd: isize) -> i32;
        pub fn GetClassNameW(hwnd: isize, buf: *mut u16, n: i32) -> i32;
        pub fn SetWinEventHook(min: u32, max: u32, dll: isize, cb: WinEventProc, pid: u32, tid: u32, flags: u32) -> isize;
        pub fn GetMessageW(msg: *mut Msg, hwnd: isize, a: u32, b: u32) -> i32;
        pub fn DispatchMessageW(msg: *const Msg) -> isize;
    }

    #[link(name = "shell32")]
    extern "system" {
        pub fn SHAppBarMessage(msg: u32, data: *mut AppBarData) -> usize;
    }

    pub const SW_HIDE: i32 = 0;
    pub const SW_SHOW: i32 = 5;
    pub const ABM_SETSTATE: u32 = 0x0000_000A;
    pub const ABS_AUTOHIDE: isize = 0x0000_0001;
    pub const ABS_ALWAYSONTOP: isize = 0x0000_0002;
    pub const EVENT_OBJECT_SHOW: u32 = 0x8002;
    pub const EVENT_OBJECT_LOCATIONCHANGE: u32 = 0x800B;
    pub const OBJID_WINDOW: i32 = 0;
    pub const WINEVENT_OUTOFCONTEXT: u32 = 0x0000;
    pub const WINEVENT_SKIPOWNPROCESS: u32 = 0x0002;

    pub fn class_of(h: isize) -> String {
        let mut buf = [0u16; 64];
        let n = unsafe { GetClassNameW(h, buf.as_mut_ptr(), buf.len() as i32) };
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }

    pub fn is_taskbar(h: isize) -> bool {
        matches!(class_of(h).as_str(), "Shell_TrayWnd" | "Shell_SecondaryTrayWnd")
    }

    pub fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// The primary taskbar plus one per secondary monitor.
    pub fn taskbars() -> Vec<isize> {
        let mut out = Vec::new();
        unsafe {
            let primary = wide("Shell_TrayWnd");
            let h = FindWindowW(primary.as_ptr(), std::ptr::null());
            if h != 0 {
                out.push(h);
            }
            // Secondary monitors each get their own Shell_SecondaryTrayWnd.
            let secondary = wide("Shell_SecondaryTrayWnd");
            let mut prev = 0isize;
            loop {
                let h = FindWindowExW(0, prev, secondary.as_ptr(), std::ptr::null());
                if h == 0 {
                    break;
                }
                out.push(h);
                prev = h;
            }
        }
        out
    }

    /// ABS_AUTOHIDE makes Windows hand the reserved strip back to the work area.
    pub fn set_autohide(on: bool) {
        unsafe {
            let mut d = AppBarData {
                cb_size: std::mem::size_of::<AppBarData>() as u32,
                hwnd: 0,
                callback_message: 0,
                edge: 0,
                rc: Rect { left: 0, top: 0, right: 0, bottom: 0 },
                lparam: if on { ABS_AUTOHIDE } else { ABS_ALWAYSONTOP },
            };
            SHAppBarMessage(ABM_SETSTATE, &mut d);
        }
    }

    pub fn set_visible(show: bool) {
        unsafe {
            for h in taskbars() {
                ShowWindow(h, if show { SW_SHOW } else { SW_HIDE });
            }
        }
    }

    pub fn is_visible() -> bool {
        unsafe { taskbars().first().map(|&h| IsWindowVisible(h) != 0).unwrap_or(true) }
    }
}

/// Marker file meaning "the user asked for the taskbar back". The watcher stops
/// re-hiding while it exists, so Win+Shift+B still wins against the daemon
/// instead of the two fighting each other a few times a second.
#[cfg(windows)]
fn shown_marker() -> std::path::PathBuf {
    rice_common::config::config_path("taskbar-shown")
}

#[cfg(windows)]
extern "system" fn hook_cb(_h: isize, _ev: u32, hwnd: isize, id_object: i32, _idc: i32, _t: u32, _tm: u32) {
    if id_object != win::OBJID_WINDOW || hwnd == 0 {
        return;
    }
    // Cheapest check first: the class filter rejects the flood of unrelated
    // location-change events without touching the disk.
    if !win::is_taskbar(hwnd) {
        return;
    }
    if unsafe { win::IsWindowVisible(hwnd) } == 0 {
        return; // already hidden; nothing to undo
    }
    if shown_marker().exists() {
        return; // deliberately shown
    }
    unsafe { win::ShowWindow(hwnd, win::SW_HIDE) };
}

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let has = |f: &str| args.iter().any(|a| a == f);

    let watch = has("--watch");
    let hide = if has("--hide") || watch {
        true
    } else if has("--show") {
        false
    } else if has("--toggle") || args.len() == 1 {
        win::is_visible()
    } else {
        return;
    };

    // Record the intent before acting, so a watcher already running sees it.
    let marker = shown_marker();
    if hide {
        let _ = std::fs::remove_file(&marker);
    } else {
        let _ = std::fs::write(&marker, "1");
    }

    if hide {
        win::set_autohide(true); // reclaim the work area first...
        // ...then take the window off screen. Explorer handles the auto-hide
        // transition asynchronously and re-shows the bar as part of it, so an
        // immediate SW_HIDE gets undone. Let it settle, hide, and assert it once
        // more in case the slide animation was still running.
        std::thread::sleep(std::time::Duration::from_millis(350));
        win::set_visible(false);
        std::thread::sleep(std::time::Duration::from_millis(250));
        win::set_visible(false);
    } else {
        win::set_autohide(false);
        std::thread::sleep(std::time::Duration::from_millis(250));
        win::set_visible(true);
    }

    if !watch {
        return;
    }

    // Stay resident and undo Explorer's reveal. Event-driven rather than polled:
    // LOCATIONCHANGE is what the slide-in animation actually raises, and SHOW
    // covers Explorer restarting or re-creating the bar.
    unsafe {
        win::SetWinEventHook(
            win::EVENT_OBJECT_SHOW,
            win::EVENT_OBJECT_SHOW,
            0,
            hook_cb,
            0,
            0,
            win::WINEVENT_OUTOFCONTEXT | win::WINEVENT_SKIPOWNPROCESS,
        );
        win::SetWinEventHook(
            win::EVENT_OBJECT_LOCATIONCHANGE,
            win::EVENT_OBJECT_LOCATIONCHANGE,
            0,
            hook_cb,
            0,
            0,
            win::WINEVENT_OUTOFCONTEXT | win::WINEVENT_SKIPOWNPROCESS,
        );
        let mut msg = win::Msg::default();
        // DispatchMessageW is not optional: without it an unhandled WM_PAINT is
        // re-posted forever and this spins a core at ~90% (the exact bug that
        // ws-slide shipped with once).
        while win::GetMessageW(&mut msg, 0, 0, 0) > 0 {
            win::DispatchMessageW(&msg);
        }
    }
}

#[cfg(not(windows))]
fn main() {}
