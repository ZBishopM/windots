#![windows_subsystem = "windows"] // no console window

mod tray;

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
    fn GetClassNameW(hwnd: isize, buf: *mut u16, n: i32) -> i32;
    // Recorrido del orden Z, para saber quien cubre un monitor sin depender de
    // quien tiene el foco. Ver `fullscreen_on_monitor`.
    fn GetTopWindow(parent: isize) -> isize;
    fn GetWindow(hwnd: isize, cmd: u32) -> isize;
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
    fn SetLayeredWindowAttributes(hwnd: isize, key: u32, alpha: u8, flags: u32) -> i32;
    fn SetWindowPos(hwnd: isize, after: isize, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32;
}

/// El clic DERECHO atraviesa la barra siempre.
///
/// La barra no hace absolutamente nada con el boton secundario: cero
/// `secondary_clicked`, cero menus contextuales en todo el crate. Se lo tragaba
/// y punto, asi que el menu contextual de lo que hubiera debajo -- el escritorio,
/// una pestaña del navegador, el reproductor -- no salia nunca en los 34 px de
/// arriba.
///
/// `WS_EX_TRANSPARENT` deja pasar los dos botones, pero solo esta puesto sobre
/// pantalla completa. Para el resto del tiempo hace falta responder al
/// hit-testing: `WM_NCHITTEST` llega ANTES de que se despache el mensaje del
/// boton, y en ese instante `GetAsyncKeyState` ya ve el derecho pulsado.
/// Devolver `HTTRANSPARENT` manda ese clic a la ventana de debajo.
///
/// Estrecho a proposito: solo con el derecho fisicamente pulsado. El izquierdo,
/// el hover y el resto de mensajes caen en `DefSubclassProc` sin tocar.
#[cfg(windows)]
mod rclick {
    pub const WM_NCHITTEST: u32 = 0x0084;
    pub const HTTRANSPARENT: isize = -1;
    pub const VK_RBUTTON: i32 = 0x02;
    /// Identificador del subclaseo. Cualquier valor sirve; solo tiene que ser
    /// el mismo para poner y quitar.
    pub const ID: usize = 0x9146;

    #[link(name = "comctl32")]
    extern "system" {
        pub fn SetWindowSubclass(hwnd: isize, proc_: SubclassProc, id: usize, refdata: usize) -> i32;
        pub fn DefSubclassProc(hwnd: isize, msg: u32, w: usize, l: isize) -> isize;
    }
    #[link(name = "user32")]
    extern "system" {
        pub fn GetAsyncKeyState(vk: i32) -> i16;
    }

    pub type SubclassProc =
        unsafe extern "system" fn(isize, u32, usize, isize, usize, usize) -> isize;

    /// La barra entera deja pasar el raton (pantalla completa debajo).
    ///
    /// AQUI Y NO EN `WS_EX_TRANSPARENT`, y esto costo una medicion: ese bit
    /// SOLO no hace click-through. Comprobado con una ventana de 1920x1080
    /// enfocada y el bit puesto -- `WindowFromPoint(400,10)` seguia devolviendo
    /// glaze-bar, o sea que el clic era suyo. Funciona acompañado de
    /// `WS_EX_LAYERED` (asi lo hace ws-slide), pero anadir LAYERED a una ventana
    /// con contexto OpenGL obliga a mantener sus atributos y puede romper el
    /// dibujado.
    ///
    /// Responder al hit-testing no tiene ese problema y es exactamente la misma
    /// pregunta: `WM_NCHITTEST` es lo que Windows usa para decidir de quien es
    /// el clic, y `HTTRANSPARENT` significa "no es mio, mira debajo".
    pub static PASAR: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    /// HWND de la barra, para el hilo vigilante. 0 = todavia no resuelto.
    pub static HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

    /// Por que se decidio lo ultimo. Se registra en el log al cambiar: mirar
    /// solo el resultado obligaba a reconstruir el razonamiento desde fuera, y
    /// eso costo una tarde con un flip-flop que no se explicaba.
    ///
    /// 0 nadie cubre | 1 el primer plano cubre | 2 lo cubre otra del orden Z
    /// 3 esta en clickthrough_apps | 4 hay una ventana normal delante que no cubre
    pub static MOTIVO: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

    pub fn motivo_txt(c: u8) -> &'static str {
        match c {
            1 => "el primer plano cubre el monitor",
            2 => "otra ventana del orden Z lo cubre",
            3 => "coincide con clickthrough_apps",
            4 => "hay una ventana normal delante que NO cubre",
            _ => "nadie cubre el monitor",
        }
    }

    pub unsafe extern "system" fn proc_(
        hwnd: isize,
        msg: u32,
        w: usize,
        l: isize,
        _id: usize,
        _refdata: usize,
    ) -> isize {
        if msg == WM_NCHITTEST {
            let pasar = PASAR.load(std::sync::atomic::Ordering::Relaxed);
            // El derecho SIEMPRE atraviesa, mande lo que mande PASAR: la barra no
            // hace nada con el boton secundario.
            let derecho = (GetAsyncKeyState(VK_RBUTTON) as u16 & 0x8000) != 0;
            if pasar || derecho {
                return HTTRANSPARENT;
            }
        }
        DefSubclassProc(hwnd, msg, w, l)
    }
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
/// Class name of a window, for deciding what it is rather than guessing.
#[cfg(windows)]
unsafe fn class_of(hwnd: isize) -> String {
    let mut buf = [0u16; 128];
    let n = GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// Windows that cover the monitor but are NOT an application taking the screen.
///
/// The desktop is the one that matters. `Progman` (and the `WorkerW` that
/// wallpaper tools slide behind it) is a window the size of the monitor, so a
/// plain click on empty desktop -- the bare strip at the bottom where the
/// taskbar used to be -- made the geometric test say "fullscreen" and the bar
/// hid itself. Explorer's own bar is listed for the same reason: it is now kept
/// realised at zero alpha so the tray can be read, and it must not count as an
/// application either.
#[cfg(windows)]
fn is_shell_surface(class: &str) -> bool {
    matches!(
        class,
        // Progman y WorkerW son el escritorio; SysListView32 es su rejilla de
        // iconos, que toma el foco cuando se selecciona algo en el escritorio.
        "Progman" | "WorkerW" | "SysListView32"
        // La barra de tareas de Windows. Ahora se queda realizada a alfa cero
        // para poder leer la bandeja (ver crates/taskbar/src/tray.rs), asi que
        // puede aparecer como ventana enfocada sin que nadie la vea.
            | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
    )
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
    // A tiled/maximised window sits BELOW the bar; only a true fullscreen window
    // covers the monitor's top strip too. A few pixels of slack, because some
    // borderless windows land a hair inside the monitor rect and an exact
    // comparison then misses them.
    const SLACK: i32 = 4;
    let cubre = |w: isize| -> bool {
        let mut r = Rect { left: 0, top: 0, right: 0, bottom: 0 };
        if GetWindowRect(w, &mut r) == 0 {
            return false;
        }
        r.left <= mi.rc_monitor.left + SLACK
            && r.top <= mi.rc_monitor.top + SLACK
            && r.right >= mi.rc_monitor.right - SLACK
            && r.bottom >= mi.rc_monitor.bottom - SLACK
    };

    // Primero la ventana en primer plano, que es el caso normal y el barato.
    let fg = GetForegroundWindow();
    if fg != 0 && fg != my && !is_shell_surface(&class_of(fg)) && cubre(fg) {
        rclick::MOTIVO.store(1, std::sync::atomic::Ordering::Relaxed);
        return true;
    }

    // Y si no, se busca QUIEN CUBRE ESTE MONITOR, tenga el foco o no.
    //
    // Mirar solo el primer plano fallaba en el caso que mas molesta: video a
    // pantalla completa en un monitor mientras el juego tiene el foco en el
    // otro. Para la barra del video, la ventana enfocada esta en la otra
    // pantalla y no cubre nada suyo, asi que nunca recibia click-through y se
    // comia los clics del reproductor.
    //
    // Se recorre el orden Z de arriba abajo y se para en la primera ventana
    // visible que no sea de las nuestras. Si esa cubre el monitor, el clic tiene
    // que atravesar.
    const GW_HWNDNEXT: u32 = 2;
    const GWL_EXSTYLE_: i32 = -20;
    const WS_EX_TOOLWINDOW_: isize = 0x80;
    const WS_EX_TRANSPARENT_: isize = 0x20;
    let mut w = GetTopWindow(0);
    let mut vistas = 0;
    while w != 0 && vistas < 40 {
        vistas += 1;
        if w != my && IsWindowVisible(w) != 0 {
            let ex = GetWindowLongPtrW(w, GWL_EXSTYLE_);
            // Se saltan overlays: barras, toasts y cualquier cosa ya
            // click-through. Si no, la primera ventana del orden Z seria
            // siempre la otra barra.
            let overlay = ex & (WS_EX_TOOLWINDOW_ | WS_EX_TRANSPARENT_) != 0;
            if !overlay && !is_shell_surface(&class_of(w)) {
                let mut r = Rect { left: 0, top: 0, right: 0, bottom: 0 };
                if GetWindowRect(w, &mut r) != 0 && r.right > r.left && r.bottom > r.top {
                    // La primera ventana real de este monitor manda: si no cubre,
                    // hay algo normal delante y la barra debe seguir pulsable.
                    if MonitorFromWindow(w, 2) == mon {
                        let c = cubre(w);
                        rclick::MOTIVO.store(if c { 2 } else { 4 }, std::sync::atomic::Ordering::Relaxed);
                        return c;
                    }
                }
            }
        }
        w = GetWindow(w, GW_HWNDNEXT);
    }
    rclick::MOTIVO.store(0, std::sync::atomic::Ordering::Relaxed);
    false
}

/// Reserva la franja de la barra en el AREA DE TRABAJO de su monitor.
///
/// EL PROBLEMA: una ventana MAXIMIZADA se dimensiona con el area de trabajo, y
/// hasta ahora esa area empezaba en y=0 -- o sea, por debajo de la barra. La
/// barra queda encima y sus workspaces se pisan con la barra de herramientas de
/// la aplicacion. Con ventanas teseladas no pasa porque GlazeWM deja su propio
/// hueco arriba; con maximizadas no habia nada que lo impidiera.
///
/// Registrarse como appbar es el mecanismo de Windows para justo esto, el mismo
/// que usa la barra de tareas. A partir de aqui NADA se maximiza por encima de
/// la barra, en ninguna aplicacion.
///
/// NO arregla la pantalla completa de verdad, y no puede: una ventana a pantalla
/// completa ignora el area de trabajo a proposito, porque la pantalla es suya.
/// Ahi la barra sigue dibujandose encima -- que es lo que se pidio, visible
/// siempre -- pero al menos los clics la atraviesan.
///
/// OJO con el hueco de GlazeWM: tila DENTRO del area de trabajo, asi que su
/// `outer_gap.top` deja de tener que compensar la altura de la barra. Estaba en
/// 42 (34 de barra + 8 de aire) y pasa a 8, o el hueco se contaria dos veces.
#[cfg(windows)]
mod appbar {
    #[repr(C)]
    pub struct Data {
        pub cb: u32,
        pub hwnd: isize,
        pub callback: u32,
        pub edge: u32,
        pub rc: super::Rect,
        pub lparam: isize,
    }
    pub const ABM_NEW: u32 = 0x0;
    pub const ABM_QUERYPOS: u32 = 0x2;
    pub const ABM_SETPOS: u32 = 0x3;
    pub const ABE_TOP: u32 = 1;
    /// Mensaje propio con el que Windows avisa de cambios; hay que dar uno
    /// aunque no se atienda, o el registro se rechaza.
    pub const WM_APPBAR: u32 = 0x0400 + 0x321;

    #[link(name = "shell32")]
    extern "system" {
        pub fn SHAppBarMessage(msg: u32, data: *mut Data) -> usize;
    }

    /// Reserva `alto` pixeles en la parte de arriba de `mon`.
    pub unsafe fn reservar(hwnd: isize, mon: super::Rect, alto: i32) {
        let mut d = Data {
            cb: std::mem::size_of::<Data>() as u32,
            hwnd,
            callback: WM_APPBAR,
            edge: ABE_TOP,
            rc: super::Rect { left: 0, top: 0, right: 0, bottom: 0 },
            lparam: 0,
        };
        SHAppBarMessage(ABM_NEW, &mut d);
        d.edge = ABE_TOP;
        d.rc = super::Rect {
            left: mon.left,
            top: mon.top,
            right: mon.right,
            bottom: mon.top + alto,
        };
        // QUERYPOS deja que Windows ajuste el rectangulo si otro appbar ya ocupa
        // ese borde; saltarselo es como se acaba con dos barras solapadas.
        SHAppBarMessage(ABM_QUERYPOS, &mut d);
        d.rc.top = mon.top;
        d.rc.bottom = mon.top + alto;
        SHAppBarMessage(ABM_SETPOS, &mut d);
    }

    // Aqui vivia un `liberar()` con ABM_REMOVE que nadie llamaba nunca.
    // Se quita en vez de cablearlo: quien reinicia la barra es el supervisor y
    // lo hace matando el proceso, asi que una ruta de cierre ordenado no
    // llegaria a correr casi nunca. Windows suelta el registro al destruirse
    // la ventana -- comprobado a lo largo de muchos ciclos de despliegue, sin
    // que el area de trabajo se quedara encogida ni una vez.
}

/// Vigila si toca dejar pasar el raton, en su propio hilo y cada 150 ms.
///
/// POR QUE NO EN `update()`: la decision estaba atada al repintado, y la barra
/// en reposo repinta una vez por segundo. Sumando la puerta de 0,5 s del tick,
/// entre poner una aplicacion a pantalla completa y que la barra dejase de comer
/// clics pasaba hasta segundo y medio -- justo el rato en que uno hace el clic
/// que se pierde.
///
/// Aqui no se toca egui ni nada suyo: solo se lee la geometria de las ventanas y
/// se escribe un atomico que consulta el hit-test. Por eso puede ir en un hilo
/// aparte sin sincronizar con el dibujado.
#[cfg(windows)]
fn spawn_clickthrough_watcher() {
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_millis(150));
        let hwnd = rclick::HWND.load(std::sync::atomic::Ordering::Relaxed);
        if hwnd == 0 {
            continue;
        }
        let want = unsafe { should_clickthrough(hwnd) };
        if want != rclick::PASAR.load(std::sync::atomic::Ordering::Relaxed) {
            rclick::PASAR.store(want, std::sync::atomic::Ordering::Relaxed);
            unsafe { set_clickthrough(hwnd, want) };
        }
    });
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

