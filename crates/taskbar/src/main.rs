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
mod appbar;
#[cfg(windows)]
mod tray;

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
        pub fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
    }

    #[link(name = "shell32")]
    extern "system" {
        pub fn SHAppBarMessage(msg: u32, data: *mut AppBarData) -> usize;
    }

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

    /// Hide it, or give it back.
    ///
    /// Hiding is NOT `SW_HIDE` any more. The tray has to be readable, and
    /// measured on this build, a hidden taskbar has no realised XAML tree at all
    /// -- UI Automation reports zero children, so there is nothing to read. So
    /// "hidden" now means shown, fully transparent and click-through; see
    /// `tray::make_invisible`. The user sees exactly the same thing (nothing),
    /// the work area is still handed back by ABM_SETSTATE, and the tray is
    /// legible.
    pub fn set_visible(show: bool) {
        for h in taskbars() {
            if show {
                crate::tray::make_normal(h);
                unsafe { ShowWindow(h, SW_SHOW) };
            } else {
                crate::tray::make_invisible(h);
            }
        }
    }

    /// Is the taskbar something the user can actually see?
    ///
    /// Not `IsWindowVisible`: while hidden our way the window IS visible, just
    /// at zero alpha. The layered bit is what distinguishes the two.
    pub fn is_visible() -> bool {
        const GWL_EXSTYLE: i32 = -20;
        const WS_EX_LAYERED: isize = 0x0008_0000;
        unsafe {
            taskbars()
                .first()
                .map(|&h| IsWindowVisible(h) != 0 && GetWindowLongPtrW(h, GWL_EXSTYLE) & WS_EX_LAYERED == 0)
                .unwrap_or(true)
        }
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
    if shown_marker().exists() {
        return; // deliberately shown
    }
    // Re-assert transparency rather than hiding. Explorer re-shows the window
    // itself as part of the auto-hide reveal, and it can drop the layered alpha
    // with it, so this has to run on every reveal or the bar flashes back in.
    tray::make_invisible(hwnd);
    tray::pin_revealed(hwnd);
}

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let has = |f: &str| args.iter().any(|a| a == f);

    // `--click <nombre>`: pulsar un icono de la bandeja. Es un proceso aparte y
    // no un mensaje al residente porque UIA puede tardar decenas de
    // milisegundos, y la barra no debe esperar a nadie para pintar el siguiente
    // fotograma.
    if let Some(i) = args.iter().position(|a| a == "--click") {
        if let Some(name) = args.get(i + 1) {
            if let Some(uia) = tray::Uia::new() {
                if let Some(&h) = win::taskbars().first() {
                    tray::make_invisible(h);
                    uia.invoke(h, name);
                }
            }
        }
        return;
    }

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
        // NO auto-hide. It was how the work area came back, but measured after
        // an explorer restart, auto-hide is what de-realises the tray's XAML
        // tree the moment the bar slides away -- UIA drops from 32 buttons to
        // zero, and an unrealised tray cannot be read. The bar stays shown (at
        // zero alpha, click-through) so the XAML lives, and the work area is
        // forced to the full screen by hand instead.
        // SIN auto-ocultar, y esta vez es definitivo. Todo lo demas se probo y
        // fallo, en este orden:
        //   - autohide: devuelve el area de trabajo, pero desrealiza el arbol
        //     XAML de la bandeja en cuanto la barra se desliza (32 botones -> 0)
        //     y entonces no hay bandeja que leer.
        //   - autohide + anclar la ventana en su posicion revelada: el arbol
        //     sigue muerto; la desrealizacion va por el estado interno del
        //     auto-ocultar, no por la posicion.
        //   - SPI_SETWORKAREA a mano: devuelve TRUE y no cambia nada; el appbar
        //     registrado de explorer manda sobre el area.
        //   - ABM_REMOVE del appbar de explorer: tampoco libera el area.
        // Conclusion: la reserva de 48px es inamovible mientras la barra este
        // realizada, y realizada tiene que estar. Los 48px los compensa GlazeWM
        // con outer_gap.bottom = -28px (ver su config.yaml).
        win::set_autohide(false);
        std::thread::sleep(std::time::Duration::from_millis(250));
        win::set_visible(false);
        if let Some(&h) = win::taskbars().first() {
            tray::pin_revealed(h);
        }
    } else {
        win::set_autohide(false);
        std::thread::sleep(std::time::Duration::from_millis(250));
        win::set_visible(true);
    }

    if !watch {
        return;
    }

    // Cada monitor sin barra de tareas recibe su propia reserva de 48px, para
    // que el outer_gap.bottom negativo de GlazeWM (que compensa la reserva del
    // primario) no empuje las ventanas fuera de pantalla en los demas. Ver
    // appbar.rs para la cadena completa de porques.
    appbar::reserve_secondary_strips();

    // Read the tray and publish it for the bar.
    //
    // Its own thread, and it never touches the window styles except through
    // `make_invisible`: a UIA call crosses into Explorer, and a slow or wedged
    // one must not be able to stall the message loop below that keeps the bar
    // from flashing back on screen.
    std::thread::Builder::new()
        .name("tray".into())
        .spawn(|| {
            let Some(uia) = tray::Uia::new() else { return };
            let mut last = 0u64;
            loop {
                if !shown_marker().exists() {
                    if let Some(&h) = win::taskbars().first() {
                        // Cheap, idempotent, and the only thing standing between
                        // the user and a taskbar reappearing.
                        tray::make_invisible(h);
                        tray::pin_revealed(h);
                        let items = uia.items(h);
                        let grabbed = tray::grab(h, &items);
                        let d = tray::digest(&grabbed);
                        if d != last {
                            last = d;
                            tray::publish(&grabbed);
                        }
                    }
                }
                // Two seconds: tray icons change on the scale of a program
                // starting or a notification badge appearing, and every tick
                // costs a PrintWindow of the whole taskbar.
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
        })
        .ok();

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
