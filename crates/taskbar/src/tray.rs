//! Leer la bandeja del sistema de Windows para poder pintarla en nuestra barra.
//!
//! # Por qué así, y no de las tres formas obvias
//!
//! **No se puede alojar la bandeja.** `Shell_NotifyIcon` busca la ventana de
//! clase `Shell_TrayWnd` y le manda un `WM_COPYDATA`. Sólo puede haber una, y es
//! la de explorer; recibir los iconos nosotros significaría sustituir el shell
//! entero. Descartado.
//!
//! **El toolbar clásico ya no existe.** El método de toda la vida era buscar
//! `Shell_TrayWnd > TrayNotifyWnd > SysPager > ToolbarWindow32` y leer sus
//! botones con `TB_GETBUTTON` desde otro proceso. Medido en esta build
//! (Windows 11 26200): `TrayNotifyWnd` está, pero **`SysPager` y
//! `ToolbarWindow32` devuelven 0**. La bandeja es XAML.
//!
//! **Queda UI Automation**, que sí lee el árbol XAML. Con una pega: medido, con
//! la barra de tareas oculta (`SW_HIDE`) el árbol no se realiza y UIA no ve
//! **nada** -- ni un hijo. Necesita estar visible.
//!
//! # La solución: visible pero invisible
//!
//! La barra se deja *mostrada* -- para que el XAML exista y UIA lo lea -- pero
//! con `WS_EX_LAYERED` y alfa 0, así que no pinta un solo píxel, y con
//! `WS_EX_TRANSPARENT`, así que tampoco se come clics. El `ABM_SETSTATE` de
//! auto-ocultar se mantiene, que es lo que hace que Windows devuelva el área de
//! trabajo completa; comprobado, sigue siendo 0,0-1920,1080, de modo que GlazeWM
//! no pierde la franja.
//!
//! Y el modo de fallo es el bueno: si este proceso muere, los estilos se quedan
//! puestos y la barra sigue sin verse.
//!
//! # Los iconos
//!
//! UIA da nombre y rectángulo, no píxeles. Se sacan con
//! `PrintWindow(PW_RENDERFULLCONTENT)` sobre la propia barra y recortando. Ese
//! flag importa: sin él la captura sale entera en negro (medido), porque el
//! contenido XAML no se dibuja por el camino clásico de `WM_PRINT`.

use std::path::PathBuf;

use windows::core::Interface;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, HGDIOBJ,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationInvokePattern,
    TreeScope_Descendants, UIA_InvokePatternId,
};
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

// `PrintWindow` no lo expone el crate en 0.58. Es una funcion plana de user32,
// asi que se declara y ya.
#[link(name = "user32")]
extern "system" {
    fn PrintWindow(hwnd: isize, hdc: isize, flags: u32) -> i32;
}

/// Lo que identifica un icono de aplicación en el árbol de la bandeja. Los del
/// propio sistema (reloj, volumen, red) llevan `SystemTrayIcon` y se dejan
/// fuera: la barra ya tiene reloj, y volumen y red tienen su propio panel.
const APP_ICON_ID: &str = "NotifyItemIcon";

/// Lado del recorte. La bandeja dibuja los iconos de 16x16 centrados en un botón
/// de 32x48 -- medido: botón en 1602,1032 32x48, imagen en 1610,1048 16x16, que
/// es exactamente el centro.
///
/// Se recortan 20 y no 16 a propósito: los dos píxeles de más por lado son
/// fondo garantizado, y de ahí sale el color a descontar.
const ICON: i32 = 20;

pub struct Item {
    pub name: String,
    /// RGBA, `ICON` x `ICON`.
    pub pixels: Vec<u8>,
}

/// Dónde se deja lo leído para que la barra lo recoja.
pub fn state_path() -> PathBuf {
    rice_common::config::config_path("tray.bin")
}

// ------------------------------------------------------------------ invisible