/// Dejar pasar el raton A OTRO PROCESO exige LAYERED + TRANSPARENT juntos.
///
/// Los dos intentos anteriores fallaron por la misma razon y conviene que quede
/// escrito:
///
/// 1. `WS_EX_TRANSPARENT` solo: el bit se pone, pero la barra se sigue quedando
///    el clic. Medido con `SendMessage(WM_NCHITTEST)`, devolvia HTCLIENT.
/// 2. Responder `HTTRANSPARENT` al hit-test: la barra deja de quedarselo -- eso
///    si funciona -- pero el clic **no llega a nadie**. HTTRANSPARENT continua
///    la busqueda entre las ventanas del MISMO hilo; cruzar de proceso no lo
///    hace. Medido sinteticamente: un clic en (400,400) llego a la ventana de
///    debajo y el de (400,10) se perdio.
///
/// La pareja LAYERED + TRANSPARENT es la que Windows enruta de verdad, y es la
/// que ya usa ws-slide para su superposicion.
///
/// LAYERED solo se pone MIENTRAS hace falta. Una ventana en capas sin atributos
/// no se compone -- se queda invisible -- asi que hay que fijar
/// `SetLayeredWindowAttributes`, y eso sustituye el alfa por pixel de la barra
/// por uno uniforme. Ponerlo siempre cambiaria como se ve en el escritorio
/// normal; ponerlo solo sobre una aplicacion a pantalla completa lo limita a los
/// ratos en que lo que importa es que el clic pase.
#[cfg(windows)]
unsafe fn set_clickthrough(hwnd: isize, on: bool) {
    const GWL_EXSTYLE: i32 = -20;
    const WS_EX_TRANSPARENT: isize = 0x20;
    const WS_EX_LAYERED: isize = 0x0008_0000;
    const LWA_ALPHA: u32 = 0x2;
    let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
    let new = if on {
        ex | WS_EX_TRANSPARENT | WS_EX_LAYERED
    } else {
        ex & !WS_EX_TRANSPARENT & !WS_EX_LAYERED
    };
    if new == ex {
        return;
    }
    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new);
    if on {
        // 255, NO la opacidad de la barra.
        //
        // El alfa de la capa MULTIPLICA lo ya pintado, y el fondo de la barra ya
        // lleva bar_opacity dentro. Poner aqui esa misma opacidad la aplicaba dos
        // veces (0,78 x 0,78 = 0,61) y la barra se veia mas transparente justo al
        // entrar en este modo -- que es lo que se noto desde fuera. Con 255 la
        // capa no altera nada y se ve igual que antes.
        SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA);
    }
    // FRAMECHANGED: sin el, Windows sigue usando las metricas viejas y el cambio
    // de estilo puede no aplicarse hasta el siguiente evento de ventana.
    const SWP: u32 = 0x0002 | 0x0001 | 0x0004 | 0x0020 | 0x0010;
    SetWindowPos(hwnd, 0, 0, 0, 0, 0, SWP);
}

// Environment flags are read once, not per call / per frame: dlog ran an
// env::var_os on every invocation and the icon-test check ran one every frame.
fn env_flag(name: &'static str, cell: &'static std::sync::OnceLock<bool>) -> bool {
    *cell.get_or_init(|| std::env::var_os(name).is_some())
}
static LOG_ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
static ICONTEST_ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

/// Cambios de estado que importan, siempre registrados.
///
/// Aparte de `dlog` a proposito. `dlog` esta detras de una variable de entorno
/// porque escribe cada segundo, y para activarla hay que saber que existe y
/// reiniciar la barra -- justo lo que no sirve cuando el fallo ya ocurrio. Esto
/// se escribe siempre, en el mismo directorio donde ya viven los registros del
/// supervisor y de notifyd, y solo ante transiciones, que son unas pocas al dia.
fn elog(msg: &str) {
    let path = config::config_path(r"logs\glaze-bar.log");
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Corte simple en vez de rotacion: esto anota transiciones, no un flujo. Si
    // alguna vez llega a un cuarto de mega es que algo esta oscilando, y en ese
    // caso lo ultimo es lo que interesa.
    if std::fs::metadata(&path).map(|m| m.len() > 256 * 1024).unwrap_or(false) {
        let _ = std::fs::remove_file(&path);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{} {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"), msg);
    }
}

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
    /// Bateria de CADA dispositivo conectado, no solo del que suena. Sustituye
    /// a la velocidad de subida/bajada, que se miraba una vez al mes; quedarte
    /// sin auriculares a media partida se nota siempre.
    baterias: Vec<rice_common::battery::Bateria>,

    /// Lo que el medidor de consumo dejo escrito en su ultimo muestreo.
    consumo: Option<ConsumoAhora>,
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
    /// Empty name = master. El estado deseado va explicito, no se invierte en el
    /// worker: entre el clic y la llamada COM pueden pasar milisegundos y otra
    /// cosa puede haber cambiado el silencio por su cuenta.
    Mute(String, bool),
    /// Flip the default output between the two devices actually in use.
    SwapOutput,
}

