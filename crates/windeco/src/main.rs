// Background daemon, but --list has to be able to print. Attaching to the
// parent's console when there is one keeps both: no window when the supervisor
// starts it, real output when a person runs it.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Strips window decorations, and optionally refuses to let windows minimise.
//!
//!   windeco              run as a daemon (strip on sight, keep watching)
//!   windeco --once       strip what is open right now and exit
//!   windeco --restore    put the decorations back on everything
//!
//! Why a daemon: new windows appear all the time, so a one-shot pass only fixes
//! what happened to be open. It listens on a WinEvent hook rather than polling.
//!
//! IMPORTANT LIMIT, learned by measuring rather than guessing: Electron and
//! Chromium apps (Vesktop, Discord, Claude, Zed) *do* carry WS_CAPTION, but they
//! paint their own title bar inside the web content. Stripping the native frame
//! does nothing visible for them -- their minimise/maximise/close buttons are
//! HTML. Only their own settings can remove those. This tool therefore helps
//! native-framed windows (Explorer, Office, most Win32 apps) and is harmless
//! elsewhere.

use std::collections::HashMap;
use std::sync::Mutex;

const GWL_STYLE: i32 = -16;
const WS_CAPTION: isize = 0x00C0_0000;
const WS_THICKFRAME: isize = 0x0004_0000;
const WS_MINIMIZEBOX: isize = 0x0002_0000;
const WS_MAXIMIZEBOX: isize = 0x0001_0000;
const WS_SYSMENU: isize = 0x0008_0000;

const SWP_NOMOVE: u32 = 0x0002;
const SWP_NOSIZE: u32 = 0x0001;
const SWP_NOZORDER: u32 = 0x0004;
const SWP_FRAMECHANGED: u32 = 0x0020;
const SWP_NOACTIVATE: u32 = 0x0010;

const EVENT_OBJECT_SHOW: u32 = 0x8002;
const EVENT_SYSTEM_MINIMIZESTART: u32 = 0x0016;
const OBJID_WINDOW: i32 = 0;
const WINEVENT_OUTOFCONTEXT: u32 = 0x0000;
const WINEVENT_SKIPOWNPROCESS: u32 = 0x0002;
const SW_RESTORE: i32 = 9;

#[link(name = "user32")]
extern "system" {
    fn GetWindowLongPtrW(h: isize, i: i32) -> isize;
    fn SetWindowLongPtrW(h: isize, i: i32, v: isize) -> isize;
    fn SetWindowPos(h: isize, after: isize, x: i32, y: i32, cx: i32, cy: i32, f: u32) -> i32;
    fn IsWindowVisible(h: isize) -> i32;
    fn IsWindow(h: isize) -> i32;
    fn GetWindowRect(h: isize, r: *mut Rect) -> i32;
    fn GetClassNameW(h: isize, buf: *mut u16, n: i32) -> i32;
    fn GetWindowTextW(h: isize, buf: *mut u16, n: i32) -> i32;
    fn GetWindowThreadProcessId(h: isize, pid: *mut u32) -> u32;
    fn EnumWindows(cb: extern "system" fn(isize, isize) -> i32, l: isize) -> i32;
    fn ShowWindow(h: isize, cmd: i32) -> i32;
    fn SetWinEventHook(min: u32, max: u32, dll: isize, cb: WinEventProc, pid: u32, tid: u32, flags: u32) -> isize;
    fn GetMessageW(msg: *mut Msg, h: isize, a: u32, b: u32) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
}

type WinEventProc = extern "system" fn(isize, u32, isize, i32, i32, u32, u32);

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}
#[repr(C)]
#[derive(Default)]
struct Msg {
    hwnd: isize,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt_x: i32,
    pt_y: i32,
}

/// Original styles, so --restore is exact rather than a guess.
static ORIGINAL: Mutex<Option<HashMap<isize, isize>>> = Mutex::new(None);

fn state_path() -> std::path::PathBuf {
    rice_common::config::config_path("windeco-state.json")
}