/// Deja la barra de tareas realizada pero sin que se vea ni estorbe.
///
/// Idempotente y barata: se llama en cada vuelta del vigilante porque explorer
/// vuelve a mostrar la ventana por su cuenta al revelar la auto-ocultación, y
/// entonces hay que reafirmar el alfa antes de que llegue a pintarse.
pub fn make_invisible(hwnd: isize) {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetLayeredWindowAttributes, SetWindowLongPtrW, ShowWindow,
        GWL_EXSTYLE, LWA_ALPHA, SW_SHOWNOACTIVATE, WINDOW_EX_STYLE, WS_EX_LAYERED,
        WS_EX_TRANSPARENT,
    };
    let h = HWND(hwnd as *mut _);
    unsafe {
        let cur = GetWindowLongPtrW(h, GWL_EXSTYLE);
        let want = cur | (WS_EX_LAYERED.0 as isize) | (WS_EX_TRANSPARENT.0 as isize);
        if cur != want {
            SetWindowLongPtrW(h, GWL_EXSTYLE, want);
        }
        // Se reafirma siempre, no sólo al cambiar el estilo: poner el estilo no
        // fija el alfa, y explorer puede repintar por su cuenta.
        let _ = SetLayeredWindowAttributes(h, windows::Win32::Foundation::COLORREF(0), 0, LWA_ALPHA);
        let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
        let _ = WINDOW_EX_STYLE(0);
    }
}

