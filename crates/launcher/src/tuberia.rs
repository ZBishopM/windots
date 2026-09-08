//! Una puerta para preguntarle al índice de archivos desde fuera del proceso.
//!
//! El índice vive en memoria y se construye recorriendo todas las unidades fijas
//! -- incluida la I:, que es un disco mecánico. Un proceso de un disparo que lo
//! reconstruyera en cada búsqueda no es viable, y volcarlo a disco sería una
//! copia más que mantener al día y que envejece entre escrituras. La instancia
//! residente YA lo tiene construido y vigilado: lo único que faltaba era poder
//! preguntarle.
//!
//! Tubería con nombre y no un archivo más un evento, que es como se comunican
//! otras piezas del rice: aquí hace falta ida Y VUELTA, y con archivos eso son
//! dos rutas, dos eventos y un estado a medias que limpiar si el que pregunta
//! muere a mitad. La tubería lo da hecho y se cierra sola.
//!
//! Protocolo, deliberadamente tonto -- una línea va, N vuelven:
//!
//! ```text
//! ->  <texto a buscar>\n
//! <-  <ruta>\n  por cada resultado, hasta MAX; vacío si no hay ninguno
//! ```

use std::time::{Duration, Instant};

use crate::files::FileIndex;

pub const TUBERIA: &str = r"\\.\pipe\rice-launcher-archivos";

/// Cuántos resultados se devuelven. Quien pregunta es un modelo que tiene que
/// elegir uno, no una lista que alguien recorre con la vista.
const MAX: usize = 12;

/// Tope de espera de una búsqueda. El índice contesta en milisegundos cuando ya
/// terminó de recorrer; esto es para el caso de preguntar recién arrancado, con
/// el recorrido todavía en marcha.
const ESPERA: Duration = Duration::from_secs(5);

// Declaradas a mano en vez de tirar del crate `windows`, igual que ShellExecuteW
// en main.rs: son seis funciones con firmas que caben aquí, y así no hay que
// añadir otra familia de features al Cargo.toml para esto.
#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn CreateNamedPipeW(
        name: *const u16,
        open_mode: u32,
        pipe_mode: u32,
        max_instances: u32,
        out_buf: u32,
        in_buf: u32,
        timeout: u32,
        sa: *const u8,
    ) -> isize;
    fn ConnectNamedPipe(h: isize, ov: *mut u8) -> i32;
    fn DisconnectNamedPipe(h: isize) -> i32;
    fn ReadFile(h: isize, buf: *mut u8, n: u32, read: *mut u32, ov: *mut u8) -> i32;
    fn WriteFile(h: isize, buf: *const u8, n: u32, written: *mut u32, ov: *mut u8) -> i32;
    fn FlushFileBuffers(h: isize) -> i32;
    fn CloseHandle(h: isize) -> i32;
}

const PIPE_ACCESS_DUPLEX: u32 = 0x0000_0003;
const INVALID_HANDLE: isize = -1;

/// Arranca el hilo que atiende la tubería. Una instancia a la vez y de una en
/// una: las consultas son esporádicas y atenderlas en serie evita que dos se
/// pisen la generación de búsqueda.
#[cfg(windows)]
pub fn servir(idx: FileIndex) {
    let _ = std::thread::Builder::new().name("tuberia-archivos".into()).spawn(move || loop {
        let nombre = rice_common::win::wide(TUBERIA);
        let h = unsafe {
            CreateNamedPipeW(
                nombre.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                0, // byte a byte y bloqueante, que es lo que quiere un texto suelto
                1,
                64 * 1024,
                64 * 1024,
                0,
                std::ptr::null(),
            )
        };
        if h == INVALID_HANDLE {
            // Ya hay otro residente sirviendo, o el sistema dijo que no. Ninguna
            // de las dos se arregla reintentando en bucle cerrado.
            std::thread::sleep(Duration::from_secs(5));
            continue;
        }
        // ConnectNamedPipe devuelve 0 si el cliente ganó la carrera y ya estaba
        // conectado antes de esta llamada; eso NO es un fallo, es una conexión
        // buena. Distinguirlo del error de verdad exigiría GetLastError; como la
        // lectura de después falla igual en el caso malo, se sigue derecho.
        unsafe { ConnectNamedPipe(h, std::ptr::null_mut()) };
        atender(&idx, h);
        unsafe {
            DisconnectNamedPipe(h);
            CloseHandle(h);
        }
    });
}

#[cfg(not(windows))]
pub fn servir(_idx: FileIndex) {}

#[cfg(windows)]
fn atender(idx: &FileIndex, h: isize) {
    let mut buf = [0u8; 4096];
    let mut leidos = 0u32;
    let ok = unsafe { ReadFile(h, buf.as_mut_ptr(), buf.len() as u32, &mut leidos, std::ptr::null_mut()) };
    if ok == 0 || leidos == 0 {
        return;
    }
    let consulta = String::from_utf8_lossy(&buf[..leidos as usize]).trim().to_string();
    if consulta.is_empty() {
        return;
    }

    idx.search(&consulta, MAX);
    let t0 = Instant::now();
    while !idx.settled() && t0.elapsed() < ESPERA {
        std::thread::sleep(Duration::from_millis(20));
    }

    let mut respuesta = String::new();
    for hit in idx.results().iter().take(MAX) {
        respuesta.push_str(&hit.path);
        respuesta.push('\n');
    }
    let mut escritos = 0u32;
    unsafe {
        WriteFile(h, respuesta.as_ptr(), respuesta.len() as u32, &mut escritos, std::ptr::null_mut());
        // Obligatorio antes de desconectar, y no una precaución: sin esto, el
        // cierre puede descartar lo que quede en el búfer y el cliente ve una
        // tubería ROTA en vez del final del flujo. Un cliente .NET llegaba a
        // leerlo todo por ser más rápido; Node daba `read EPIPE` siempre, con
        // la respuesta ya escrita del lado de aquí.
        FlushFileBuffers(h);
    }
}
