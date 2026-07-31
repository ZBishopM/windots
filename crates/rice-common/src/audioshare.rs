//! Un anillo de audio en memoria compartida: un escritor, varios lectores.
//!
//! POR QUE EXISTE: había **tres capturas WASAPI del mismo audio del sistema** a
//! la vez. Cada `glaze-bar` lanzaba su propio `sysaudio-loopback.exe` para el
//! espectro (dos monitores, dos helpers) y el grabador lanzaba otro para los
//! clips. Tres procesos, tres clientes de loopback, tres copias del mismo flujo.
//!
//! Ahora captura UNO y publica aquí; los demás leen. La ventaja no es sólo el
//! recuento de procesos: los tres helpers competían por el mismo endpoint y cada
//! uno mantenía su propio hilo de sondeo.
//!
//! Un escritor y varios lectores, sin bloqueos. El contador de fotogramas es
//! monótono y se publica con `Release`; el lector lo lee con `Acquire` y copia
//! hacia atrás desde ahí. Una lectura puede pillar un fotograma a medio escribir
//! si el lector va lentísimo -- para un visualizador eso es un cuadro feo y nada
//! más, y no vale la pena un bloqueo que el escritor tendría que pagar 100 veces
//! por segundo.
//!
//! Se usan `extern "system"` a pelo, igual que `win`, para que esto no arrastre
//! una dependencia de metadatos Win32 a quien sólo quiere ocho barritas.

#[cfg(windows)]
use std::sync::atomic::{AtomicU64, Ordering};

/// `RASH`, para no leer basura si alguien más toma ese nombre.
const MAGIC: u32 = 0x4841_5352;
const VERSION: u32 = 1;
/// Un segundo a 48 kHz. De sobra: el espectro lee 1024 fotogramas cada ~20 ms,
/// así que sólo se perdería datos si un lector se durmiera un segundo entero.
pub const CAPACITY_FRAMES: usize = 48_000;
pub const CHANNELS: usize = 2;
const HEADER_BYTES: usize = 64;
const DATA_BYTES: usize = CAPACITY_FRAMES * CHANNELS * 2;
const TOTAL_BYTES: usize = HEADER_BYTES + DATA_BYTES;

/// Nombre del mapeo del audio del sistema.
///
/// `Local\` y NO `Global\`, que es lo que usa el resto de la casa para mutex y
/// eventos. La diferencia no es de estilo: crear una SECCIÓN (un mapeo de
/// archivo) en el espacio global exige `SeCreateGlobalPrivilege`, que un proceso
/// de usuario sin elevar no tiene; los mutex y los eventos no piden nada de eso.
/// Costó un rato de "no existe el anillo" sin un solo error a la vista, porque el
/// fallo estaba en la creación y se lo tragaba un `Option`.
///
/// `Local\` basta: todo el rice corre en la misma sesión.
pub const SYS_NAME: &str = "Local\\rice-audio-sys";

#[cfg(windows)]
#[repr(C)]
struct Header {
    magic: u32,
    version: u32,
    sample_rate: u32,
    channels: u32,
    /// Fotogramas escritos desde que arrancó el publicador. Monótono: sirve a la
    /// vez de posición en el anillo (módulo capacidad) y de señal de vida.
    written: AtomicU64,
    capacity: u32,
    _pad: u32,
    /// Lo suben los lectores en cada lectura. Es como un publicador arrancado
    /// SOLO para las barras sabe que ya no hace falta y se va, en vez de quedarse
    /// de huerfano cuando muere quien lo lanzo.
    reads: AtomicU64,
}

#[cfg(windows)]
mod ffi {
    #[link(name = "kernel32")]
    extern "system" {
        pub fn CreateFileMappingW(
            file: isize,
            attrs: *const core::ffi::c_void,
            protect: u32,
            hi: u32,
            lo: u32,
            name: *const u16,
        ) -> isize;
        pub fn OpenFileMappingW(access: u32, inherit: i32, name: *const u16) -> isize;
        pub fn MapViewOfFile(map: isize, access: u32, hi: u32, lo: u32, bytes: usize) -> *mut u8;
        pub fn UnmapViewOfFile(addr: *mut u8) -> i32;
        pub fn CloseHandle(h: isize) -> i32;
    }
    pub const PAGE_READWRITE: u32 = 0x04;
    pub const FILE_MAP_ALL_ACCESS: u32 = 0x000F_001F;
    pub const INVALID_HANDLE_VALUE: isize = -1;
}

/// Mapeo vivo. Cierra el handle y desmapea al soltarse.
#[cfg(windows)]
struct View {
    map: isize,
    base: *mut u8,
}

#[cfg(windows)]
impl Drop for View {
    fn drop(&mut self) {
        unsafe {
            ffi::UnmapViewOfFile(self.base);
            ffi::CloseHandle(self.map);
        }
    }
}

// El puntero apunta a memoria compartida que se sincroniza con el atómico de la
// cabecera; moverlo entre hilos es correcto.
#[cfg(windows)]
unsafe impl Send for View {}

#[cfg(windows)]
impl View {
    fn header(&self) -> &Header {
        unsafe { &*(self.base as *const Header) }
    }
    fn data(&self) -> *mut i16 {
        unsafe { self.base.add(HEADER_BYTES) as *mut i16 }
    }
}

