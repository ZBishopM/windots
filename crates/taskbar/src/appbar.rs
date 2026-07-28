//! Un appbar propio por cada monitor SIN barra de tareas.
//!
//! El porqué, en cadena:
//!
//!   1. La barra de tareas de Windows tiene que quedarse realizada (alfa 0)
//!      para poder leerle la bandeja; ver `tray.rs`. Realizada, su appbar
//!      reserva 48 px abajo en el monitor primario y no hay forma de soltarla
//!      -- se probaron autohide, `SPI_SETWORKAREA` y `ABM_REMOVE`.
//!   2. GlazeWM tila dentro del área de trabajo, así que esos 48 px se
//!      compensan con `outer_gap.bottom: -28px`... que es global.
//!   3. En el monitor 2 no hay barra de tareas, no hay reserva, y el -28
//!      mandaba las ventanas 28 px por debajo del borde (medido: 1468 en una
//!      pantalla de 1440).
//!
//! En vez de pedirle a GlazeWM gaps por monitor (no existen), se iguala la
//! realidad: cada monitor sin barra de tareas recibe un appbar invisible de
//! los mismos 48 px. Reserva simétrica, gap uniforme, y las ventanas terminan
//! a 20 px del borde en todas las pantallas.
//!
//! El modo de fallo es el bueno: un appbar se desregistra solo cuando su
//! ventana muere, así que si este proceso cae, Windows recupera el área y lo
//! peor que pasa es que el monitor 2 tila 28 px más abajo hasta que el
//! supervisor lo reviva.

/// Los mismos 48 px que reserva la barra de tareas real en el primario.
const STRIP: i32 = 48;

#[repr(C)]
struct MonitorInfo {
    cb_size: u32,
    rc_monitor: [i32; 4],
    rc_work: [i32; 4],
    flags: u32,
}

#[link(name = "user32")]
extern "system" {
    fn EnumDisplayMonitors(
        hdc: isize,
        clip: *const core::ffi::c_void,
        cb: extern "system" fn(isize, isize, *mut [i32; 4], isize) -> i32,
        lparam: isize,
    ) -> i32;
    fn GetMonitorInfoW(mon: isize, mi: *mut MonitorInfo) -> i32;
    fn CreateWindowExW(
        ex: u32,
        class: *const u16,
        name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: isize,
        menu: isize,
        inst: isize,
        param: isize,
    ) -> isize;
    fn RegisterClassW(wc: *const WndClass) -> u16;
    fn DefWindowProcW(h: isize, m: u32, w: usize, l: isize) -> isize;
}

#[repr(C)]
struct WndClass {
    style: u32,
    wnd_proc: extern "system" fn(isize, u32, usize, isize) -> isize,
    cls_extra: i32,
    wnd_extra: i32,
    instance: isize,
    icon: isize,
    cursor: isize,
    background: isize,
    menu_name: *const u16,
    class_name: *const u16,
}

extern "system" fn wnd_proc(h: isize, m: u32, w: usize, l: isize) -> isize {
    unsafe { DefWindowProcW(h, m, w, l) }
}

extern "system" fn enum_cb(mon: isize, _dc: isize, _rc: *mut [i32; 4], lparam: isize) -> i32 {
    unsafe {
        let out = &mut *(lparam as *mut Vec<(i32, i32, i32, i32, bool)>);
        let mut mi = MonitorInfo {
            cb_size: std::mem::size_of::<MonitorInfo>() as u32,
            rc_monitor: [0; 4],
            rc_work: [0; 4],
            flags: 0,
        };
        if GetMonitorInfoW(mon, &mut mi) != 0 {
            const MONITORINFOF_PRIMARY: u32 = 1;
            out.push((
                mi.rc_monitor[0],
                mi.rc_monitor[1],
                mi.rc_monitor[2],
                mi.rc_monitor[3],
                mi.flags & MONITORINFOF_PRIMARY != 0,
            ));
        }
    }
    1
}

/// Registra un appbar de [`STRIP`] px en el borde inferior de cada monitor no
/// primario. Llamar una vez; las ventanas viven lo que viva el proceso.
pub fn reserve_secondary_strips() {
    let mut monitors: Vec<(i32, i32, i32, i32, bool)> = Vec::new();
    unsafe {
        EnumDisplayMonitors(0, std::ptr::null(), enum_cb, &mut monitors as *mut _ as isize);
    }

    let class: Vec<u16> = "rice-appbar\0".encode_utf16().collect();
    let wc = WndClass {
        style: 0,
        wnd_proc,
        cls_extra: 0,
        wnd_extra: 0,
        instance: 0,
        icon: 0,
        cursor: 0,
        background: 0,
        menu_name: std::ptr::null(),
        class_name: class.as_ptr(),
    };
    unsafe { RegisterClassW(&wc) };

    for (left, _top, right, bottom, primary) in monitors {
        if primary {
            continue; // ahí ya reserva la barra de tareas real
        }
        unsafe {
            // Ventana real pero nunca mostrada: el appbar necesita un HWND al
            // que atar la reserva, no una superficie que pintar.
            let h = CreateWindowExW(
                0x0000_0080, // WS_EX_TOOLWINDOW: fuera de alt-tab
                class.as_ptr(),
                class.as_ptr(),
                0, // sin WS_VISIBLE
                left,
                bottom - STRIP,
                right - left,
                STRIP,
                0,
                0,
                0,
                0,
            );
            if h == 0 {
                continue;
            }
            let mut d = crate::win::AppBarData {
                cb_size: std::mem::size_of::<crate::win::AppBarData>() as u32,
                hwnd: h,
                callback_message: 0,
                edge: 3, // ABE_BOTTOM
                rc: crate::win::Rect { left, top: bottom - STRIP, right, bottom },
                lparam: 0,
            };
            const ABM_NEW: u32 = 0x0000_0000;
            const ABM_QUERYPOS: u32 = 0x0000_0002;
            const ABM_SETPOS: u32 = 0x0000_0003;
            crate::win::SHAppBarMessage(ABM_NEW, &mut d);
            crate::win::SHAppBarMessage(ABM_QUERYPOS, &mut d);
            // QUERYPOS puede recortar el rect; se reafirma el alto pedido.
            d.rc.top = d.rc.bottom - STRIP;
            crate::win::SHAppBarMessage(ABM_SETPOS, &mut d);
        }
    }
}