/// One row of the widget. La primera fila es siempre master.
///
/// `muted` estaba disponible en `rice_common::audio::Session` desde el principio
/// y esta pantalla lo tiraba: se quedaba solo con el nivel. Una app silenciada se
/// veia igual que una al 72%, que es exactamente como se pierde media hora
/// buscando por que no suena un juego.
#[derive(Clone)]
struct VolRow {
    name: String,
    level: f32,
    muted: bool,
}

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
                let mut mutes: std::collections::HashMap<String, bool> = Default::default();
                match first {
                    VolMsg::Set(n, v) => {
                        pending.insert(n, v);
                    }
                    VolMsg::Mute(n, m) => {
                        mutes.insert(n, m);
                    }
                    _ => {}
                }
                while let Ok(m) = rx.try_recv() {
                    match m {
                        VolMsg::Refresh => refresh = true,
                        VolMsg::SwapOutput => swap = true,
                        VolMsg::Set(n, v) => {
                            pending.insert(n, v);
                        }
                        VolMsg::Mute(n, m) => {
                            mutes.insert(n, m);
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
                for (name, m) in mutes {
                    if name.is_empty() {
                        rice_common::audio::set_master_mute(m);
                    } else {
                        rice_common::audio::set_app_mute(&name, m);
                    }
                    // Releer siempre tras silenciar: si la llamada fallo (la app
                    // cerro su sesion entre el clic y esto), el icono debe volver
                    // a lo que el sistema diga, no a lo que pedimos.
                    refresh = true;
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
                    let mut rows: Vec<VolRow> = vec![VolRow {
                        name: "master".into(),
                        level: rice_common::audio::master_volume().unwrap_or(0.0),
                        muted: rice_common::audio::master_muted(),
                    }];
                    for s in rice_common::audio::sessions().into_iter().take(4) {
                        rows.push(VolRow {
                            name: s.name.trim_end_matches(".exe").to_string(),
                            level: s.volume,
                            muted: s.muted,
                        });
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
    fn mute(&self, name: &str, muted: bool) {
        let _ = self.tx.send(VolMsg::Mute(name.to_string(), muted));
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
    /// Bateria del dispositivo, cuando es uno de los dos que sabemos preguntar.
    bateria: Option<rice_common::battery::Bateria>,
}

enum DevMsg {
    Refresh,
    /// Look for nearby devices that are not paired yet.
    Scan,
    /// Pair with one of them, by WinRT device id.
    Pair(String),
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
    /// Nearby unpaired devices, and whether a scan is running. A scan takes
    /// ~22s (Windows runs a full inquiry and only answers at the end), so the
    /// UI has to say so rather than looking frozen.
    pairable: Arc<Mutex<(bool, Vec<rice_common::bluetooth::Pairable>)>>,
}

impl DevCtl {
    fn new(ctx: egui::Context) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<DevMsg>();
        let state: Arc<Mutex<Vec<DevRow>>> = Arc::new(Mutex::new(Vec::new()));
        let pairable: Arc<Mutex<(bool, Vec<rice_common::bluetooth::Pairable>)>> =
            Arc::new(Mutex::new((false, Vec::new())));
        let out = state.clone();
        let pair_out = pairable.clone();
        std::thread::spawn(move || {
            while let Ok(msg) = rx.recv() {
                match msg {
                    DevMsg::Refresh => {}
                    DevMsg::Scan => {
                        pair_out.lock().unwrap().0 = true;
                        ctx.request_repaint();
                        let found = rice_common::bluetooth::scan_pairable();
                        *pair_out.lock().unwrap() = (false, found);
                    }
                    DevMsg::Pair(id) => {
                        pair_out.lock().unwrap().0 = true;
                        ctx.request_repaint();
                        let res = rice_common::bluetooth::pair(&id);
                        // Drop it from the pairable list either way: on success
                        // it is now a normal device and appears in the rows
                        // below, on failure a stale entry would just invite
                        // another doomed attempt.
                        {
                            let mut g = pair_out.lock().unwrap();
                            g.0 = false;
                            g.1.retain(|p| p.id != id);
                        }
                        if let Err(e) = res {
                            let ev = rice_common::event::IslandEvent::new(
                                "warn",
                                "Bluetooth",
                                &e,
                                "#d08770",
                            );
                            let _ = ev.publish();
                        }
                    }
                    DevMsg::Select(id) => {
                        // Console + multimedia only, NOT communications. Picking
                        // headphones to listen on must not also hand them the
                        // voice path: for Bluetooth that forces hands-free mode,
                        // which is mono 16kHz and wrecks playback too.
                        rice_common::audio::set_default_output_roles(
                            &id,
                            &[
                                rice_common::audio::ROLE_CONSOLE,
                                rice_common::audio::ROLE_MULTIMEDIA,
                            ],
                        );
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
                                rice_common::audio::set_default_output_roles(
                                    &id,
                                    &[
                                        rice_common::audio::ROLE_CONSOLE,
                                        rice_common::audio::ROLE_MULTIMEDIA,
                                    ],
                                );
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
        Self { tx, state, pairable }
    }
    fn refresh(&self) {
        let _ = self.tx.send(DevMsg::Refresh);
    }
    fn scan(&self) {
        let _ = self.tx.send(DevMsg::Scan);
    }
    fn pair(&self, id: String) {
        let _ = self.tx.send(DevMsg::Pair(id));
    }
    /// (scanning, nearby unpaired devices)
    fn pairable(&self) -> (bool, Vec<rice_common::bluetooth::Pairable>) {
        self.pairable.lock().unwrap().clone()
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
/// La bateria que corresponde a un endpoint, por nombre. El HyperX aparece
/// como "Altavoces (HyperX Cloud II Wireless)" y los AirPods con el nombre
/// larguisimo que les puso el telefono, asi que se busca por trozo.
fn bateria_para(
    nombre: &str,
    baterias: &[rice_common::battery::Bateria],
) -> Option<rice_common::battery::Bateria> {
    let n = nombre.to_lowercase();
    baterias
        .iter()
        .find(|b| match b.clase {
            rice_common::battery::Clase::HyperX => n.contains("hyperx") || n.contains("cloud"),
            rice_common::battery::Clase::AirPods => n.contains("airpods"),
        })
        .cloned()
}

fn build_device_rows() -> Vec<DevRow> {
    let cfg = rice_common::settings::Settings::live();
    let baterias = rice_common::battery::todas();
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
        // Alias corto cuando sabemos quien es: "hyperx cloud" en vez de
        // "Altavoces (HyperX Cloud II Wireless)".
        let bateria = bateria_para(&e.name, &baterias);
        rows.push(DevRow {
            label: bateria
                .as_ref()
                .map(|b| b.alias().to_string())
                .unwrap_or_else(|| short_output_name(&e.name, &want)),
            endpoint: Some(e.id.clone()),
            bt_container: None,
            connected: e.active,
            is_default: e.id == current,
            busy: false,
            bateria,
        });
    }

    for d in bt {
        let is_default = d.output_id.as_deref() == Some(current.as_str());
        let bateria = bateria_para(&d.name, &baterias);
        rows.push(DevRow {
            label: bateria
                .as_ref()
                .map(|b| b.alias().to_string())
                .unwrap_or_else(|| d.name.clone()),
            endpoint: d.output_id.clone(),
            bt_container: Some(d.container),
            connected: d.connected,
            is_default,
            busy: false,
            bateria,
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

const ACTIONS: [(&str, &str, [u8; 3]); 9] = [
    ("mic", "\u{f130}", [224, 163, 92]),      // switch mic
    ("save", "\u{f03d}", [169, 181, 106]),    // save a replay clip
    ("term", "\u{f120}", [206, 150, 112]),    // open a terminal
    ("opacity", "\u{f1de}", [200, 172, 150]), // fa-sliders -> opacity widget
    ("bright", "\u{f185}", [230, 190, 120]),  // fa-sun -> monitor brightness (DDC/CI)
    ("vol", "\u{f028}", [150, 190, 200]),     // fa-volume-up -> master + per-app volume
    ("timer", "\u{f017}", [205, 150, 170]),   // fa-clock-o -> pomodoro
    ("devices", "\u{f025}", [150, 200, 190]), // fa-headphones -> outputs + bluetooth
    ("notifs", "\u{f0f3}", [190, 170, 210]),  // fa-bell -> centro de notificaciones
];

/// Cuantas notificaciones se ven de entrada; "ver mas" suma otra tanda.
const NOTIFS_PAGE: usize = 4;

/// Margen a los lados de la barra: cuanto se separan del borde de la pantalla
/// los workspaces (izquierda) y los stats con la bandeja (derecha).
///
/// Era 10 y estaba pegado al canto. Contra el borde cuesta leer el icono y
/// cuesta acertarle, porque el raton se para ahi y no se ve bien donde cae.
///
/// TRES SITIOS LO USAN y tienen que cuadrar, que es justo por lo que ahora es
/// una constante: el margen del marco, el alto util (que resta el vertical dos
/// veces) y el rectangulo del fondo (que lo suma dos veces para volver a tocar
/// los bordes). Cambiar uno solo dejaba una franja sin pintar en un lado.
const BAR_PAD_H: f32 = 22.0;
/// Margen arriba y abajo. Separado del horizontal porque el alto de la barra es
/// fijo y este si es un compromiso con el tamano de la fuente.
const BAR_PAD_V: f32 = 5.0;

/// Pasado esto sin latido, el atajo global se da por caido. El script escribe
/// cada 5 s, asi que veinte son cuatro fallos seguidos y no una vuelta lenta.
const AHK_STALE_SECS: u64 = 20;

/// Estado del atajo global (AutoHotkey), leido de `~/.config/ahk-alive.json`.
///
/// EXISTE PORQUE Alt+F10 fallo dos veces sin dejar rastro. Desde fuera, "AHK
/// caido", "la tecla no llega al script" y "el guardado fallo" se ven igual: no
/// pasa nada. Sin una senal de vida no hay forma de saber cual de las tres es, y
/// eso convierte cada fallo en una investigacion desde cero.
#[derive(Clone, Copy, PartialEq)]
enum Atajo {
    Vivo,
    /// Corriendo, pero con los atajos suspendidos: el proceso responde y no
    /// haria nada, que es el caso mas engañoso de los tres.
    Suspendido,
    Caido,
}

impl Atajo {
    fn color(self) -> egui::Color32 {
        match self {
            Atajo::Vivo => egui::Color32::from_rgb(169, 181, 106),
            Atajo::Suspendido => egui::Color32::from_rgb(224, 163, 92),
            Atajo::Caido => egui::Color32::from_rgb(208, 113, 112),
        }
    }
}

/// Lee el latido. Devuelve el estado y cuando fue el ultimo Alt+F10 (epoch, s).
fn leer_atajo() -> (Atajo, u64) {
    #[derive(serde::Deserialize, Default)]
    struct F {
        #[serde(default)]
        suspended: bool,
        #[serde(default)]
        last_f10: u64,
    }
    let p = rice_common::config::config_path("ahk-alive.json");
    let Ok(md) = std::fs::metadata(&p) else { return (Atajo::Caido, 0) };
    let viejo = md
        .modified()
        .ok()
        .and_then(|t| t.elapsed().ok())
        .map(|d| d.as_secs() > AHK_STALE_SECS)
        .unwrap_or(true);
    let f: F = std::fs::read_to_string(&p)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .map(|v| F {
            suspended: v.get("suspended").and_then(|x| x.as_bool()).unwrap_or(false),
            last_f10: v.get("lastF10").and_then(|x| x.as_u64()).unwrap_or(0),
        })
        .unwrap_or_default();
    let estado = if viejo {
        Atajo::Caido
    } else if f.suspended {
        Atajo::Suspendido
    } else {
        Atajo::Vivo
    };
    (estado, f.last_f10)
}

/// "hace 3 min", "hace 2 h", "ayer". Sin dependencia de fechas: la resta de dos
/// marcas en milisegundos da todo lo que hace falta.
fn hace_cuanto(ahora_ms: u64, then_ms: u64) -> String {
    let s = ahora_ms.saturating_sub(then_ms) / 1000;
    if s < 60 {
        "ahora".into()
    } else if s < 3600 {
        format!("{} min", s / 60)
    } else if s < 86_400 {
        format!("{} h", s / 3600)
    } else {
        format!("{} d", s / 86_400)
    }
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
        // El icono de guardar es TAMBIEN el indicador de estado de Alt+F10, y
        // ahora es su interruptor: pulsarlo suspende los atajos globales, y
        // pulsarlo otra vez los devuelve.
        //
        // Se manda por ARCHIVO (~/.config/ahk-suspend.flag) y no simulando la
        // pulsacion: inyectar teclas desde la barra desincroniza AltSnap, que
        // entonces se come la barra espaciadora. Ese camino esta cerrado en este
        // rice a proposito.
        //
        // No se pinta el estado aqui: se deja que AutoHotkey lo confirme en su
        // latido. Asi el color dice lo que PASA, no lo que pedimos -- si AHK
        // esta caido, el icono no se pone verde por mucho que lo pulses.
        "save" => {
            // El estado se lee del latido de AHK, no de lo que crea la interfaz:
            // esto corre en un hilo suelto sin acceso a `self`, y ademas el
            // latido es la unica fuente que sabe si los atajos estan de verdad
            // suspendidos.
            let (estado, _) = leer_atajo();
            let f = format!("{home}\\.config\\ahk-suspend.flag");
            let _ = std::fs::write(&f, if estado == Atajo::Suspendido { "0" } else { "1" });
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

        {
            let mut s = shared.lock().unwrap();
            s.cpu = cpu;
            s.mem = mem;
        }
        ctx.request_repaint();
        std::thread::sleep(Duration::from_millis(1500));
    }
}

/// Bateria del dispositivo que esta sonando.
///
/// Aqui SI hay un sondeo, y a proposito: el HyperX no avisa de nada, hay que
/// preguntarle por HID. Pero va lento -- la bateria de unos auriculares no
/// cambia en segundos. Lo que se mira seguido es cual es la salida
/// predeterminada, que es barato, para que al cambiar de auriculares el numero
/// salte enseguida en vez de esperar al siguiente minuto.
///
/// Los AirPods no se preguntan: su oyente BLE se arranca una vez aqui y va
/// dejando lo ultimo que oyo.
fn battery_thread(shared: Arc<Mutex<Shared>>, ctx: egui::Context) {
    rice_common::battery::iniciar_escucha_airpods();
    let mut ultima_salida = String::new();
    let mut ultima_lectura: Option<Instant> = None;
    // Cada cuanto se relee TODO. Un minuto cuando el casco contesta; diez
    // segundos mientras no lo haga, que es cuando importa la prisa: acabas de
    // apagarlo y el numero sigue en pantalla. Asi desaparece en ~20-30 s en vez
    // de en tres minutos, sin sondear mas rapido el resto del tiempo.
    let mut intervalo = Duration::from_secs(60);
    // El estado de carga aparte, y mas seguido: es un solo dialogo y es lo
    // unico que cambia de golpe. Enchufas el cable y el rayo sale en ~15 s en
    // vez de esperar al siguiente minuto.
    let mut ultima_carga: Option<Instant> = None;
    loop {
        let salida = rice_common::audio::current_output_name().unwrap_or_default();
        let toca = salida != ultima_salida
            || ultima_lectura.map(|t| t.elapsed() >= intervalo).unwrap_or(true);
        if toca {
            ultima_salida = salida;
            ultima_lectura = Some(Instant::now());
            // Este hilo es el unico que habla con el dongle; todo lo demas
            // (incluida la lista de dispositivos) lee la cache que deja aqui.
            let contesto = rice_common::battery::refrescar();
            intervalo = if contesto {
                Duration::from_secs(60)
            } else {
                Duration::from_secs(10)
            };
            let nuevas = rice_common::battery::conectadas();
            let mut s = shared.lock().unwrap();
            // Repintar solo si cambio algo visible: sin esto la barra se
            // despierta cada minuto para dibujar los mismos numeros.
            let resumen = |v: &Vec<rice_common::battery::Bateria>| -> Vec<_> {
                v.iter()
                    .map(|b| (b.clase, b.nivel, b.cargando, b.partes.clone()))
                    .collect()
            };
            if resumen(&s.baterias) != resumen(&nuevas) {
                s.baterias = nuevas;
                drop(s);
                ctx.request_repaint();
            }
            ultima_carga = Some(Instant::now());
        }
        // Chequeo ligero del rayo entre lecturas completas. Se salta si acaba
        // de hacerse una lectura entera, que ya trae el estado de carga fresco.
        else if ultima_carga
            .map(|t| t.elapsed() >= Duration::from_secs(15))
            .unwrap_or(true)
        {
            ultima_carga = Some(Instant::now());
            let antes: Vec<bool> = shared
                .lock()
                .unwrap()
                .baterias
                .iter()
                .map(|b| b.cargando)
                .collect();
            if rice_common::battery::refrescar_carga() {
                let nuevas = rice_common::battery::conectadas();
                let ahora: Vec<bool> = nuevas.iter().map(|b| b.cargando).collect();
                if antes != ahora {
                    shared.lock().unwrap().baterias = nuevas;
                    ctx.request_repaint();
                }
            }
        }
        std::thread::sleep(Duration::from_secs(5));
    }
}

/// Lo que el crate `consumo` escribe cada minuto en
/// ~/.config/consumo/ahora.json.
///
/// La barra LEE un archivo en vez de medir por su cuenta, y eso es
/// deliberado: quien integra en el tiempo tiene que ser un proceso solo. Con
/// dos barras midiendo cada una por su lado, el gasto del dia se contaria dos
/// veces.
#[derive(serde::Deserialize, Clone, Default)]
struct ConsumoAhora {
    w: f64,
    #[serde(default)]
    gpu_w: f64,
    #[serde(default)]
    cpu_w: f64,
    #[serde(default)]
    base_w: f64,
    #[serde(default)]
    monitores_w: f64,
    #[serde(default)]
    horas_encendido: f64,
    #[serde(default)]
    horas_medidas: f64,
    kwh_hoy: f64,
    coste_hoy: f64,
    moneda: String,
    cpu_medida: bool,
}

/// Relee ese archivo. Cada 30 s: el medidor lo escribe una vez por minuto, asi
/// que mirarlo mas seguido solo gasta lecturas para ver lo mismo.
fn consumo_thread(shared: Arc<Mutex<Shared>>, ctx: egui::Context) {
    let ruta = std::path::Path::new(&std::env::var("USERPROFILE").unwrap_or_default())
        .join(".config")
        .join("consumo")
        .join("ahora.json");
    loop {
        let leido = std::fs::read_to_string(&ruta)
            .ok()
            .and_then(|t| serde_json::from_str::<ConsumoAhora>(&t).ok());
        {
            let mut s = shared.lock().unwrap();
            let cambio = match (&s.consumo, &leido) {
                (Some(a), Some(b)) => {
                    (a.w - b.w).abs() > 0.5 || (a.coste_hoy - b.coste_hoy).abs() > 0.005
                }
                (None, None) => false,
                _ => true,
            };
            if cambio {
                s.consumo = leido;
                drop(s);
                ctx.request_repaint();
            }
        }
        std::thread::sleep(Duration::from_secs(30));
    }
}

/// Cuanto hace de una lectura, en palabras. Los AirPods solo se anuncian de
/// vez en cuando, asi que decir "hace 20 min" es parte de la respuesta.
fn hace(d: Duration) -> String {
    let s = d.as_secs();
    if s < 90 {
        format!("{s} s")
    } else if s < 5400 {
        format!("{} min", s / 60)
    } else {
        format!("{} h", s / 3600)
    }
}

/// Globo de la bateria: el desglose, la salud y, si la lectura es vieja, cuanto.
fn descripcion_bateria(b: &rice_common::battery::Bateria) -> String {
    let mut t = b.alias().to_string();
    match b.nivel {
        Some(n) => t.push_str(&format!(" — {n}%")),
        None => t.push_str(" — sin lectura"),
    }
    if b.cargando {
        t.push_str(" (cargando)");
    }
    for (nombre, v) in &b.partes {
        t.push_str(&format!("\n{nombre}: {v}%"));
    }
    // Tension real de la celda, no un porcentaje redondeado. El HyperX la da;
    // los AirPods no.
    if let Some(mv) = b.voltaje_mv {
        t.push_str(&format!("\ntension: {:.2} V", mv as f32 / 1000.0));
    }
    // Velocidad de carga o descarga. Es DERIVADA de como se mueve el
    // porcentaje: el casco no publica corriente, asi que no hay vatios que dar.
    match b.ritmo_pct_h {
        Some(r) if r.abs() >= 0.5 => {
            t.push_str(&format!("\nritmo: {r:+.0} puntos/h"));
            if let Some(n) = b.nivel {
                let restante = if r > 0.0 { 100.0 - n as f32 } else { n as f32 };
                let horas = restante / r.abs();
                let etiqueta = if r > 0.0 { "lleno en" } else { "vacio en" };
                if horas < 1.0 {
                    t.push_str(&format!("\n{etiqueta}: ~{:.0} min", horas * 60.0));
                } else {
                    t.push_str(&format!("\n{etiqueta}: ~{horas:.1} h"));
                }
            }
        }
        _ => t.push_str("\nritmo: aun midiendo (hacen falta ~15 min)"),
    }
    t.push_str(&format!("\n{}", rice_common::battery::POTENCIA_POR_QUE));
    match b.salud {
        Some(h) => t.push_str(&format!("\nsalud: {h}%")),
        None => t.push_str(&format!(
            "\nsalud: sin dato — {}",
            rice_common::battery::SALUD_POR_QUE
        )),
    }
    if b.edad.as_secs() > 90 {
        t.push_str(&format!("\nleido hace {}", hace(b.edad)));
    }
    t
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
        // La VRAM sale en la MISMA consulta: no cuesta ni un spawn mas, y en
        // esta maquina es el numero que de verdad importa. Con 12 GB, un LLM
        // local y un TTS cargados a la vez, quedarse por debajo de ~500 MiB
        // libres hace que CUDA se desborde a memoria compartida por PCIe y todo
        // se hunde. Sin el dato a la vista, eso se descubre cuando ya va lento.
        let out = std::process::Command::new("nvidia-smi")
            .args([
                "--query-gpu=temperature.gpu,utilization.gpu,memory.used,memory.total",
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
        // Si nvidia-smi devolviera menos campos de los pedidos, se cae al
        // formato de antes en vez de no pintar GPU.
        // "VRAM" escrito, y no solo los numeros pegados al porcentaje.
        //
        // Antes salia "GPU 53° 32%  2.2/12G" y se leia como una contradiccion:
        // el 32% es USO DEL NUCLEO y el 2.2/12 es MEMORIA (que seria un 18%).
        // Dos cifras contiguas que parecen la misma magnitud y no lo son.
        let g = match (parts.next(), parts.next()) {
            (Some(usada), Some(total)) => {
                let (u, t) = (usada.parse::<f32>().ok()?, total.parse::<f32>().ok()?);
                format!("{temp}° {util}%  VRAM {:.1}/{:.0}G", u / 1024.0, t / 1024.0)
            }
            _ => format!("{temp}° {util}%"),
        };
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
    /// Borde izquierdo del bloque de stats del fotograma anterior, o None si no
    /// se dibujaron (pantalla completa). Un fotograma de retraso no se ve.
    stats_left: Option<f32>,
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
    /// The notification-area icons, published by `taskbar`. Reading them costs
    /// nothing here: that process does the Windows side, this one only draws.
    tray: tray::Tray,
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
    isl_notifs: bool,
    /// Historial leido del disco. Se relee al abrir el panel y cuando cambia el
    /// archivo, no en cada fotograma.
    notifs: Vec<rice_common::event::NotifRecord>,
    notifs_stamp: Option<std::time::SystemTime>,
    /// Cuando se miro por ultima vez la fecha del archivo.
    notifs_checked: Instant,
    /// Estado del atajo global y cuando se miro por ultima vez.
    atajo: Atajo,
    atajo_f10: u64,
    atajo_checked: Instant,
    /// Se avisa UNA vez por transicion, no en cada sondeo.
    atajo_avisado: bool,
    /// Cuantas se muestran ahora mismo; "ver mas" lo sube.
    notifs_shown: usize,
    want_close: bool,   // set under the shared lock, acted on after it is dropped
    want_back: bool,    // return to the action grid without closing
    timer_left: Duration,
    timer_total: Duration,
    timer_running: bool,
    timer_tick: Instant,
    win_h: i32,              // last window height requested
    panel_rect: Option<egui::Rect>, // where the bubble is, for outside-click hit testing
    last_opacity_write: Instant,
    /// Ultimo latido escrito para el supervisor. Ver `update`.
    last_beat: Instant,
}



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
        } else if self.isl_notifs {
            7
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
            7 => 340.0,
            _ => 3.0 * 52.0 + 24.0,
        }
    }

    /// Resting height of the bubble for the current sub-view.
    fn panel_target_h(&self) -> f32 {
        match self.panel_mode() {
            // Volumen necesita 16 px mas que brillo y opacidad: lleva una fila
            // extra de botones de silencio bajo las etiquetas. Con los 140 de
            // antes el icono quedaba a 7 px del borde.
            1 => 156.0,
            2 | 3 => 140.0,
            4 => 132.0,
            5 => 138.0,
            6 => {
                // One row per device, plus the header, the scan control, and any
                // nearby devices found.
                let (scanning, near) = self.dev_ctl.pairable();
                let n = self.dev_ctl.rows().len().max(1) as f32;
                let extra = if scanning || !near.is_empty() { near.len() as f32 * 34.0 + 30.0 } else { 0.0 };
                (n * 38.0 + 34.0 + 30.0 + extra).min(260.0)
            }
            7 => {
                let n = self.notifs.len().min(self.notifs_shown).max(1) as f32;
                // Linea del atajo + cabecera + filas + "ver mas / limpiar".
                (20.0 + 30.0 + n * 46.0 + 34.0).min(340.0)
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
        self.isl_notifs = false;
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

        // Sub-views get a back chevron; the action grid is the root and has none.
        if self.panel_mode() != 0 {
            let c = egui::pos2(rect.left() + 18.0, rect.top() + 17.0);
            let r = ui
                .interact(
                    egui::Rect::from_center_size(c, egui::vec2(30.0, 28.0)),
                    egui::Id::new("panel-back"),
                    egui::Sense::click(),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if r.hovered() {
                self.isl_interact = now;
            }
            draw_icon(
                &p,
                c,
                "\u{f053}", // fa-chevron-left
                12.0,
                if r.hovered() { WARM_ACCENT } else { WARM_SUB },
            );
            if r.clicked() {
                self.want_back = true;
                self.isl_interact = now;
            }
        }

        match self.panel_mode() {
            1 => self.panel_volume(ui, &p, rect, now),
            2 => self.panel_bright(ui, &p, rect, now),
            3 => self.panel_opacity(ui, &p, rect, now),
            4 => self.panel_media(ui, &p, rect, now),
            5 => self.panel_timer(ui, &p, rect, now),
            6 => self.panel_devices(ui, &p, rect, now),
            7 => self.panel_notifs(ui, &p, rect, now),
            _ => self.panel_actions(ui, &p, rect, now, ctx),
        }

        let timeout = rice_common::settings::Settings::live().animation.panel_timeout_secs;
        if now.duration_since(self.isl_interact).as_secs_f32() > timeout {
            self.close_panel();
        }
    }

    /// Sondea el latido del atajo global, como mucho una vez por segundo.
    ///
    /// Solo la barra primaria avisa por la isla: con dos monitores, avisar desde
    /// las dos publicaria el mismo evento dos veces.
    fn refresh_atajo(&mut self) {
        if self.atajo_checked.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.atajo_checked = Instant::now();
        let (estado, f10) = leer_atajo();
        self.atajo_f10 = f10;
        if estado != self.atajo {
            // Una vez por transicion, no en cada sondeo: si no, un AHK caido
            // soltaria un aviso por segundo para siempre.
            if estado != Atajo::Vivo && !self.atajo_avisado && self.x == 0 {
                let cuerpo = if estado == Atajo::Suspendido {
                    "atajos suspendidos: Win+Shift+Z los devuelve"
                } else {
                    "AutoHotkey no responde; Alt+F10 no hara nada"
                };
                let _ = rice_common::event::IslandEvent::new(
                    "warn",
                    "Atajos globales caidos",
                    cuerpo,
                    "#d08770",
                )
                .publish();
                self.atajo_avisado = true;
            }
            if estado == Atajo::Vivo {
                self.atajo_avisado = false;
            }
            self.atajo = estado;
        }
    }

    fn panel_actions(&mut self, ui: &mut egui::Ui, p: &egui::Painter, rect: egui::Rect, now: Instant, ctx: &egui::Context) {
        self.refresh_atajo();
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
            // El boton de guardar lleva el estado del atajo global encima. Es el
            // indicador: si Alt+F10 no va a hacer nada, se ve aqui antes de
            // pulsarlo y no despues de no ver pasar nada.
            let es_guardar = *kind == "save";
            let acc = if es_guardar {
                self.atajo.color()
            } else {
                egui::Color32::from_rgb(col[0], col[1], col[2])
            };
            let a = if r.hovered() { 90 } else { 42 };
            p.circle_filled(c, 17.0, egui::Color32::from_rgba_unmultiplied(acc.r(), acc.g(), acc.b(), a));
            draw_icon(p, c, glyph, 17.0, acc);
            if es_guardar {
                let estado = match self.atajo {
                    Atajo::Vivo => "Alt+F10 activo - pulsa para desactivarlo",
                    Atajo::Suspendido => "Alt+F10 desactivado - pulsa para activarlo",
                    Atajo::Caido => "Alt+F10 CAIDO: AutoHotkey no responde",
                };
                let ultimo = if self.atajo_f10 == 0 {
                    "sin usar desde que arranco".to_string()
                } else {
                    let ahora = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    format!("ultimo uso: hace {}", hace_cuanto(ahora * 1000, self.atajo_f10 * 1000))
                };
                r.clone().on_hover_text(format!("{estado}\n{ultimo}"));
            }
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
                    "notifs" => {
                        self.reload_notifs(true);
                        self.notifs_shown = NOTIFS_PAGE;
                        self.isl_notifs = true;
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
        let mut toggled: Option<(String, bool)> = None;
        for (i, fila) in self.vol.iter_mut().enumerate() {
            let cx = x0 + i as f32 * step;
            // Silenciado = barra apagada. Es la senal que se lee de un vistazo,
            // antes de mirar iconos: una app muda no debe parecer que suena al
            // 72% solo porque su nivel siga ahi.
            let color = if fila.muted { WARM_SUB } else { WARM_ACCENT };
            if let Some(v) = Self::vslider(ui, p, ("pvol", i), cx, top, h, fila.level, color) {
                fila.level = v;
                changed = Some((if i == 0 { String::new() } else { fila.name.clone() }, v));
            }
            p.text(
                egui::pos2(cx, top + h + 12.0),
                egui::Align2::CENTER_CENTER,
                fila.name.chars().take(7).collect::<String>(),
                egui::FontId::proportional(8.5),
                if fila.muted { WARM_SUB } else { WARM_SUB },
            );

            // Boton de silencio bajo la etiqueta.
            let bc = egui::pos2(cx, top + h + 28.0);
            let br = ui
                .interact(
                    egui::Rect::from_center_size(bc, egui::vec2(22.0, 22.0)),
                    egui::Id::new(("pvol-mute", i)),
                    egui::Sense::click(),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if br.hovered() {
                self.isl_interact = now;
            }
            if fila.muted || br.hovered() {
                let a = if br.hovered() { 90 } else { 55 };
                let base = if fila.muted { egui::Color32::from_rgb(200, 110, 100) } else { WARM_ACCENT };
                p.circle_filled(
                    bc,
                    10.0,
                    egui::Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), a),
                );
            }
            // f026 = volumen apagado, f028 = volumen alto. Los dos estan en el
            // rango FA4 que esta barra ya usa para el resto de iconos.
            let (glifo, tinte) = if fila.muted {
                ("\u{f026}", egui::Color32::from_rgb(230, 140, 130))
            } else {
                ("\u{f028}", WARM_SUB)
            };
            draw_icon(p, bc, glifo, 10.5, tinte);
            if br.clicked() {
                let objetivo = !fila.muted;
                fila.muted = objetivo;   // pintado optimista; el worker relee y corrige
                toggled = Some((if i == 0 { String::new() } else { fila.name.clone() }, objetivo));
            }
        }
        if let Some((name, v)) = changed {
            self.isl_interact = now;
            self.vol_ctl.set(&name, v);
        }
        if let Some((name, m)) = toggled {
            self.isl_interact = now;
            self.vol_ctl.mute(&name, m);
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


    /// Relee el historial si el archivo cambio (o si se fuerza al abrir).
    ///
    /// Con freno de un segundo: sin el, `fs::metadata` corria una vez por
    /// FOTOGRAMA mientras el panel estuviera abierto, o sea ~60 llamadas al
    /// sistema por segundo para mirar un archivo que cambia cada varios minutos.
    /// Es el mismo freno que ya lleva `Tray::poll` por la misma razon.
    fn reload_notifs(&mut self, force: bool) {
        if !force && self.notifs_checked.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.notifs_checked = Instant::now();
        let path = rice_common::config::config_path(rice_common::event::HISTORY_FILE);
        let stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if force || stamp != self.notifs_stamp {
            self.notifs_stamp = stamp;
            self.notifs = rice_common::event::history();
        }
    }

    /// Centro de notificaciones: las últimas, con descartar y ver más.
    ///
    /// EXISTE PORQUE una notificación del sistema aparecía, se iba sola, y no
    /// quedaba sitio donde mirar qué era. El historial lo escriben `notifyd` (y
    /// por tanto TODAS las de Windows) y `Set-RiceIsland` (las del propio rice),
    /// asi que aquí está todo, independientemente de si se mostró como isla o
    /// como toast.
    fn panel_notifs(&mut self, ui: &mut egui::Ui, p: &egui::Painter, rect: egui::Rect, now: Instant) {
        self.reload_notifs(false);
        self.refresh_atajo();

        // Estado del atajo global, arriba del todo y siempre: es lo primero que
        // uno quiere saber cuando "no paso nada". Un punto y una linea.
        let punto = egui::pos2(rect.left() + 16.0, rect.top() + 16.0);
        p.circle_filled(punto, 4.0, self.atajo.color());
        let texto = match self.atajo {
            Atajo::Vivo => "Alt+F10 activo".to_string(),
            Atajo::Suspendido => "Alt+F10 suspendido (Win+Shift+Z)".to_string(),
            Atajo::Caido => "Alt+F10 CAIDO: AutoHotkey no responde".to_string(),
        };
        p.text(
            egui::pos2(punto.x + 10.0, punto.y),
            egui::Align2::LEFT_CENTER,
            texto,
            egui::FontId::proportional(10.0),
            if self.atajo == Atajo::Vivo { WARM_SUB } else { self.atajo.color() },
        );
        // Deja sitio a esa linea: la cabecera vieja empezaba donde ahora va ella.
        let rect = egui::Rect::from_min_max(
            egui::pos2(rect.left(), rect.top() + 20.0),
            rect.max,
        );

        if self.notifs.is_empty() {
            p.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "sin notificaciones",
                egui::FontId::proportional(11.5),
                WARM_SUB,
            );
            return;
        }

        let ahora_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        // A la DERECHA: el borde izquierdo lo ocupa la flecha de volver que
        // pinta el marco del panel, y el contador quedaba debajo de ella.
        p.text(
            egui::pos2(rect.right() - 16.0, rect.top() + 16.0),
            egui::Align2::RIGHT_CENTER,
            format!("{}", self.notifs.len()),
            egui::FontId::proportional(10.5),
            WARM_SUB,
        );

        let mut descartar: Option<u64> = None;
        let visibles = self.notifs.len().min(self.notifs_shown);
        for i in 0..visibles {
            let nrec = self.notifs[i].clone();
            let y = rect.top() + 34.0 + i as f32 * 46.0;
            let fila = egui::Rect::from_min_size(
                egui::pos2(rect.left() + 10.0, y),
                egui::vec2(rect.width() - 20.0, 42.0),
            );
            let r = ui.interact(fila, egui::Id::new(("ntf", nrec.at)), egui::Sense::hover());
            if r.hovered() {
                self.isl_interact = now;
                p.rect_filled(fila, egui::Rounding::same(7.0), ISL_HI);
            }

            let acc = rice_common::theme::parse_hex(&nrec.accent)
                .map(|c| egui::Color32::from_rgb(c[0], c[1], c[2]))
                .unwrap_or(WARM_ACCENT);
            let ic = egui::pos2(fila.left() + 20.0, fila.center().y);
            p.circle_filled(ic, 13.0, egui::Color32::from_rgba_unmultiplied(acc.r(), acc.g(), acc.b(), 42));
            draw_icon(p, ic, rice_common::ui::icon_glyph(&nrec.icon), 13.0, acc);

            let tx = fila.left() + 40.0;
            p.text(
                egui::pos2(tx, fila.top() + 13.0),
                egui::Align2::LEFT_CENTER,
                nrec.title.chars().take(30).collect::<String>(),
                egui::FontId::proportional(11.5),
                WARM_TEXT,
            );
            p.text(
                egui::pos2(tx, fila.top() + 29.0),
                egui::Align2::LEFT_CENTER,
                nrec.body.chars().take(38).collect::<String>(),
                egui::FontId::proportional(10.0),
                WARM_SUB,
            );
            p.text(
                egui::pos2(fila.right() - 30.0, fila.top() + 13.0),
                egui::Align2::RIGHT_CENTER,
                hace_cuanto(ahora_ms, nrec.at),
                egui::FontId::proportional(9.5),
                WARM_SUB,
            );

            // Descartar: sólo aparece al pasar por encima, para que la lista no
            // sea una fila de aspas.
            if r.hovered() {
                let c = egui::pos2(fila.right() - 16.0, fila.center().y);
                let x = ui
                    .interact(
                        egui::Rect::from_center_size(c, egui::vec2(22.0, 22.0)),
                        egui::Id::new(("ntf-x", nrec.at)),
                        egui::Sense::click(),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                draw_icon(p, c, "\u{f00d}", 11.0, if x.hovered() { WARM_ACCENT } else { WARM_SUB });
                if x.clicked() {
                    descartar = Some(nrec.at);
                }
            }
        }

        // Pie: "ver más" mientras queden, y "limpiar" siempre.
        let by = rect.top() + 34.0 + visibles as f32 * 46.0 + 12.0;
        if visibles < self.notifs.len() {
            let c = egui::pos2(rect.left() + 60.0, by);
            let r = ui
                .interact(
                    egui::Rect::from_center_size(c, egui::vec2(100.0, 22.0)),
                    egui::Id::new("ntf-mas"),
                    egui::Sense::click(),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            p.text(
                c,
                egui::Align2::CENTER_CENTER,
                format!("ver {} más", (self.notifs.len() - visibles).min(NOTIFS_PAGE)),
                egui::FontId::proportional(10.5),
                if r.hovered() { WARM_ACCENT } else { WARM_SUB },
            );
            if r.clicked() {
                self.notifs_shown += NOTIFS_PAGE;
                self.isl_interact = now;
            }
        }
        let c = egui::pos2(rect.right() - 50.0, by);
        let r = ui
            .interact(
                egui::Rect::from_center_size(c, egui::vec2(80.0, 22.0)),
                egui::Id::new("ntf-limpiar"),
                egui::Sense::click(),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        p.text(
            c,
            egui::Align2::CENTER_CENTER,
            "limpiar",
            egui::FontId::proportional(10.5),
            if r.hovered() { WARM_ACCENT } else { WARM_SUB },
        );
        if r.clicked() {
            let _ = rice_common::event::history_clear();
            self.reload_notifs(true);
            self.notifs_shown = NOTIFS_PAGE;
            self.isl_interact = now;
        }

        if let Some(at) = descartar {
            let _ = rice_common::event::history_dismiss(at);
            self.reload_notifs(true);
            self.isl_interact = now;
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
        p.circle_stroke(ring_c, 30.0, egui::Stroke::new(4.0_f32, ISL_HI));
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
                egui::Stroke::new(4.0_f32, WARM_ACCENT),
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
                p.circle_stroke(dot, 4.5, egui::Stroke::new(1.5_f32, WARM_SUB));
            }

            let fg = if r.connected { WARM_TEXT } else { WARM_SUB };
            p.text(
                egui::pos2(row_rect.left() + 28.0, row_rect.center().y),
                egui::Align2::LEFT_CENTER,
                &r.label,
                egui::FontId::proportional(12.5),
                fg,
            );

            // Bateria y salud a la derecha del nombre: es lo que hace util la
            // fila, saber si te vas a quedar sin auriculares ANTES de elegirlos.
            if let Some(b) = &r.bateria {
                let nivel = match b.nivel {
                    Some(n) => format!("{n}%"),
                    None => "--".to_string(),
                };
                let salud = match b.salud {
                    Some(h) => format!("salud {h}%"),
                    None => "salud ?".to_string(),
                };
                p.text(
                    egui::pos2(row_rect.right() - 52.0, row_rect.center().y),
                    egui::Align2::RIGHT_CENTER,
                    format!("{nivel}   {salud}"),
                    egui::FontId::proportional(11.0),
                    if b.nivel.map(|n| n <= 20).unwrap_or(false) {
                        col(theme::ACCENT_WARN)
                    } else {
                        WARM_SUB
                    },
                );
                resp.clone().on_hover_text(descripcion_bateria(b));
            }

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

        // ---- pairing ------------------------------------------------------
        // Connecting and pairing are different things: everything above is a
        // device Windows already knows, and an unpaired one has no audio
        // endpoint at all, so it cannot appear there however close it is.
        let (scanning, near) = self.dev_ctl.pairable();
        let scan_rect = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 12.0, y),
            egui::vec2(rect.width() - 24.0, 26.0),
        );
        let scan_resp = ui
            .interact(scan_rect, egui::Id::new("dev-scan"), egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if scan_resp.hovered() {
            self.isl_interact = now;
        }
        let label = if scanning { "Buscando..." } else { "+ Emparejar" };
        p.text(
            egui::pos2(scan_rect.center().x, scan_rect.center().y),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(11.5),
            if scanning { WARM_SUB } else { WARM_ACCENT },
        );
        if scan_resp.clicked() && !scanning {
            // A scan blocks for ~22s, so keep the panel from timing out under it.
            self.isl_interact = now;
            self.dev_ctl.scan();
        }
        if scanning {
            // The panel's own idle timeout would close it mid-scan otherwise.
            self.isl_interact = now;
        }
        y += 30.0;

        for (i, n) in near.iter().enumerate() {
            let r_rect = egui::Rect::from_min_size(
                egui::pos2(rect.left() + 12.0, y),
                egui::vec2(rect.width() - 24.0, 28.0),
            );
            let resp = ui
                .interact(r_rect, egui::Id::new(("pairable", i)), egui::Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if resp.hovered() {
                self.isl_interact = now;
                p.rect_filled(
                    r_rect,
                    egui::Rounding::same(8.0),
                    egui::Color32::from_rgba_unmultiplied(
                        WARM_ACCENT.r(),
                        WARM_ACCENT.g(),
                        WARM_ACCENT.b(),
                        24,
                    ),
                );
            }
            p.text(
                egui::pos2(r_rect.left() + 16.0, r_rect.center().y),
                egui::Align2::LEFT_CENTER,
                &n.name,
                egui::FontId::proportional(12.0),
                WARM_SUB,
            );
            draw_icon(
                p,
                egui::pos2(r_rect.right() - 14.0, r_rect.center().y),
                "\u{f067}", // fa-plus
                10.0,
                WARM_ACCENT,
            );
            if resp.clicked() {
                self.isl_interact = now;
                self.dev_ctl.pair(n.id.clone());
            }
            y += 34.0;
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
        // Latido para el supervisor, escrito DESDE EL BUCLE DE DIBUJO y no desde
        // un hilo aparte, porque lo que hay que poder detectar es exactamente
        // que el dibujo se pare.
        //
        // Paso de verdad: el reloj de la barra se quedo congelado en 03:11
        // mientras el proceso seguia vivo y con Responding=True, o sea invisible
        // para toda comprobacion por proceso o por mutex. Un hilo aparte habria
        // seguido latiendo tan tranquilo. Reiniciar la barra lo arreglo.
        //
        // Cada 5 s: sys_thread pide un repintado cada 1,5 s pase lo que pase, asi
        // que un latido de mas de 30 s de antiguedad significa parada de verdad y
        // no escritorio quieto.
        if self.last_beat.elapsed() >= Duration::from_secs(5) {
            self.last_beat = Instant::now();
            let p = rice_common::config::config_path(&format!("bar-alive-{}.txt", self.x));
            let _ = std::fs::write(p, "1");
        }
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
                let antes = self.hwnd;
                self.hwnd = find_own_window(self.x, self.width as i32);
                if self.hwnd != 0 {
                    // Cheap no-op once done, but re-asserted because eframe can
                    // recreate the window under us.
                    unsafe { strip_native_frame(self.hwnd) };

                    // NO ROBAR EL FOCO. Es la causa raiz del sintoma "hago clic
                    // arriba y la barra se queda comiendo clics".
                    //
                    // Sin WS_EX_NOACTIVATE, un clic en la barra la pone en
                    // primer plano. Y should_clickthrough devuelve false cuando
                    // la ventana en primer plano es ella misma, asi que el
                    // click-through se apagaba y NO se volvia a encender hasta
                    // que otra ventana recuperaba el foco. Medido en su propio
                    // log: una vez se quedo asi 2 min 19 s.
                    //
                    // NOACTIVATE deja llegar los clics a egui igual -- los
                    // workspaces se siguen pulsando -- pero sin activar la
                    // ventana. El cierre del panel no depende del foco: ya
                    // sondea el cursor globalmente.
                    unsafe { rice_common::win::harden_overlay(self.hwnd) };

                    // El subclaseo va con el HWND, asi que se rehace si eframe
                    // recrea la ventana. SetWindowSubclass con el mismo id es
                    // idempotente, pero se limita al cambio para no llamarlo
                    // dos veces por segundo para nada.
                    if self.hwnd != antes {
                        unsafe {
                            rclick::SetWindowSubclass(self.hwnd, rclick::proc_, rclick::ID, 0);
                        }
                        // El vigilante trabaja sobre este HWND. Se republica al
                        // cambiar porque eframe puede recrear la ventana.
                        rclick::HWND.store(self.hwnd, std::sync::atomic::Ordering::Relaxed);

                        // Reservar la franja para que nada se maximice debajo.
                        // Va aqui, con el HWND recien resuelto, porque el appbar
                        // se registra CONTRA una ventana concreta: si eframe la
                        // recrea hay que volver a registrarlo.
                        unsafe {
                            let mon = MonitorFromWindow(self.hwnd, 2);
                            let mut mi = MonInfo {
                                cb: std::mem::size_of::<MonInfo>() as u32,
                                rc_monitor: Rect { left: 0, top: 0, right: 0, bottom: 0 },
                                rc_work: Rect { left: 0, top: 0, right: 0, bottom: 0 },
                                flags: 0,
                            };
                            if GetMonitorInfoW(mon, &mut mi) != 0 {
                                appbar::reservar(self.hwnd, mi.rc_monitor, self.bar_h() as i32);
                            }
                        }
                    }
                    // Re-assert TOPMOST too. The style bit survives an explorer
                    // restart but the z-band ordering does not: measured, a bar
                    // with WS_EX_TOPMOST still set sat UNDER a plain Firefox
                    // window after explorer came back, and the panic hotkey
                    // (which only touches AltSnap and the AHK script) could not
                    // help. One idempotent SetWindowPos per tick fixes it for
                    // good.
                    unsafe {
                        const HWND_TOPMOST: isize = -1;
                        const SWP: u32 = 0x0001 | 0x0002 | 0x0010; // NOSIZE NOMOVE NOACTIVATE
                        SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP);
                    }
                    let want = rclick::PASAR.load(std::sync::atomic::Ordering::Relaxed);
                    if want != self.clickthrough {
                        // Solo al cambiar: registrarlo dos veces por segundo
                        // llenaria el archivo y no diria nada. Se anota QUIEN lo
                        // provoco, que es justo lo que faltaba para diagnosticar
                        // por que la barra desaparecia al hacer clic abajo.
                        unsafe {
                            let fg = GetForegroundWindow();
                            let mut r = Rect { left: 0, top: 0, right: 0, bottom: 0 };
                            GetWindowRect(fg, &mut r);
                            elog(&format!(
                                "x={} clickthrough {} -> {}  motivo: {}  foco: clase='{}' proc={:?} rect={},{} {}x{}",
                                self.x,
                                self.clickthrough,
                                want,
                                rclick::motivo_txt(
                                    rclick::MOTIVO.load(std::sync::atomic::Ordering::Relaxed)
                                ),
                                class_of(fg),
                                win::foreground_process_name(),
                                r.left,
                                r.top,
                                r.right - r.left,
                                r.bottom - r.top
                            ));
                        }
                    }
                    self.clickthrough = want;
                    // Assert the style unconditionally rather than only on a
                    // transition. set_clickthrough is a no-op when the bit already
                    // matches, and this way anything that clears the ex-style
                    // behind our back is corrected on the next tick instead of
                    // leaving the bar permanently solid.
                    // ...pero NO mientras hay una notificacion en pantalla. Con
                    // WS_EX_TRANSPARENT puesto la barra no recibe raton, asi que
                    // el clic-para-descartar de la pildora no podia dispararse
                    // nunca sobre un juego: la unica forma de quitarla era
                    // esperar los 4 s. Aqui recupera el raton exactamente
                    // mientras hay algo que descartar, y solo entonces.
                    // Aplicarlo ya no se hace aqui: lo hace el vigilante cada
                    // 150 ms. Este tick solo lo lee para el log y para `hidden`.
                }
                // Live-reload bar opacity from the file (so editing it directly also
                // updates in real time), except while the slider owns the value.
                if !self.isl_opacity {
                    self.bar_opacity = read_opacity("bar-opacity.txt", self.bar_opacity);
                }
            }
        }

        // Only the primary bar shows the tray: reading it is one stat a second,
        // but two copies of the notification area on two monitors is not what
        // anyone means by "the tray".
        if self.x == 0 {
            self.tray.poll(ctx);
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
                // Sobre un juego a pantalla completa, la isla y punto.
                //
                // El toast es un PROCESO aparte con su propia ventana OpenGL, y
                // una ventana nueva encima de un juego en exclusiva fuerza un
                // cambio de modo de video: eso es lo que minimizaba League cada
                // vez que llegaba una notificacion. notifyd ya lo evita
                // (notifyd/src/main.rs), pero este temporizador se lo saltaba y
                // era el unico sitio del rice que seguia abriendo esa ventana.
                if !rice_common::win::fullscreen_app_focused() {
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
        // ANTES de tomar el cerrojo de `shared`: esto necesita `&mut self` y ese
        // cerrojo mantiene prestado `self` durante todo el dibujado.
        //
        // Se sondea aqui y no solo al abrir un panel porque el indicador vive
        // ahora en la fila de stats, siempre a la vista, y tiene que estar al dia
        // sin que nadie abra nada. Dentro se limita a una lectura por segundo.
        self.refresh_atajo();
        let atajo = self.atajo;
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
        // A fullscreen application owns the screen. `self.clickthrough` already
        // says one is covering this monitor -- it is what disarms hit-testing --
        // so the same answer decides whether anything is drawn at all.
        //
        // The window stays alive and keeps rendering: it just renders nothing.
        // Tearing it down would lose the GL context and the notification would
        // have nowhere to appear.
        let hidden = self.clickthrough
            && rice_common::settings::Settings::live().hide_bar_on_fullscreen;
        let bar_bg = if hidden { egui::Color32::TRANSPARENT } else { bar_bg };
        // An open panel would otherwise stay parked over the game, unclickable
        // (the bar is click-through here) and impossible to dismiss.
        if hidden && self.isl_expanded {
            self.want_close = true;
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::none().inner_margin(egui::Margin::symmetric(BAR_PAD_H, BAR_PAD_V)))
            .show(ctx, |ui| {
                // The window is TALLER than the bar whenever the bubble is open, so
                // ui.max_rect() is not the bar. Everything here is positioned against
                // `full`, so pin that to the top strip: using the whole window pushed
                // the workspaces, clock and metrics down into the enlarged area, where
                // the window region clips them away and they vanished.
                let win = ui.max_rect();
                let full = egui::Rect::from_min_size(
                    win.min,
                    egui::vec2(win.width(), (bar_strip_h - BAR_PAD_V * 2.0).max(1.0)),
                );
                // Only the strip gets the bar colour. Anything below it belongs to
                // the bubble and must stay transparent so the island is the only
                // thing drawn there.
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(
                        win.min - egui::vec2(BAR_PAD_H, BAR_PAD_V),
                        egui::vec2(win.width() + BAR_PAD_H * 2.0, bar_strip_h),
                    ),
                    egui::Rounding::ZERO,
                    bar_bg,
                );

                // Frame delta, shared by every in-bar animation (island + workspace indicator).
                let now_i = Instant::now();
                let dt = (now_i - self.last_frame).as_secs_f32().clamp(0.0, 0.05);
                self.last_frame = now_i;


                // Workspaces, clock and metrics are the bar. Over a fullscreen
                // application none of them are worth covering a pixel of it, so
                // the whole strip is skipped -- but the island block below still
                // runs, because a notification IS worth covering a pixel of it.
                if !hidden {
                // allocate_new_ui, no allocate_ui_at_rect: egui 0.29 marco esa
                // como obsoleta. El rectangulo se pasa ahora por UiBuilder.
                ui.allocate_new_ui(egui::UiBuilder::new().max_rect(full), |ui| {
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
                    let derecha = ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(4.0);
                        // La bandeja va pegada al borde derecho, que es donde
                        // lleva estando treinta años.
                        if let Some(name) = self.tray.ui(ui, 18.0) {
                            tray::click(name.as_str());
                        }
                        if !self.tray.is_empty() {
                            ui.add_space(10.0);
                        }

                        // Estado de Alt+F10, junto al resto de indicadores.
                        //
                        // Estaba solo dentro del centro de notificaciones, o sea
                        // a un clic de distancia -- y la pregunta que resuelve
                        // ("¿va a hacer algo si lo pulso?") uno se la hace ANTES
                        // de pulsar, no despues de que no pase nada. El color ya
                        // lo dice de un vistazo; el texto exacto, al pasar por
                        // encima.
                        let (rect_a, resp_a) =
                            ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                        draw_icon(ui.painter(), rect_a.center(), "\u{f03d}", 13.0, atajo.color());
                        resp_a.on_hover_text(match atajo {
                            Atajo::Vivo => "Alt+F10 activo",
                            Atajo::Suspendido => "Alt+F10 suspendido — Win+Shift+Z lo devuelve",
                            Atajo::Caido => "Alt+F10 CAIDO — AutoHotkey no responde",
                        });
                        ui.add_space(12.0);

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
                        // Consumo electrico. Va aqui, pegado a las baterias,
                        // porque las dos cosas responden a la misma pregunta:
                        // cuanta energia se esta yendo.
                        if let Some(c) = &s.consumo {
                            let mut globo = format!(
                                "{:.0} W ahora\nhoy: {:.2} kWh",
                                c.w, c.kwh_hoy
                            );
                            if c.coste_hoy > 0.0 {
                                globo.push_str(&format!(
                                    "\ncoste de hoy: {}{:.2}",
                                    c.moneda, c.coste_hoy
                                ));
                                // Una proyeccion a 24 h y a 30 dias, para no
                                // tener que esperar al final del dia por
                                // curiosidad. Va dicha como lo que es -- una
                                // hipotesis a la potencia de AHORA -- porque
                                // nadie deja el equipo igual las 24 horas, y
                                // presentarla como una prevision seria mentir
                                // con dos decimales.
                                let precio = if c.kwh_hoy > 0.0 {
                                    c.coste_hoy / c.kwh_hoy
                                } else {
                                    0.0
                                };
                                let dia = c.w * 24.0 / 1000.0 * precio;
                                globo.push_str(&format!(
                                    "\n\nsi se quedara a {:.0} W:\n  24 h -> {}{:.2}\n  30 dias -> {}{:.0}",
                                    c.w, c.moneda, dia, c.moneda, dia * 30.0
                                ));
                            } else {
                                globo.push_str("\npon consumo.precio_kwh en rice.json para el coste");
                            }
                            globo.push_str(if c.cpu_medida {
                                "\nCPU medida (LibreHardwareMonitor)"
                            } else {
                                "\nCPU ESTIMADA del uso -- abre LibreHardwareMonitor como admin para medirla"
                            });
                            // El desglose es lo que hace auditable el numero:
                            // sin el, "226 W" solo se puede creer o no creer.
                            globo.push_str(&format!(
                                "\n\nde donde sale:\n  GPU {:.0} W (medida)\n  CPU {:.0} W\n  resto del equipo {:.0} W\n  monitores {:.0} W\n  + perdidas de la fuente y margen",
                                c.gpu_w, c.cpu_w, c.base_w, c.monitores_w
                            ));
                            // Cobertura: si el medidor no vio todas las horas
                            // encendidas, el kWh se queda corto y hay que
                            // decirlo en vez de dar un total con pinta de
                            // completo.
                            if c.horas_encendido > 0.05 {
                                let cobertura = (c.horas_medidas / c.horas_encendido * 100.0).min(100.0);
                                if cobertura < 95.0 {
                                    globo.push_str(&format!(
                                        "\n\nOJO: medido {:.1} h de {:.1} h encendido ({:.0}%).\nLo que falta no esta contado en el kWh.",
                                        c.horas_medidas, c.horas_encendido, cobertura
                                    ));
                                }
                            }
                            globo.push_str("\nes una estimacion AL ALZA, no la lectura del contador");

                            // Se emite en orden inverso al que se lee, porque la
                            // tira se compone de derecha a izquierda. Queda:
                            //   232W  0,24 kWh  S/0,17  5,2 h
                            //
                            // Las horas van al lado del kWh a proposito: un
                            // consumo sin el tiempo que lo produjo no dice nada.
                            // Y son horas ENCENDIDO, no horas medidas -- que el
                            // medidor se perdiera un trozo es problema suyo, no
                            // algo que deba cambiar lo que el dueño lee.
                            ui.colored_label(WARM_SUB, format!("{:.1}h", c.horas_encendido));
                            ui.add_space(6.0);
                            if c.coste_hoy > 0.0 {
                                ui.colored_label(WARM_SUB, format!("{}{:.2}", c.moneda, c.coste_hoy))
                                    .on_hover_text(globo.clone());
                                ui.add_space(6.0);
                            }
                            ui.colored_label(WARM_SUB, format!("{:.2}kWh", c.kwh_hoy))
                                .on_hover_text(globo.clone());
                            ui.add_space(6.0);
                            // Amarillo cuando la CPU va estimada: el numero vale
                            // menos y se nota sin abrir el globo.
                            let col_w = if c.cpu_medida {
                                dim
                            } else {
                                egui::Color32::from_rgb(214, 176, 110)
                            };
                            ui.colored_label(col_w, format!("{:.0}W", c.w))
                                .on_hover_text(globo);
                            ui.add_space(12.0);
                        }

                        // Un bloque por dispositivo conectado: auriculares
                        // rojos para el HyperX, blancos para los AirPods. Se
                        // distinguen de un vistazo sin leer nada.
                        for b in &s.baterias {
                            let color = match b.clase {
                                rice_common::battery::Clase::HyperX => {
                                    egui::Color32::from_rgb(220, 90, 80)
                                }
                                rice_common::battery::Clase::AirPods => {
                                    egui::Color32::from_rgb(238, 238, 240)
                                }
                            };
                            // Esta tira se compone de DERECHA a IZQUIERDA, asi
                            // que lo que va mas a la derecha se emite primero.
                            // Orden final: rayo, auriculares, niveles, estuche.
                            let parte = |nombre: &str| {
                                b.partes.iter().find(|(n, _)| n == nombre).map(|(_, v)| *v)
                            };
                            // Los AirPods son dos baterias, no una: el numero
                            // unico esconde justo lo que hace falta saber, que
                            // es si UNO de los dos esta a punto de morirse.
                            let txt = match (parte("izquierdo"), parte("derecho")) {
                                (Some(izq), Some(der)) => format!("{izq}·{der}"),
                                _ => match b.nivel {
                                    Some(n) => format!("{n}%"),
                                    None => "--".to_string(),
                                },
                            };
                            if let Some(est) = parte("estuche") {
                                ui.colored_label(WARM_SUB, format!("est {est}"));
                                ui.add_space(6.0);
                            }
                            // Una lectura vieja se apaga de color en vez de
                            // desaparecer: sigue siendo el ultimo dato real que
                            // hubo, y verlo apagado dice mas que un "--".
                            let viejo = b.edad.as_secs() > 300;
                            let col_txt = if b.cargando {
                                col(theme::ACCENT_OK)
                            } else if b.nivel.map(|n| n <= 20).unwrap_or(false) {
                                egui::Color32::from_rgb(255, 120, 120)
                            } else if viejo {
                                WARM_SUB
                            } else {
                                dim
                            };
                            let resp_t = ui.colored_label(col_txt, txt);
                            ui.add_space(4.0);
                            let (rb, resp_b) = ui
                                .allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                            draw_icon(ui.painter(), rb.center(), "\u{f025}", 13.0, color);
                            let globo = descripcion_bateria(b);
                            resp_b.on_hover_text(globo.clone());
                            resp_t.on_hover_text(globo.clone());
                            // Rayo solo mientras carga. Va despues del icono
                            // porque la tira se compone de derecha a izquierda,
                            // asi que acaba a su izquierda: "rayo auriculares 53%".
                            if b.cargando {
                                let (rr, resp_r) = ui.allocate_exact_size(
                                    egui::vec2(12.0, 18.0),
                                    egui::Sense::hover(),
                                );
                                draw_icon(
                                    ui.painter(),
                                    rr.center(),
                                    "\u{f0e7}", // fa-bolt
                                    11.0,
                                    col(theme::ACCENT_OK),
                                );
                                resp_r.on_hover_text(globo);
                            }
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
                    // Borde izquierdo de los stats. Se guarda para que la isla
                    // sepa donde parar en vez de crecer por encima.
                    self.stats_left = Some(derecha.response.rect.left());
                });
                });
                }

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

                // Over a fullscreen application the island appears ONLY to
                // deliver a notification. The state machine above still runs
                // either way -- it is what notices a new event and expires an
                // old one -- so what is gated here is the drawing, not the
                // knowing.
                let island_off = hidden && self.isl_notif.is_none();
                if !island_off {

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
                let mut target_w = if notif_open { pad + clock_w + div_gap + extra_w + pad } else { idle_w };

                // La isla PARA antes de los stats en vez de taparlos.
                //
                // La pildora se ancla por su izquierda -- para que el reloj no se
                // mueva -- y se despliega hacia la derecha, que es justo donde
                // viven RAM / CPU / GPU / red y la bandeja. Un aviso con titulo
                // largo se los comia enteros: la notificacion aparecia Y la barra
                // dejaba de informar de nada mientras durase.
                //
                // Se usa el borde del fotograma anterior. Un fotograma de retraso
                // no se ve, y medirlo despues de dibujar es lo unico que da el
                // ancho real del bloque, que cambia con la bandeja y con el modo.
                //
                // Sin stats (pantalla completa) no hay nada que respetar y el
                // aviso puede ocupar lo que necesite.
                if let Some(sl) = self.stats_left {
                    let left_anchor = full.center().x - idle_w / 2.0;
                    let tope = (sl - 12.0 - left_anchor).max(idle_w);
                    target_w = target_w.min(tope);
                }
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
                    egui::Stroke::new(1.0_f32, egui::Color32::from_rgba_unmultiplied(ISL_HI.r(), ISL_HI.g(), ISL_HI.b(), 110)),
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
                        egui::Stroke::new(1.0_f32, egui::Color32::from_rgba_unmultiplied(WARM_SUB.r(), WARM_SUB.g(), WARM_SUB.b(), 70)),
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
                    // Puntos suspensivos, no un corte a hueso.
                    //
                    // La pildora ya no crece por encima de los stats, asi que un
                    // titulo largo TIENE que caber en lo que quede. Dejarlo al
                    // recorte del clip funciona mientras la pildora se despliega
                    // -- ese barrido es lo que hace la animacion -- pero una vez
                    // quieta un texto cortado a mitad de letra se lee como un
                    // fallo de dibujo, no como un texto que no cabia.
                    let sitio = (pill.rect.right() - 12.0 - tx).max(24.0);
                    let recortado = |txt: &str, size: f32, col: egui::Color32| {
                        let mut job = egui::text::LayoutJob::single_section(
                            txt.to_string(),
                            egui::TextFormat { font_id: egui::FontId::proportional(size), color: col, ..Default::default() },
                        );
                        job.wrap = egui::text::TextWrapping {
                            max_width: sitio,
                            max_rows: 1,
                            break_anywhere: true,
                            overflow_character: Some('…'),
                        };
                        job
                    };
                    if ev.body.is_empty() {
                        cp.galley(
                            egui::pos2(tx, cy - 8.0),
                            ui.fonts(|f| f.layout_job(recortado(&ev.title, 13.0, WARM_TEXT))),
                            WARM_TEXT,
                        );
                    } else {
                        cp.galley(
                            egui::pos2(tx, cy - 14.0),
                            ui.fonts(|f| f.layout_job(recortado(&ev.title, 12.5, WARM_TEXT))),
                            WARM_TEXT,
                        );
                        cp.galley(
                            egui::pos2(tx, cy - 1.0),
                            ui.fonts(|f| f.layout_job(recortado(&ev.body, 11.0, WARM_SUB))),
                            WARM_SUB,
                        );
                    }
                    if pill.clicked() {
                        self.isl_notif = None; // click dismisses early
                    }
                } else if expanded {
                    // The pill itself stays compact while expanded; every control
                    // now lives in the vertical panel drawn below.
                    // Only the strip: `rect` is the whole morphed shape while
                    // the panel is open, and using it made every click over the
                    // panel background a dismiss.
                    let clock_hit = egui::Rect::from_min_max(
                        rect.left_top(),
                        egui::pos2(rect.right(), (cy + h / 2.0).min(rect.bottom())),
                    );
                    if ui
                        .interact(clock_hit, egui::Id::new("isl-clock"), egui::Sense::click())
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        // close_panel() cannot be called here -- the shared lock
                        // is still held and it needs all of self -- so ask for it
                        // and let the code past the lock do it. Duplicating the
                        // reset is what broke this before.
                        self.want_close = true;
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
                } // if !island_off
            });
        drop(s);

        if std::mem::take(&mut self.want_close) {
            self.close_panel();
        }
        if std::mem::take(&mut self.want_back) {
            // Back to the action grid: drop the sub-view but stay open.
            self.isl_vol = false;
            self.isl_bright = false;
            self.isl_opacity = false;
            self.isl_media = false;
            self.isl_timer = false;
            self.isl_devices = false;
            self.isl_notifs = false;
            self.vol.clear();
            self.bright.clear();
            self.isl_interact = Instant::now();
        }

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
            // Los DOS botones. Solo con el izquierdo, un clic derecho fuera del
            // panel no lo cerraba y ademas se perdia: la barra se lo tragaba sin
            // hacer nada con el.
            let down = unsafe {
                (GetAsyncKeyState(0x01) as u16 & 0x8000) != 0   // VK_LBUTTON
                    || (GetAsyncKeyState(0x02) as u16 & 0x8000) != 0 // VK_RBUTTON
            };
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
            std::thread::spawn(move || battery_thread(s4, ctx4));
            let s5 = shared.clone();
            let ctx5 = cc.egui_ctx.clone();
            std::thread::spawn(move || consumo_thread(s5, ctx5));
            let s4 = shared.clone();
            let ctx4 = cc.egui_ctx.clone();
            std::thread::spawn(move || island_watcher(s4, ctx4));
            #[cfg(windows)]
            spawn_clickthrough_watcher();
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
                isl_expanded: std::env::var("GLAZEBAR_PANEL").is_ok(),
                isl_interact: Instant::now(),
                last_frame: Instant::now(),
                ws_ind: None,
                hwnd: 0,
                clickthrough: false,
                tray: tray::Tray::new(),
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
                // GLAZEBAR_PANEL=notifs abre ese panel al arrancar. Mismo
                // proposito que GLAZEBAR_ICONTEST: poder mirar como queda algo
                // sin tener que inyectar clics en el escritorio de nadie.
                isl_notifs: std::env::var("GLAZEBAR_PANEL").as_deref() == Ok("notifs"),
                notifs: Vec::new(),
                notifs_stamp: None,
                notifs_checked: Instant::now() - Duration::from_secs(2),
                stats_left: None,
                atajo: Atajo::Vivo,
                atajo_f10: 0,
                atajo_checked: Instant::now() - Duration::from_secs(5),
                atajo_avisado: false,
                notifs_shown: NOTIFS_PAGE,
                want_close: false,
                want_back: false,
                timer_left: Duration::from_secs(25 * 60),
                timer_total: Duration::from_secs(25 * 60),
                timer_running: false,
                timer_tick: Instant::now(),
                win_h: 0,
                panel_rect: None,
                last_opacity_write: Instant::now(),
                last_beat: Instant::now(),
            }))
        }),
    )
}