/// El lado que captura. Sólo debe haber uno; quien lo crea se asegura con un
/// mutex con nombre.
#[cfg(windows)]
pub struct Publisher {
    view: View,
}

#[cfg(windows)]
impl Publisher {
    pub fn create(name: &str, sample_rate: u32) -> Option<Self> {
        let w = crate::win::wide(name);
        unsafe {
            let map = ffi::CreateFileMappingW(
                ffi::INVALID_HANDLE_VALUE,
                std::ptr::null(),
                ffi::PAGE_READWRITE,
                0,
                TOTAL_BYTES as u32,
                w.as_ptr(),
            );
            if map == 0 {
                return None;
            }
            let base = ffi::MapViewOfFile(map, ffi::FILE_MAP_ALL_ACCESS, 0, 0, TOTAL_BYTES);
            if base.is_null() {
                ffi::CloseHandle(map);
                return None;
            }
            let h = &mut *(base as *mut Header);
            h.magic = MAGIC;
            h.version = VERSION;
            h.sample_rate = sample_rate;
            h.channels = CHANNELS as u32;
            h.capacity = CAPACITY_FRAMES as u32;
            // El contador NO se pone a cero: si otro publicador vivió antes en
            // este mismo mapeo, retroceder haría que los lectores creyeran tener
            // datos nuevos que no lo son. Sólo crece.
            Some(Self { view: View { map, base } })
        }
    }

    /// Añade fotogramas intercalados s16 (L,R,L,R...).
    pub fn write(&self, frames: &[i16]) {
        let n = frames.len() / CHANNELS;
        if n == 0 {
            return;
        }
        let h = self.view.header();
        let start = h.written.load(Ordering::Relaxed) as usize % CAPACITY_FRAMES;
        let data = self.view.data();
        // Dos memcpy y no 48.000 modulos por segundo: el trozo hasta el final del
        // anillo y, si hace falta, el que da la vuelta.
        let primero = n.min(CAPACITY_FRAMES - start);
        unsafe {
            std::ptr::copy_nonoverlapping(
                frames.as_ptr(),
                data.add(start * CHANNELS),
                primero * CHANNELS,
            );
            if n > primero {
                std::ptr::copy_nonoverlapping(
                    frames.as_ptr().add(primero * CHANNELS),
                    data,
                    (n - primero) * CHANNELS,
                );
            }
        }
        // Release: los fotogramas de arriba tienen que ser visibles antes que el
        // contador que dice que están.
        h.written.fetch_add(n as u64, Ordering::Release);
    }

    /// Cuántas lecturas se han hecho. Un publicador arrancado sólo para servir a
    /// las barras se apaga cuando esto deja de subir.
    pub fn reads(&self) -> u64 {
        self.view.header().reads.load(Ordering::Relaxed)
    }
}

/// El lado que mira. Puede haber tantos como quieran.
#[cfg(windows)]
pub struct Reader {
    view: View,
    /// Último contador visto, para saber si el publicador sigue vivo.
    last_seen: std::cell::Cell<u64>,
}

#[cfg(windows)]
impl Reader {
    pub fn open(name: &str) -> Option<Self> {
        let w = crate::win::wide(name);
        unsafe {
            // ALL_ACCESS y no READ: el lector no toca el audio, pero SI escribe
            // el contador `reads`, y con un mapeo de solo lectura esa escritura
            // seria una violacion de acceso.
            let map = ffi::OpenFileMappingW(ffi::FILE_MAP_ALL_ACCESS, 0, w.as_ptr());
            if map == 0 {
                return None;
            }
            let base = ffi::MapViewOfFile(map, ffi::FILE_MAP_ALL_ACCESS, 0, 0, TOTAL_BYTES);
            if base.is_null() {
                ffi::CloseHandle(map);
                return None;
            }
            let view = View { map, base };
            if view.header().magic != MAGIC || view.header().version != VERSION {
                return None;
            }
            Some(Self { view, last_seen: std::cell::Cell::new(0) })
        }
    }

    /// Cuántos fotogramas lleva publicados el escritor.
    pub fn written(&self) -> u64 {
        self.view.header().written.load(Ordering::Acquire)
    }

    /// True si han llegado fotogramas desde la última llamada. Así se distingue
    /// "no suena nada" (el publicador sigue mandando silencio, el contador sube)
    /// de "no hay publicador" (el contador está parado).
    pub fn advancing(&self) -> bool {
        let now = self.written();
        let moved = now != self.last_seen.get();
        self.last_seen.set(now);
        moved
    }

    /// Copia los últimos `out.len() / CHANNELS` fotogramas. Devuelve false si el
    /// publicador aún no ha producido tantos.
    pub fn latest(&self, out: &mut [i16]) -> bool {
        let want = out.len() / CHANNELS;
        if want == 0 || want > CAPACITY_FRAMES {
            return false;
        }
        let end = self.written();
        if end < want as u64 {
            return false;
        }
        // Señal de vida para el publicador. Relaxed basta: nadie sincroniza
        // datos con esto, sólo se mira si se mueve.
        self.view.header().reads.fetch_add(1, Ordering::Relaxed);
        let start = (end - want as u64) as usize;
        let data = self.view.data();
        unsafe {
            for i in 0..want {
                let slot = ((start + i) % CAPACITY_FRAMES) * CHANNELS;
                out[i * CHANNELS] = *data.add(slot);
                out[i * CHANNELS + 1] = *data.add(slot + 1);
            }
        }
        true
    }
}
