//! Small Win32 helpers shared by every tool.
//!
//! These use raw `extern "system"` declarations rather than the `windows`
//! crate, so a binary that only needs (say) a single-instance mutex doesn't
//! gain a Win32 metadata dependency to get one.

#[cfg(windows)]
extern "system" {
    fn GetCurrentProcess() -> isize;
    fn K32EmptyWorkingSet(process: isize) -> i32;
    fn CreateMutexW(attr: *const core::ffi::c_void, owner: i32, name: *const u16) -> isize;
    fn GetLastError() -> u32;
    fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
    fn SetWindowLongPtrW(hwnd: isize, index: i32, val: isize) -> isize;
    fn SetWindowPos(hwnd: isize, after: isize, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32;
}

const ERROR_ALREADY_EXISTS: u32 = 183;
const GWL_EXSTYLE: i32 = -20;
const WS_EX_TRANSPARENT: isize = 0x20;
const WS_EX_NOACTIVATE: isize = 0x0800_0000;
const WS_EX_TOOLWINDOW: isize = 0x80;
const HWND_TOPMOST: isize = -1;
const SWP_NOMOVE: u32 = 0x0002;
const SWP_NOSIZE: u32 = 0x0001;
const SWP_NOACTIVATE: u32 = 0x0010;

/// `CREATE_NO_WINDOW` for `std::os::windows::process::CommandExt::creation_flags`,
/// so spawning a helper never flashes a console.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A nul-terminated UTF-16 buffer for Win32 `W` APIs.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Take a named mutex. Returns `false` if another process already holds it.
///
/// The handle is intentionally leaked: it must live for the process lifetime,
/// and Windows releases it on exit.
pub fn single_instance(name: &str) -> bool {
    #[cfg(windows)]
    unsafe {
        let w = wide(name);
        CreateMutexW(std::ptr::null(), 0, w.as_ptr());
        GetLastError() != ERROR_ALREADY_EXISTS
    }
    #[cfg(not(windows))]
    true
}

/// Take a named mutex or exit the process. For tools whose only response to a
/// duplicate launch is to go away quietly.
pub fn single_instance_or_exit(name: &str) {
    if !single_instance(name) {
        std::process::exit(0);
    }
}

/// Release physical RAM while idle: pages move to standby and fault back on
/// demand. Cheap, but it does cost soft faults afterwards, so call it on a
/// wallclock interval rather than every frame.
pub fn trim_ram() {
    #[cfg(windows)]
    unsafe {
        K32EmptyWorkingSet(GetCurrentProcess());
    }
}

/// Toggle `WS_EX_TRANSPARENT` so mouse input passes through to whatever is
/// underneath (used when a fullscreen game covers an overlay).
///
/// # Safety
/// `hwnd` must be a valid window handle owned by this process.
#[cfg(windows)]
pub unsafe fn set_clickthrough(hwnd: isize, on: bool) {
    let cur = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
    let next = if on { cur | WS_EX_TRANSPARENT } else { cur & !WS_EX_TRANSPARENT };
    if next != cur {
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next);
    }
}

/// Make a window behave like an overlay: never take focus, no taskbar button,
/// always on top.
///
/// # Safety
/// `hwnd` must be a valid window handle owned by this process.
#[cfg(windows)]
pub unsafe fn harden_overlay(hwnd: isize) {
    let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW);
    SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
}

#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn GetForegroundWindow() -> isize;
    fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
}
#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
    fn QueryFullProcessImageNameW(h: isize, flags: u32, buf: *mut u16, size: *mut u32) -> i32;
    fn CloseHandle(h: isize) -> i32;
}

/// Lowercased executable name of whatever window currently has focus, e.g.
/// `"league of legends.exe"`. `None` if there is no foreground window or the
/// process can't be opened (elevated processes, for instance).
///
/// Used to decide click-through by application rather than only by geometry:
/// some games in borderless mode don't quite cover the monitor, so a purely
/// geometric test misses them.
#[cfg(windows)]
pub fn foreground_process_name() -> Option<String> {
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd == 0 {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return None;
        }
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h == 0 {
            return None;
        }
        let mut buf = [0u16; 260];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        if ok == 0 {
            return None;
        }
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        Some(
            full.rsplit(['\\', '/'])
                .next()
                .unwrap_or(&full)
                .to_lowercase(),
        )
    }
}

#[cfg(not(windows))]
pub fn foreground_process_name() -> Option<String> {
    None
}

/// Path to a sibling executable in the same directory as this one. Every tool
/// finds its helpers this way, which is what lets the workspace ship a single
/// flat `target/release/` with no copy step.
pub fn sibling_exe(name: &str) -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(name)))
        .unwrap_or_else(|| name.into())
}
