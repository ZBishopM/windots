// ws-slide: interim workspace-switch slide animation for GlazeWM, standing in
// until the official animation PR (glzr-io/glazewm#1392) lands.
//
// This version OWNS the workspace hotkeys (Super+1..9); GlazeWM's own Super+N
// focus bindings are removed from its config. That lets ws-slide control the
// order: on Super+N it captures the current (outgoing) desktop, covers the
// monitor with it in a click-through layered window, THEN tells GlazeWM to
// switch. GlazeWM paints the new workspace *behind* the cover, so there is zero
// flash of it -- the new workspace is only ever revealed by the slide itself.
//
// A double-width filmstrip [outgoing | incoming] is scrolled inside a window
// pinned to the monitor (only the source offset moves), so it stays clipped to
// its monitor and the two halves of the desktop slide together toward the new
// content. The top BAR_H rows are left untouched so the bar never slides.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use serde::Deserialize;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::Message;

// A kept-open GlazeWM IPC connection, reused for every query + focus command.
type Sock = tungstenite::WebSocket<MaybeTlsStream<TcpStream>>;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    COLORREF, ERROR_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, GetDC, ReleaseDC, SelectObject,
    AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, CAPTUREBLT, DIB_RGB_COLORS,
    HDC, SRCCOPY,
};
use windows::Win32::Foundation::GetLastError;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{CreateMutexW, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LWIN, VK_RWIN, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, GetMessageW, PeekMessageW, PostThreadMessageW,
    RegisterClassW, SetWindowDisplayAffinity, SetWindowsHookExW, ShowWindow, UpdateLayeredWindow,
    KBDLLHOOKSTRUCT, MSG, PM_NOREMOVE, SW_HIDE, SW_SHOWNOACTIVATE, ULW_ALPHA, WDA_EXCLUDEFROMCAPTURE,
    WH_KEYBOARD_LL, WM_APP, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_USER, WNDCLASSW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

// Shared with the low-level keyboard hook (a bare C callback, hence globals).
static WIN_DOWN: AtomicBool = AtomicBool::new(false);
static WORKER_TID: AtomicU32 = AtomicU32::new(0);

const IPC: &str = "ws://127.0.0.1:6123";
const DUR: f32 = 0.20; // seconds per slide
const FRAME: Duration = Duration::from_millis(6);
const BAR_H: i32 = 34; // top-bar height to leave untouched so glaze-bar doesn't slide
const HOLD: Duration = Duration::from_millis(35); // brief cover while GlazeWM paints the new ws
const IO_TIMEOUT: Duration = Duration::from_millis(1200); // per IPC read/write; never block forever

// ---- GlazeWM IPC JSON ---------------------------------------------------------
#[derive(Deserialize)]
struct Resp {
    data: Option<MonData>,
}
#[derive(Deserialize)]
struct MonData {
    monitors: Vec<Mon>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Mon {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    #[serde(default)]
    children: Vec<Ws>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Ws {
    name: String,
    #[serde(default)]
    is_displayed: bool,
}

#[derive(Clone, Copy)]
struct MonRect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

// A monitor plus its currently displayed workspace and the workspace numbers it
// owns (so Super+N can be routed to the right monitor).
struct MonInfo {
    rect: MonRect,
    cur: i32,
    owns: Vec<i32>,
}

// Per-monitor filmstrip buffer (double the desktop width, minus the bar height).
struct MonBuf {
    strip: HDC,
    w: i32,
    h: i32,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    DefWindowProcW(h, m, w, l)
}

fn make_overlay() -> HWND {
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap();
        let cls = wide("wsslide_overlay");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: PCWSTR(cls.as_ptr()),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            PCWSTR(cls.as_ptr()),
            PCWSTR::null(),
            WS_POPUP,
            0,
            0,
            100,
            100,
            None,
            None,
            hinst,
            None,
        )
        .unwrap();
        // Excluded from screen capture: the overlay is visible to the user but a
        // BitBlt of the screen reads the *real desktop under it* -- that's how the
        // slide grabs the incoming workspace while covering it, with no flash.
        let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
        hwnd
    }
}

fn make_dib(screen: HDC, w: i32, h: i32) -> HDC {
    unsafe {
        let hdc = HDC(CreateCompatibleDC(screen).0);
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let hbm = CreateDIBSection(hdc, &bi, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
        SelectObject(hdc, hbm);
        hdc
    }
}

fn make_buf(screen: HDC, w_full: i32, h_full: i32) -> MonBuf {
    let (w, h) = (w_full, (h_full - BAR_H).max(1));
    MonBuf {
        strip: make_dib(screen, w * 2, h),
        w,
        h,
    }
}

// One frame of the filmstrip onto the pinned overlay: source offset `off` picks
// the visible w-wide slice, so scrolling `off` slides the strip without moving
// (or un-clipping) the window.
fn blit(overlay: HWND, screen: HDC, b: &MonBuf, m: &MonRect, off: i32) {
    unsafe {
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: 0,
        };
        let size = SIZE { cx: b.w, cy: b.h };
        let dst = POINT {
            x: m.x,
            y: m.y + BAR_H,
        };
        let src = POINT { x: off, y: 0 };
        let _ = UpdateLayeredWindow(
            overlay,
            screen,
            Some(&dst),
            Some(&size),
            b.strip,
            Some(&src),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );
    }
}

// Capture the current (pre-switch) desktop into the outgoing half and cover the
// monitor with it. Returns (start_off, end_off) for the slide. forward -> new
// enters from the right: strip is [old | new], scroll 0..w.
fn cover(overlay: HWND, screen: HDC, b: &MonBuf, m: &MonRect, forward: bool) -> (i32, i32) {
    unsafe {
        let (w, h) = (b.w, b.h);
        let cy = m.y + BAR_H;
        let (start, end) = if forward {
            let _ = BitBlt(b.strip, 0, 0, w, h, screen, m.x, cy, SRCCOPY | CAPTUREBLT);
            (0, w)
        } else {
            let _ = BitBlt(b.strip, w, 0, w, h, screen, m.x, cy, SRCCOPY | CAPTUREBLT);
            (w, 0)
        };
        blit(overlay, screen, b, m, start);
        let _ = ShowWindow(overlay, SW_SHOWNOACTIVATE);
        (start, end)
    }
}

// Capture the now-switched desktop into the incoming half and scroll the strip.
fn slide(overlay: HWND, screen: HDC, b: &MonBuf, m: &MonRect, forward: bool, start: i32, end: i32) {
    unsafe {
        let (w, h) = (b.w, b.h);
        let cy = m.y + BAR_H;
        if forward {
            let _ = BitBlt(b.strip, w, 0, w, h, screen, m.x, cy, SRCCOPY | CAPTUREBLT);
        } else {
            let _ = BitBlt(b.strip, 0, 0, w, h, screen, m.x, cy, SRCCOPY | CAPTUREBLT);
        }
        let t0 = Instant::now();
        loop {
            let t = (t0.elapsed().as_secs_f32() / DUR).min(1.0);
            let e = 1.0 - (1.0 - t).powi(3); // ease-out cubic
            let off = (start as f32 + (end - start) as f32 * e).round() as i32;
            blit(overlay, screen, b, m, off);
            if t >= 1.0 {
                break;
            }
            std::thread::sleep(FRAME);
        }
        let _ = ShowWindow(overlay, SW_HIDE);
    }
}

// --- IPC -----------------------------------------------------------------------
fn parse_mons(txt: &str) -> Option<Vec<MonInfo>> {
    let r = serde_json::from_str::<Resp>(txt).ok()?;
    Some(
        r.data?
            .monitors
            .into_iter()
            .map(|m| {
                let cur = m
                    .children
                    .iter()
                    .find(|w| w.is_displayed)
                    .and_then(|w| w.name.parse().ok())
                    .unwrap_or(0);
                let owns = m.children.iter().filter_map(|w| w.name.parse().ok()).collect();
                MonInfo {
                    rect: MonRect {
                        x: m.x,
                        y: m.y,
                        w: m.width,
                        h: m.height,
                    },
                    cur,
                    owns,
                }
            })
            .collect(),
    )
}

// Connect with read/write timeouts. Without them a wedged GlazeWM blocks the
// worker in sock.read() forever -- and since the hook swallows Super+N to keep it
// away from the taskbar, workspace switching would die silently with no way back.
// Bounded, the worst case is a short stall and a skipped animation.
fn connect_ipc() -> Option<Sock> {
    let (sock, _) = tungstenite::connect(IPC).ok()?;
    if let MaybeTlsStream::Plain(s) = sock.get_ref() {
        let _ = s.set_read_timeout(Some(IO_TIMEOUT));
        let _ = s.set_write_timeout(Some(IO_TIMEOUT));
    }
    Some(sock)
}

fn query_on<S: Read + Write>(sock: &mut tungstenite::WebSocket<S>) -> Option<Vec<MonInfo>> {
    sock.send(Message::Text("query monitors".into())).ok()?;
    for _ in 0..8 {
        match sock.read() {
            Ok(Message::Text(t)) => {
                if let Some(v) = parse_mons(&t) {
                    return Some(v);
                }
            }
            Ok(_) => {}
            Err(_) => return None, // timed out or closed -> caller falls back
        }
    }
    None
}

fn query_new() -> Option<Vec<MonInfo>> {
    query_on(&mut connect_ipc()?)
}

// Fire-and-wait on a throwaway connection (fallback when the persistent one died).
fn ipc_send(cmd: &str) {
    if let Some(mut s) = connect_ipc() {
        if s.send(Message::Text(cmd.into())).is_ok() {
            let _ = s.read();
        }
    }
}

fn send_on<S: Read + Write>(sock: &mut tungstenite::WebSocket<S>, cmd: &str) -> bool {
    sock.send(Message::Text(cmd.into())).is_ok() && sock.read().is_ok()
}

// Command on the persistent socket; reconnect + fall back to a throwaway one if
// it's stale, so the switch always happens even if the animation is skipped.
fn cmd(sock: &mut Option<Sock>, c: &str) {
    if sock.is_none() {
        *sock = connect_ipc();
    }
    let ok = matches!(sock.as_mut().map(|s| send_on(s, c)), Some(true));
    if !ok {
        *sock = None;
        ipc_send(c);
    }
}

fn query(sock: &mut Option<Sock>) -> Option<Vec<MonInfo>> {
    if sock.is_none() {
        *sock = connect_ipc();
    }
    let out = sock.as_mut().and_then(|s| query_on(s));
    if out.is_none() {
        *sock = None;
    }
    out
}

// Empty/inactive workspaces aren't listed under any monitor, so route by number:
// each monitor owns a contiguous block, so n belongs to the monitor whose
// smallest workspace is the largest one still <= n. (Falls back to the monitor
// with the overall smallest workspace, e.g. if the low workspaces are empty.)
fn target_monitor(mons: &[MonInfo], n: i32) -> usize {
    let min_of = |m: &MonInfo| m.owns.iter().min().copied().unwrap_or(i32::MAX);
    let (mut target, mut best_le) = (0usize, i32::MIN);
    let (mut fallback, mut lowest) = (0usize, i32::MAX);
    for (i, m) in mons.iter().enumerate() {
        let min = min_of(m);
        if min < lowest {
            lowest = min;
            fallback = i;
        }
        if min <= n && min > best_le {
            best_le = min;
            target = i;
        }
    }
    if best_le == i32::MIN {
        fallback
    } else {
        target
    }
}

fn handle_switch(
    n: i32,
    overlay: HWND,
    screen: HDC,
    rects: &[MonRect],
    bufs: &[MonBuf],
    sock: &mut Option<Sock>,
) {
    let focus = format!("command focus --workspace {n}");
    let Some(mons) = query(sock) else {
        cmd(sock, &focus);
        return;
    };
    let i = mons
        .iter()
        .position(|m| m.owns.contains(&n))
        .unwrap_or_else(|| target_monitor(&mons, n));
    let Some(info) = mons.get(i) else {
        cmd(sock, &focus);
        return;
    };
    let old = info.cur;
    if n == old || i >= bufs.len() {
        cmd(sock, &focus); // already there / no monitor: just switch, no animation
        return;
    }
    // Moving to a HIGHER workspace: old exits left, new enters from the right.
    let forward = n > old;
    // Cover with the current desktop BEFORE switching, so GlazeWM repaints hidden.
    let (start, end) = cover(overlay, screen, &bufs[i], &rects[i], forward);
    cmd(sock, &focus);
    std::thread::sleep(HOLD);
    slide(overlay, screen, &bufs[i], &rects[i], forward, start, end);
}

// Low-level keyboard hook. Win+1..9 are reserved by the shell (taskbar), so
// RegisterHotKey can't take them (err 1409); a WH_KEYBOARD_LL hook sees the key
// before the shell and swallows it. It only forwards the number to the worker --
// never animates inline -- so it returns immediately (LL hooks are time-limited).
unsafe extern "system" fn kb_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == 0 {
        // HC_ACTION
        let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let vk = kb.vkCode;
        let msg = wparam.0 as u32;
        let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
        if vk == VK_LWIN.0 as u32 || vk == VK_RWIN.0 as u32 {
            if down {
                WIN_DOWN.store(true, Ordering::Relaxed);
            } else if up {
                WIN_DOWN.store(false, Ordering::Relaxed);
            }
        } else if down && WIN_DOWN.load(Ordering::Relaxed) && (0x31..=0x39).contains(&vk) {
            // Win+Shift+N is GlazeWM's "move window to workspace" -> leave it alone.
            let shift = (GetAsyncKeyState(VK_SHIFT.0 as i32) as u16 & 0x8000) != 0;
            if !shift {
                let n = (vk - 0x30) as usize;
                let tid = WORKER_TID.load(Ordering::Relaxed);
                // Post to the worker, which covers the desktop and animates. If the
                // worker is gone (panicked, or still starting) fall back to a plain
                // switch on a detached thread: no animation, but Super+N must never
                // become a dead key -- GlazeWM no longer binds it, so this process
                // is the only thing that can service it.
                if tid == 0 || !PostThreadMessageW(tid, WM_APP, WPARAM(n), LPARAM(0)).is_ok() {
                    std::thread::spawn(move || ipc_send(&format!("command focus --workspace {n}")));
                }
                return LRESULT(1); // swallow so the taskbar doesn't also act on Win+N
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

// Worker thread: owns all the GDI (screen DC, overlay, per-monitor buffers) and
// runs the animation. Gets workspace numbers from the hook via thread messages,
// so the hook thread stays free to keep intercepting keys during a slide.
fn worker() {
    let screen = unsafe { GetDC(None) };
    let overlay = make_overlay();
    let rects: Vec<MonRect> = loop {
        if let Some(m) = query_new() {
            break m.iter().map(|mi| mi.rect).collect();
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    let bufs: Vec<MonBuf> = rects.iter().map(|r| make_buf(screen, r.w, r.h)).collect();

    unsafe {
        // Force a message queue to exist before advertising this thread's id.
        let mut m = MSG::default();
        let _ = PeekMessageW(&mut m, None, WM_USER, WM_USER, PM_NOREMOVE);
        WORKER_TID.store(GetCurrentThreadId(), Ordering::Relaxed);

        let mut sock: Option<Sock> = None;
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            if msg.message == WM_APP {
                handle_switch(msg.wParam.0 as i32, overlay, screen, &rects, &bufs, &mut sock);
            }
        }
        // Stop advertising: from here the hook must use its own fallback path
        // rather than posting into a queue nobody is reading.
        WORKER_TID.store(0, Ordering::Relaxed);
        for b in &bufs {
            let _ = DeleteDC(b.strip);
        }
        ReleaseDC(None, screen);
    }
}

fn main() {
    // Single instance: a second copy would install a second keyboard hook and
    // double-fire every Win+N. Held for the process lifetime.
    unsafe {
        let name = wide("Global\\ws-slide-singleton");
        let _mutex = CreateMutexW(None, false, PCWSTR(name.as_ptr()));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
    }
    std::thread::spawn(worker);
    while WORKER_TID.load(Ordering::Relaxed) == 0 {
        std::thread::sleep(Duration::from_millis(30));
    }
    unsafe {
        let hinst: HINSTANCE = GetModuleHandleW(None).unwrap().into();
        let _hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(kb_hook), hinst, 0).unwrap();
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {} // pump so the hook stays alive
    }
}
