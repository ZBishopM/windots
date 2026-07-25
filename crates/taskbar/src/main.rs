// No console: this is fired from a hotkey and prints nothing anyone reads.
#![windows_subsystem = "windows"]

//! Show or hide the Windows taskbar.
//!
//!   taskbar --hide     hide it and reclaim the space
//!   taskbar --show     put it back
//!   taskbar --toggle   flip, based on whether it is currently visible
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

    #[link(name = "user32")]
    extern "system" {
        pub fn FindWindowW(class: *const u16, window: *const u16) -> isize;
        pub fn FindWindowExW(parent: isize, after: isize, class: *const u16, window: *const u16) -> isize;
        pub fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
        pub fn IsWindowVisible(hwnd: isize) -> i32;
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

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let has = |f: &str| args.iter().any(|a| a == f);

    let hide = if has("--hide") {
        true
    } else if has("--show") {
        false
    } else if has("--toggle") || args.len() == 1 {
        win::is_visible()
    } else {
        return;
    };

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
}

#[cfg(not(windows))]
fn main() {}