/// El área de trabajo, a pantalla completa.
///
/// Con el método anterior la devolvía `ABM_SETSTATE(ABS_AUTOHIDE)`: Windows
/// recalcula el área al poner la barra en auto-ocultar. Pero medido tras un
/// reinicio de explorer, **con auto-ocultar el árbol XAML de la bandeja se
/// desrealiza en cuanto la barra se esconde** -- UIA pasa de 32 botones a 0 --
/// y sin árbol no hay bandeja que leer. Así que ahora la barra queda SIN
/// auto-ocultar (XAML siempre vivo) y el área de trabajo se fija aquí a mano.
///
/// Explorer la vuelve a estrechar cuando le parece (cambios de resolución, de
/// DPI, al re-registrar su appbar), así que esto se reafirma en el bucle del
/// lector: compara y sólo escribe si difiere.
/// Ancla la barra en su posición revelada.
///
/// El área de trabajo la devuelve el auto-ocultar (probado: `SPI_SETWORKAREA`
/// devuelve TRUE y no cambia nada -- el appbar registrado manda). Pero el
/// auto-ocultar desliza la ventana a y=1078, un filo de 2 px, y **ahí es donde
/// el XAML se desrealiza** y UIA se queda sin árbol. Medido: deslizada, 0
/// botones; en su sitio, 32.
///
/// Así que auto-ocultar para el área, y esto para deshacer el deslizamiento:
/// la ventana anclada en pantalla (y = alto - barra), a alfa 0. Explorer la
/// vuelve a deslizar cuando quiere; el gancho de LOCATIONCHANGE y el bucle del
/// lector la devuelven. La guarda de "solo si difiere" evita que nuestro propio
/// SetWindowPos realimente el gancho en bucle.
pub fn pin_revealed(hwnd: isize) {
    #[link(name = "user32")]
    extern "system" {
        fn GetSystemMetrics(i: i32) -> i32;
        fn SetWindowPos(h: isize, after: isize, x: i32, y: i32, cx: i32, cy: i32, f: u32) -> i32;
    }
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOZORDER: u32 = 0x0004;
    const SWP_NOACTIVATE: u32 = 0x0010;
    unsafe {
        let mut r = RECT::default();
        if GetWindowRect(HWND(hwnd as *mut _), &mut r).is_err() {
            return;
        }
        let want_top = GetSystemMetrics(1) - (r.bottom - r.top);
        if r.top != want_top {
            SetWindowPos(hwnd, 0, r.left, want_top, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }
}

/// Deshace lo anterior: opaca y otra vez clicable.
pub fn make_normal(hwnd: isize) {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_LAYERED, WS_EX_TRANSPARENT,
    };
    let h = HWND(hwnd as *mut _);
    unsafe {
        let cur = GetWindowLongPtrW(h, GWL_EXSTYLE);
        let want = cur & !(WS_EX_LAYERED.0 as isize) & !(WS_EX_TRANSPARENT.0 as isize);
        SetWindowLongPtrW(h, GWL_EXSTYLE, want);
    }
}

// ----------------------------------------------------------------------- UIA

/// Un cliente de UIA vivo. Crearlo cuesta, así que se hace una vez.
pub struct Uia {
    auto: IUIAutomation,
}

impl Uia {
    pub fn new() -> Option<Self> {
        unsafe {
            // MULTITHREADED: este hilo no bombea mensajes, y en un apartamento
            // de un solo hilo sin bomba las llamadas COM a explorer se quedarían
            // colgadas.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let auto: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
            Some(Self { auto })
        }
    }

    /// Los iconos de aplicación que hay ahora mismo, en orden de izquierda a
    /// derecha.
    pub fn items(&self, hwnd: isize) -> Vec<(String, RECT)> {
        let mut out = Vec::new();
        unsafe {
            let Ok(root) = self.auto.ElementFromHandle(HWND(hwnd as *mut _)) else { return out };
            // Condición "todo" y filtrado aquí: construir una condición por
            // propiedad exige montar un VARIANT, y el árbol de la bandeja son
            // unas decenas de elementos una vez cada dos segundos.
            let Ok(cond) = self.auto.CreateTrueCondition() else { return out };
            let Ok(all) = root.FindAll(TreeScope_Descendants, &cond) else { return out };
            let Ok(n) = all.Length() else { return out };
            for i in 0..n {
                let Ok(el) = all.GetElement(i) else { continue };
                let Ok(id) = el.CurrentAutomationId() else { continue };
                if id.to_string() != APP_ICON_ID {
                    continue;
                }
                let name = el.CurrentName().map(|b| b.to_string()).unwrap_or_default();
                let Ok(r) = el.CurrentBoundingRectangle() else { continue };
                out.push((name, r));
            }
        }
        out.sort_by_key(|(_, r)| r.left);
        out
    }

    /// Pulsa el icono cuyo nombre empiece por `prefix`.
    ///
    /// Por prefijo y no por igualdad porque el nombre ES el tooltip, y el
    /// tooltip cambia solo: "Discord - 3 mensajes" un segundo y "Discord" al
    /// siguiente. Lo que se guardó al pintar la fila puede no ser ya exacto.
    pub fn invoke(&self, hwnd: isize, prefix: &str) -> bool {
        let key = short(prefix);
        unsafe {
            let Ok(root) = self.auto.ElementFromHandle(HWND(hwnd as *mut _)) else { return false };
            let Ok(cond) = self.auto.CreateTrueCondition() else { return false };
            let Ok(all) = root.FindAll(TreeScope_Descendants, &cond) else { return false };
            let Ok(n) = all.Length() else { return false };
            for i in 0..n {
                let Ok(el) = all.GetElement(i) else { continue };
                let Ok(id) = el.CurrentAutomationId() else { continue };
                if id.to_string() != APP_ICON_ID {
                    continue;
                }
                let name = el.CurrentName().map(|b| b.to_string()).unwrap_or_default();
                if !same(&short(&name), &key) {
                    continue;
                }
                if let Ok(p) = el.GetCurrentPattern(UIA_InvokePatternId) {
                    if let Ok(inv) = p.cast::<IUIAutomationInvokePattern>() {
                        return inv.Invoke().is_ok();
                    }
                }
            }
        }
        false
    }
}

/// La parte estable de un tooltip: hasta el primer salto de línea o guion.
fn short(s: &str) -> String {
    let s = s.split('\n').next().unwrap_or(s);
    let s = s.split(" - ").next().unwrap_or(s);
    s.trim().to_lowercase()
}

/// ¿Son el mismo icono?
///
/// No basta con la igualdad. Entre que la barra pintó la fila y el usuario la
/// pulsó, el tooltip ha podido crecer o encogerse solo -- "Discord" pasa a
/// "Discord - 3 mensajes sin leer" y vuelve. Que uno sea prefijo del otro es lo
/// que sobrevive a eso; el mínimo de tres letras evita que un tooltip que se
/// haya quedado en nada empareje con cualquier cosa.
fn same(a: &str, b: &str) -> bool {
    if a.len() < 3 || b.len() < 3 {
        return a == b;
    }
    a == b || a.starts_with(b) || b.starts_with(a)
}

// ------------------------------------------------------------------- píxeles

/// Captura la barra entera una vez y recorta cada icono de ahí.
///
/// Una sola captura para todos: `PrintWindow` sobre una ventana XAML cuesta
/// varios milisegundos, y hacerlo por icono multiplicaría eso por nada.
pub fn grab(hwnd: isize, items: &[(String, RECT)]) -> Vec<Item> {
    const PW_RENDERFULLCONTENT: u32 = 0x2;
    let h = HWND(hwnd as *mut _);
    let mut wr = RECT::default();
    if unsafe { GetWindowRect(h, &mut wr) }.is_err() {
        return Vec::new();
    }
    let (w, ht) = (wr.right - wr.left, wr.bottom - wr.top);
    if w <= 0 || ht <= 0 {
        return Vec::new();
    }

    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, w, ht);
        let old = SelectObject(mem, HGDIOBJ(bmp.0));

        let ok = PrintWindow(hwnd, mem.0 as isize, PW_RENDERFULLCONTENT) != 0;

        let mut buf = vec![0u8; (w * ht * 4) as usize];
        if ok {
            let mut bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    // Negativo: filas de arriba abajo.
                    biHeight: -ht,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: 0,
                    ..Default::default()
                },
                ..Default::default()
            };
            GetDIBits(mem, bmp, 0, ht as u32, Some(buf.as_mut_ptr() as *mut _), &mut bi, DIB_RGB_COLORS);
        }

        SelectObject(mem, old);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        if !ok {
            return Vec::new();
        }

        items
            .iter()
            .filter_map(|(name, r)| {
                // El rectángulo viene en coordenadas de pantalla; la captura
                // empieza en la esquina de la ventana.
                let cx = r.left - wr.left + (r.right - r.left) / 2;
                let cy = r.top - wr.top + (r.bottom - r.top) / 2;
                let x0 = cx - ICON / 2;
                let y0 = cy - ICON / 2;
                if x0 < 0 || y0 < 0 || x0 + ICON > w || y0 + ICON > ht {
                    return None;
                }
                // El fondo de la barra de tareas es acrílico: un desenfoque del
                // fondo de escritorio, así que no hay un color fijo que
                // descontar. Se mide en las cuatro esquinas del propio recorte,
                // que por construcción son fondo.
                let at = |x: i32, y: i32| {
                    let p = (((y0 + y) * w + x0 + x) * 4) as usize;
                    [buf[p + 2] as i32, buf[p + 1] as i32, buf[p] as i32]
                };
                let corners = [at(0, 0), at(ICON - 1, 0), at(0, ICON - 1), at(ICON - 1, ICON - 1)];
                let bg = [
                    corners.iter().map(|c| c[0]).sum::<i32>() / 4,
                    corners.iter().map(|c| c[1]).sum::<i32>() / 4,
                    corners.iter().map(|c| c[2]).sum::<i32>() / 4,
                ];

                let mut px = Vec::with_capacity((ICON * ICON * 4) as usize);
                for y in 0..ICON {
                    for x in 0..ICON {
                        let c = at(x, y);
                        // Alfa por distancia al fondo, no por igualdad: los
                        // bordes del icono vienen suavizados contra la barra, y
                        // un recorte duro los dejaría con una orla azul.
                        let d = (c[0] - bg[0]).abs().max((c[1] - bg[1]).abs()).max((c[2] - bg[2]).abs());
                        let a = (d * 5).clamp(0, 255) as u8;
                        px.extend_from_slice(&[c[0] as u8, c[1] as u8, c[2] as u8, a]);
                    }
                }
                Some(Item { name: name.clone(), pixels: px })
            })
            .collect()
    }
}

