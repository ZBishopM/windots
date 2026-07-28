//! Iconos para las filas.
//!
//! # Por qué hay un hilo de por medio
//!
//! `SHGetFileInfoW` con `SHGFI_ICON` no es una consulta barata: para un `.lnk`
//! resuelve el acceso directo, y para una asociación puede acabar cargando la
//! DLL de una extensión del shell. Es la misma familia de llamada que
//! `ShellExecuteW`, que medida aquí bloqueaba **440-930 ms en caliente y más de
//! 5 s en frío** -- y que se sacó del hilo de interfaz por eso mismo. Pedir
//! iconos desde el bucle de dibujo sería repetir el error, sólo que nueve veces
//! por fotograma.
//!
//! Así que aquí no se busca nada: se pregunta a la caché, y si no está, se
//! encarga y se dibuja un hueco. Cuando el icono llega, el hilo pide un
//! repintado y aparece.
//!
//! # Y por qué las claves no son las rutas
//!
//! Para un archivo el icono depende de la extensión, no del archivo. Con
//! `SHGFI_USEFILEATTRIBUTES` el shell responde a partir del nombre sin tocar el
//! disco -- ni siquiera hace falta que el archivo exista -- así que un millón de
//! `.rs` son una sola entrada en la caché y una sola llamada. Las aplicaciones sí
//! van por ruta: cada una tiene el suyo.

use eframe::egui;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

/// Lado del icono que se pide al shell. 32 y no 16: la fila mide 34 px y el
/// icono se dibuja a 20, de modo que reducir uno grande se ve limpio mientras
/// que agrandar uno de 16 se ve borroso.
const SIZE: usize = 32;

/// Un icono ya convertido a píxeles, listo para subir a la GPU.
type Rgba = Vec<u8>;

/// Sacar el icono del archivo que existe en disco.
pub const REAL: u32 = 0;
/// Icono genérico del tipo, deducido del nombre. No toca el disco.
pub const FILE: u32 = 0x80;
/// Icono de carpeta.
pub const DIR: u32 = 0x10;

#[derive(Default)]
struct Cache {
    /// Clave -> píxeles. `None` = se intentó y no había icono, para no volver a
    /// pedirlo en cada fotograma.
    done: HashMap<String, Option<Rgba>>,
    /// Encargados y todavía sin respuesta.
    asked: HashMap<String, ()>,
}

pub struct Icons {
    cache: Arc<Mutex<Cache>>,
    tx: Sender<(String, String, u32)>,
    /// Texturas ya subidas. Aparte de la caché porque sólo se pueden crear desde
    /// el hilo de la interfaz, y el hilo de extracción no lo es.
    tex: HashMap<String, egui::TextureHandle>,
    alive: Arc<AtomicBool>,
}

impl Icons {
    pub fn new(repaint: Arc<dyn Fn() + Send + Sync>) -> Self {
        let cache = Arc::new(Mutex::new(Cache::default()));
        let alive = Arc::new(AtomicBool::new(true));
        let (tx, rx) = std::sync::mpsc::channel::<(String, String, u32)>();
        {
            let cache = cache.clone();
            let alive = alive.clone();
            std::thread::Builder::new()
                .name("icons".into())
                .spawn(move || {
                    #[cfg(windows)]
                    unsafe {
                        // El shell es COM. Sin esto cada llamada pagaría su
                        // propia inicialización.
                        #[link(name = "ole32")]
                        extern "system" {
                            fn CoInitializeEx(r: isize, f: u32) -> i32;
                        }
                        const COINIT_APARTMENTTHREADED: u32 = 0x2;
                        CoInitializeEx(0, COINIT_APARTMENTTHREADED);
                    }
                    while let Ok((key, path, attrs)) = rx.recv() {
                        let px = extract(&path, attrs);
                        let mut c = cache.lock().unwrap();
                        c.asked.remove(&key);
                        c.done.insert(key, px);
                        drop(c);
                        if alive.load(Ordering::Relaxed) {
                            repaint();
                        }
                    }
                })
                .ok();
        }
        Self { cache, tx, tex: HashMap::new(), alive }
    }

    /// El icono de `path`, o `None` mientras no esté.
    ///
    /// `attrs` es [`REAL`] para sacar el icono del archivo que hay en disco, o
    /// [`FILE`] / [`DIR`] para que el shell responda desde el nombre sin tocar
    /// el disco. Lo segundo es lo que hace que buscar en dos millones de
    /// archivos no se convierta en dos millones de consultas al shell.
    pub fn get(
        &mut self,
        ctx: &egui::Context,
        key: &str,
        path: &str,
        attrs: u32,
    ) -> Option<egui::TextureId> {
        if let Some(t) = self.tex.get(key) {
            return Some(t.id());
        }
        let mut c = self.cache.lock().unwrap();
        match c.done.get(key) {
            Some(None) => return None, // no tiene icono; no insistir
            Some(Some(px)) => {
                let img = egui::ColorImage::from_rgba_unmultiplied([SIZE, SIZE], px);
                drop(c);
                let t = ctx.load_texture(key, img, egui::TextureOptions::LINEAR);
                let id = t.id();
                self.tex.insert(key.to_string(), t);
                return Some(id);
            }
            None => {}
        }
        if c.asked.insert(key.to_string(), ()).is_none() {
            let _ = self.tx.send((key.to_string(), path.to_string(), attrs));
        }
        None
    }
}