fn class_of(h: isize) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { GetClassNameW(h, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn proc_of(h: isize) -> String {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(h, &mut pid) };
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(a: u32, i: i32, p: u32) -> isize;
        fn QueryFullProcessImageNameW(h: isize, f: u32, b: *mut u16, s: *mut u32) -> i32;
        fn CloseHandle(h: isize) -> i32;
    }
    unsafe {
        let ph = OpenProcess(0x1000, 0, pid);
        if ph == 0 {
            return String::new();
        }
        let mut buf = [0u16; 260];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(ph, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(ph);
        if ok == 0 {
            return String::new();
        }
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        full.rsplit(['\\', '/']).next().unwrap_or("").to_lowercase()
    }
}

/// Windows we must never touch: our own overlays, the shell, and anything
/// without a real frame.
fn skip(h: isize) -> bool {
    if unsafe { IsWindowVisible(h) } == 0 {
        return true;
    }
    let mut r = Rect::default();
    if unsafe { GetWindowRect(h, &mut r) } == 0 {
        return true;
    }
    // Ignore tiny/utility windows; a real app window is at least this big.
    if r.right - r.left < 300 || r.bottom - r.top < 200 {
        return true;
    }
    let cls = class_of(h);
    if matches!(
        cls.as_str(),
        "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "Progman" | "WorkerW" | "Windows.UI.Core.CoreWindow"
    ) {
        return true;
    }
    let p = proc_of(h);
    // Our own tools draw their own chrome already.
    matches!(
        p.as_str(),
        "glaze-bar.exe" | "shadowplay-notify.exe" | "ws-slide.exe" | "explorer.exe" | ""
    )
}

/// Strip the frame. `no_minimize` additionally fights windows that minimise
/// themselves; the buttons are removed either way, because a bar with no
/// caption still leaves a system menu on Alt+Space otherwise.
fn undecorate(h: isize, no_minimize: bool) {
    if skip(h) || !allowed(h) {
        return;
    }
    unsafe {
        let cur = GetWindowLongPtrW(h, GWL_STYLE);
        if cur & (WS_CAPTION | WS_THICKFRAME) == 0 {
            return; // already bare
        }
        // Remember the original once per window.
        {
            let mut g = ORIGINAL.lock().unwrap();
            let map = g.get_or_insert_with(HashMap::new);
            map.entry(h).or_insert(cur);
        }
        let mut next = cur & !(WS_CAPTION | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX);
        if no_minimize {
            next &= !WS_SYSMENU;
        }
        SetWindowLongPtrW(h, GWL_STYLE, next);
        relayout(h);
    }
}

/// Make the window recalculate its frame AND its contents.
///
/// SWP_FRAMECHANGED alone is not enough, and that is what made the first attempt
/// at this leave debris all over the screen. It tells Windows to recompute the
/// non-client area, but an application that draws its own chrome inside the
/// client area -- WezTerm with `window_decorations = 'RESIZE'`, Firefox, every
/// Electron app -- never finds out its client rect moved, so it keeps rendering
/// at the old offset and its old tab bar stays painted where it was. GlazeWM's
/// wm-redraw does not fix it either; it was tried.
///
/// A real size change does, because it delivers WM_SIZE and the app relayouts.
/// One pixel down and back is invisible and costs a single extra frame.
unsafe fn relayout(h: isize) {
    const SWP: u32 = SWP_NOMOVE | SWP_NOZORDER | SWP_FRAMECHANGED | SWP_NOACTIVATE;
    let mut r = Rect::default();
    if GetWindowRect(h, &mut r) == 0 {
        SetWindowPos(h, 0, 0, 0, 0, 0, SWP | SWP_NOSIZE);
        return;
    }
    let (w, ht) = (r.right - r.left, r.bottom - r.top);
    SetWindowPos(h, 0, 0, 0, w, ht - 1, SWP);
    SetWindowPos(h, 0, 0, 0, w, ht, SWP);
}

static NO_MINIMIZE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `--only <substring>`: act on matching executables only. This tool's failure
/// mode is mangling an application someone is using, so being able to try it on
/// one first is worth the flag.
static ONLY: Mutex<Option<String>> = Mutex::new(None);

fn allowed(h: isize) -> bool {
    match ONLY.lock().unwrap().as_deref() {
        Some(pat) => proc_of(h).contains(pat),
        None => true,
    }
}

extern "system" fn list_cb(h: isize, _l: isize) -> i32 {
    if !skip(h) {
        let cur = unsafe { GetWindowLongPtrW(h, GWL_STYLE) };
        let bare = cur & (WS_CAPTION | WS_THICKFRAME) == 0;
        let mut buf = [0u16; 120];
        let n = unsafe { GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32) };
        let title = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
        println!(
            "{:<10} {:<22} 0x{:08X}  {}",
            if bare { "already" } else { "STRIP" },
            proc_of(h),
            cur,
            title
        );
    }
    1
}