// ------------------------------------------------------------------ publicar

/// Formato: `RICETRAY`, versión, número de iconos, y luego por icono su nombre y
/// sus píxeles en crudo. Crudo y no PNG para que la barra no necesite un
/// descodificador: son 4 KB por icono y se escriben una vez cada dos segundos.
pub fn publish(items: &[Item]) {
    let mut out = Vec::with_capacity(items.len() * (ICON * ICON * 4) as usize + 64);
    out.extend_from_slice(b"RICETRAY");
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&(items.len() as u32).to_le_bytes());
    out.extend_from_slice(&(ICON as u32).to_le_bytes());
    for it in items {
        let n = it.name.as_bytes();
        out.extend_from_slice(&(n.len() as u32).to_le_bytes());
        out.extend_from_slice(n);
        out.extend_from_slice(&it.pixels);
    }
    // Escritura atómica: la barra lee este archivo desde otro proceso y no debe
    // poder ver medio archivo.
    let path = state_path();
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, &out).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// Ha cambiado algo desde la última vez? Evita reescribir el archivo -- y por
/// tanto despertar a la barra -- dos veces por segundo sin motivo.
pub fn digest(items: &[Item]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for it in items {
        for b in it.name.as_bytes().iter().chain(it.pixels.iter()) {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
    }
    h
}