impl Drop for Icons {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------- extracción

#[cfg(windows)]
mod sys {
    #[repr(C)]
    pub struct ShFileInfoW {
        pub h_icon: isize,
        pub i_icon: i32,
        pub dw_attributes: u32,
        pub sz_display_name: [u16; 260],
        pub sz_type_name: [u16; 80],
    }
    impl Default for ShFileInfoW {
        fn default() -> Self {
            Self {
                h_icon: 0,
                i_icon: 0,
                dw_attributes: 0,
                sz_display_name: [0; 260],
                sz_type_name: [0; 80],
            }
        }
    }

    #[repr(C)]
    #[derive(Default)]
    pub struct IconInfo {
        pub f_icon: i32,
        pub x_hotspot: u32,
        pub y_hotspot: u32,
        pub hbm_mask: isize,
        pub hbm_color: isize,
    }

    #[repr(C)]
    #[derive(Default)]
    pub struct BitmapInfoHeader {
        pub size: u32,
        pub width: i32,
        pub height: i32,
        pub planes: u16,
        pub bit_count: u16,
        pub compression: u32,
        pub size_image: u32,
        pub x_ppm: i32,
        pub y_ppm: i32,
        pub clr_used: u32,
        pub clr_important: u32,
    }

    #[link(name = "shell32")]
    extern "system" {
        pub fn SHGetFileInfoW(
            path: *const u16,
            attrs: u32,
            psfi: *mut ShFileInfoW,
            cb: u32,
            flags: u32,
        ) -> usize;
    }
    #[link(name = "user32")]
    extern "system" {
        pub fn GetIconInfo(icon: isize, info: *mut IconInfo) -> i32;
        pub fn DestroyIcon(icon: isize) -> i32;
        pub fn GetDC(h: isize) -> isize;
        pub fn ReleaseDC(h: isize, dc: isize) -> i32;
    }
    #[link(name = "gdi32")]
    extern "system" {
        pub fn GetDIBits(
            dc: isize,
            bmp: isize,
            start: u32,
            lines: u32,
            bits: *mut u8,
            bi: *mut BitmapInfoHeader,
            usage: u32,
        ) -> i32;
        pub fn DeleteObject(o: isize) -> i32;
    }

    pub const SHGFI_ICON: u32 = 0x0000_0100;
    pub const SHGFI_LARGEICON: u32 = 0x0000_0000;
    pub const SHGFI_USEFILEATTRIBUTES: u32 = 0x0000_0010;
    pub const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
    pub const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
}

#[cfg(windows)]
fn extract(path: &str, attrs: u32) -> Option<Rgba> {
    use sys::*;

    let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    let mut sfi = ShFileInfoW::default();
    let mut flags = SHGFI_ICON | SHGFI_LARGEICON;
    if attrs != REAL {
        // Responde desde la asociación del nombre y no toca el disco: el archivo
        // ni siquiera tiene que existir.
        flags |= SHGFI_USEFILEATTRIBUTES;
    }

    unsafe {
        let ok = SHGetFileInfoW(
            wide.as_ptr(),
            attrs,
            &mut sfi,
            std::mem::size_of::<ShFileInfoW>() as u32,
            flags,
        );
        if ok == 0 || sfi.h_icon == 0 {
            return None;
        }
        let out = icon_to_rgba(sfi.h_icon);
        DestroyIcon(sfi.h_icon);
        out
    }
}

/// HICON -> RGBA, en el orden de filas y el orden de canales que quiere egui.
#[cfg(windows)]
unsafe fn icon_to_rgba(hicon: isize) -> Option<Rgba> {
    use sys::*;

    let mut ii = IconInfo::default();
    if GetIconInfo(hicon, &mut ii) == 0 {
        return None;
    }
    let mut buf = vec![0u8; SIZE * SIZE * 4];
    let mut bi = BitmapInfoHeader {
        size: std::mem::size_of::<BitmapInfoHeader>() as u32,
        width: SIZE as i32,
        // Negativo = filas de arriba abajo, que es el orden que espera egui. En
        // positivo la imagen sale del revés.
        height: -(SIZE as i32),
        planes: 1,
        bit_count: 32,
        ..Default::default()
    };
    let dc = GetDC(0);
    let got = GetDIBits(dc, ii.hbm_color, 0, SIZE as u32, buf.as_mut_ptr(), &mut bi, 0);
    ReleaseDC(0, dc);
    // GetIconInfo entrega copias de los dos mapas de bits y son nuestras.
    if ii.hbm_color != 0 {
        DeleteObject(ii.hbm_color);
    }
    if ii.hbm_mask != 0 {
        DeleteObject(ii.hbm_mask);
    }
    if got == 0 {
        return None;
    }

    // GDI entrega BGRA. Además, un icono sin canal alfa llega con todo el alfa a
    // cero, y dibujarlo tal cual da un icono invisible: si no hay ni un píxel
    // opaco, se toma la imagen entera como opaca.
    let any_alpha = buf.chunks_exact(4).any(|px| px[3] != 0);
    for px in buf.chunks_exact_mut(4) {
        px.swap(0, 2);
        if !any_alpha {
            px[3] = 255;
        }
    }
    Some(buf)
}

#[cfg(not(windows))]
fn extract(_path: &str, _attrs: u32) -> Option<Rgba> {
    None
}