extern "system" fn enum_cb(h: isize, _l: isize) -> i32 {
    undecorate(h, NO_MINIMIZE.load(std::sync::atomic::Ordering::Relaxed));
    1
}

extern "system" fn hook_cb(_hook: isize, event: u32, hwnd: isize, id_object: i32, _idc: i32, _t: u32, _tm: u32) {
    if id_object != OBJID_WINDOW || hwnd == 0 || unsafe { IsWindow(hwnd) } == 0 {
        return;
    }
    let no_min = NO_MINIMIZE.load(std::sync::atomic::Ordering::Relaxed);
    match event {
        EVENT_OBJECT_SHOW => undecorate(hwnd, no_min),
        EVENT_SYSTEM_MINIMIZESTART if no_min => {
            // Styles alone don't stop this: an app can call ShowWindow(SW_MINIMIZE)
            // on itself regardless of WS_MINIMIZEBOX, and "minimise to tray" apps
            // do exactly that. Undo it as it happens.
            if !skip(hwnd) {
                unsafe { ShowWindow(hwnd, SW_RESTORE) };
            }
        }
        _ => {}
    }
}

fn save_state() {
    if let Some(map) = ORIGINAL.lock().unwrap().as_ref() {
        let list: Vec<(String, String)> = map
            .iter()
            .map(|(h, s)| (h.to_string(), s.to_string()))
            .collect();
        if let Ok(j) = serde_json::to_string(&list) {
            let _ = std::fs::write(state_path(), j);
        }
    }
}

fn restore() {
    let Ok(raw) = std::fs::read_to_string(state_path()) else {
        return;
    };
    let Ok(list) = serde_json::from_str::<Vec<(String, String)>>(&raw) else {
        return;
    };
    for (h, s) in list {
        let (Ok(h), Ok(s)) = (h.parse::<isize>(), s.parse::<isize>()) else {
            continue;
        };
        if unsafe { IsWindow(h) } == 0 {
            continue; // window is gone
        }
        unsafe {
            SetWindowLongPtrW(h, GWL_STYLE, s);
            relayout(h);
        }
    }
    let _ = std::fs::remove_file(state_path());
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    #[cfg(windows)]
    unsafe {
        #[link(name = "kernel32")]
        extern "system" {
            fn AttachConsole(pid: u32) -> i32;
        }
        AttachConsole(u32::MAX); // ATTACH_PARENT_PROCESS; fails harmlessly if none
    }

    if has("--restore") {
        restore();
        return;
    }
    // --list changes nothing: it names every window that WOULD be stripped, and
    // the ones already bare. Worth having, because the failure mode of this tool
    // is "it silently mangled an application you were using".
    if has("--list") {
        unsafe { EnumWindows(list_cb, 0) };
        return;
    }
    NO_MINIMIZE.store(has("--no-minimize"), std::sync::atomic::Ordering::Relaxed);
    if let Some(i) = args.iter().position(|a| a == "--only") {
        if let Some(v) = args.get(i + 1) {
            *ONLY.lock().unwrap() = Some(v.to_lowercase());
        }
    }

    // First pass over what is already open.
    unsafe { EnumWindows(enum_cb, 0) };
    save_state();

    if has("--once") {
        return;
    }

    // Then keep watching. WINEVENT_SKIPOWNPROCESS so we never chase our own
    // windows; OUTOFCONTEXT so nothing is injected into other processes.
    unsafe {
        SetWinEventHook(
            EVENT_SYSTEM_MINIMIZESTART,
            EVENT_SYSTEM_MINIMIZESTART,
            0,
            hook_cb,
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );
        SetWinEventHook(
            EVENT_OBJECT_SHOW,
            EVENT_OBJECT_SHOW,
            0,
            hook_cb,
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );
        let mut msg = Msg::default();
        let mut ticks = 0u32;
        while GetMessageW(&mut msg, 0, 0, 0) > 0 {
            DispatchMessageW(&msg);
            ticks += 1;
            if ticks % 64 == 0 {
                save_state(); // keep --restore usable across a crash
            }
        }
    }
}
