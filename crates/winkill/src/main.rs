// No console: this is fired from a hotkey.
#![windows_subsystem = "windows"]

//! Force-kill the process behind the focused window.
//!
//!   winkill            terminate the focused window's process
//!   winkill --close    ask it to close politely (what GlazeWM's `close` does)
//!
//! This exists because the window manager's close command is a *request*:
//! WM_CLOSE, which the application is free to ignore, delay behind a "save
//! changes?" dialog, or interpret as minimise-to-tray. That is the right default
//! and stays on Super+Q. This is the escape hatch for when an application has
//! stopped listening, and it is deliberately bound to something harder to press.
//!
//! Unsaved work IS lost -- there is no polite phase and no dialog. That is the
//! entire point, and the reason it is not the default binding.

#[cfg(windows)]
mod win {
    #[link(name = "user32")]
    extern "system" {
        pub fn GetForegroundWindow() -> isize;
        pub fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
        pub fn PostMessageW(hwnd: isize, msg: u32, w: usize, l: isize) -> i32;
        pub fn GetClassNameW(hwnd: isize, buf: *mut u16, n: i32) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        pub fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
        pub fn TerminateProcess(h: isize, code: u32) -> i32;
        pub fn CloseHandle(h: isize) -> i32;
        pub fn GetCurrentProcessId() -> u32;
        pub fn QueryFullProcessImageNameW(h: isize, flags: u32, buf: *mut u16, size: *mut u32) -> i32;
    }
    pub const WM_CLOSE: u32 = 0x0010;
    pub const PROCESS_TERMINATE: u32 = 0x0001;
    pub const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
}

#[cfg(windows)]
fn class_of(hwnd: isize) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { win::GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

#[cfg(windows)]
fn exe_of(pid: u32) -> String {
    unsafe {
        let h = win::OpenProcess(win::PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h == 0 {
            return String::new();
        }
        let mut buf = [0u16; 260];
        let mut len = buf.len() as u32;
        let ok = win::QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
        win::CloseHandle(h);
        if ok == 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..len as usize])
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or("")
            .to_lowercase()
    }
}

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let polite = args.iter().any(|a| a == "--close");

    let hwnd = unsafe { win::GetForegroundWindow() };
    if hwnd == 0 {
        return;
    }

    // Never act on the shell. Killing explorer takes the desktop, the taskbar and
    // every Explorer window with it, and the foreground window is Progman
    // whenever the desktop itself has focus -- which is exactly the state you are
    // in after closing the last window, i.e. the moment you are most likely to
    // press this again.
    let cls = class_of(hwnd);
    if matches!(
        cls.as_str(),
        "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "Windows.UI.Core.CoreWindow"
    ) {
        return;
    }

    let mut pid = 0u32;
    unsafe { win::GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid == 0 || pid == unsafe { win::GetCurrentProcessId() } {
        return;
    }

    // Nor on our own desktop: killing the bar or the window manager from a
    // stray keypress is not a recoverable mistake mid-session.
    let exe = exe_of(pid);
    if matches!(
        exe.as_str(),
        "explorer.exe"
            | "glazewm.exe"
            | "glaze-bar.exe"
            | "ws-slide.exe"
            | "taskbar.exe"
            | "notifyd.exe"
            | "autohotkey64.exe"
            | "altsnap.exe"
            | "dwm.exe"
            | ""
    ) {
        return;
    }

    if polite {
        unsafe { win::PostMessageW(hwnd, win::WM_CLOSE, 0, 0) };
        return;
    }

    unsafe {
        let h = win::OpenProcess(win::PROCESS_TERMINATE, 0, pid);
        if h != 0 {
            win::TerminateProcess(h, 1);
            win::CloseHandle(h);
        }
    }
}

#[cfg(not(windows))]
fn main() {}
