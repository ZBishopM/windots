// Background daemon: no console.
#![windows_subsystem = "windows"]

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

fn undecorate(h: isize, no_minimize: bool) {
    if skip(h) {
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
        let mut next = cur & !(WS_CAPTION | WS_THICKFRAME);
        if no_minimize {
            next &= !(WS_MINIMIZEBOX | WS_MAXIMIZEBOX | WS_SYSMENU);
        }
        SetWindowLongPtrW(h, GWL_STYLE, next);
        // SWP_FRAMECHANGED is required: without it Windows keeps drawing the old
        // non-client area until something else forces a recalculation.
        SetWindowPos(h, 0, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED | SWP_NOACTIVATE);
    }
}

static NO_MINIMIZE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
            SetWindowPos(h, 0, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED | SWP_NOACTIVATE);
        }
    }
    let _ = std::fs::remove_file(state_path());
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let has = |f: &str| args.iter().any(|a| a == f);

    if has("--restore") {
        restore();
        return;
    }
    NO_MINIMIZE.store(has("--no-minimize"), std::sync::atomic::Ordering::Relaxed);

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
