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
//! ->  <texto a buscar>\n                 (hasta MAX resultados)
//! ->  <limite>\t<texto a buscar>\n       (hasta <limite>, tope MAX_DURO)
//! <-  <ruta>\n  por cada resultado; vacío si no hay ninguno
//! ```
//!
//! La petición tiene que ir en UNA escritura. El servidor hace un solo
//! `ReadFile`, así que un cliente que la mande a trozos -- lo que hace
//! `writeln!` con formato, por ejemplo -- solo entrega el primero.

use std::time::{Duration, Instant};

use crate::files::FileIndex;

pub const TUBERIA: &str = r"\\.\pipe\rice-launcher-archivos";

/// Cuántos resultados se devuelven si nadie pide otra cosa. Quien pregunta por
/// defecto es el modelo local, que tiene que elegir UNO, no una lista que
/// alguien recorre con la vista.
const MAX: usize = 12;

/// Tope duro aunque se pida más. El índice tiene más de un millón de entradas y
/// una consulta de una letra las empareja casi todas; devolverlas sería llenar
/// la tubería de ruido que nadie va a leer.
const MAX_DURO: usize = 2000;

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

/// El lado CLIENTE: le pregunta al residente por la misma tubería.
///
/// Se abre con `OpenOptions` como si fuera un archivo, que en Windows es
/// exactamente lo que una tubería con nombre es. El servidor es síncrono y
/// bloqueante, así que leerla como archivo es lo que le corresponde.
#[cfg(windows)]
pub fn preguntar(consulta: &str, cuantos: usize) -> Result<Vec<String>, String> {
    use std::io::{Read, Write};

    let mut t = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(TUBERIA)
        .map_err(|e| format!("no hay launcher residente al que preguntar ({e})"))?;

    // Se arma la linea entera y se manda de UNA escritura.
    //
    // `writeln!` con formato hace varias llamadas -- "200", "\t", "mp4", "\n" --
    // y en una tuberia en modo BYTE cada una viaja por su cuenta. El servidor
    // hace UN solo `ReadFile`, asi que recogia el primer trozo y trataba "200"
    // como si fuera la busqueda. Ese era el sintoma: el mismo servidor
    // contestaba bien a un cliente .NET y devolvia nada al mio.
    let peticion = format!("{cuantos}\t{consulta}\n");
    t.write_all(peticion.as_bytes()).map_err(|e| e.to_string())?;

    // Se lee a mano en vez de con `read_to_string`.
    //
    // Al terminar, el servidor hace `DisconnectNamedPipe`, y Windows corta la
    // lectura pendiente con ERROR_PIPE_NOT_CONNECTED (233) -- a veces con
    // ERROR_BROKEN_PIPE (109). Ninguno de los dos es un fallo aqui: son el
    // final de la respuesta. `read_to_string` los propaga como error y se
    // pierde todo lo ya leido, que era exactamente el sintoma: la tuberia
    // contestaba bien y el cliente devolvia cero resultados.
    let diag = std::env::var("RICE_TUBERIA_DIAG").is_ok();
    let mut datos = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        match t.read(&mut buf) {
            Ok(0) => {
                if diag {
                    eprintln!("diag: read devolvio 0 tras {} bytes", datos.len());
                }
                break;
            }
            Ok(n) => {
                if diag {
                    eprintln!("diag: +{n} bytes");
                }
                datos.extend_from_slice(&buf[..n]);
            }
            Err(e) => {
                if diag {
                    eprintln!("diag: read fallo {:?} tras {} bytes", e.raw_os_error(), datos.len());
                }
                match e.raw_os_error() {
                    Some(233) | Some(109) => break,
                    _ => return Err(e.to_string()),
                }
            }
        }
    }
    let s = String::from_utf8_lossy(&datos);
    Ok(s.lines().filter(|l| !l.trim().is_empty()).map(|l| l.to_string()).collect())
}

#[cfg(not(windows))]
pub fn preguntar(_consulta: &str, _cuantos: usize) -> Result<Vec<String>, String> {
    Err("solo Windows".into())
}

#[cfg(windows)]
fn atender(idx: &FileIndex, h: isize) {
    let mut buf = [0u8; 4096];
    let mut leidos = 0u32;
    let ok = unsafe { ReadFile(h, buf.as_mut_ptr(), buf.len() as u32, &mut leidos, std::ptr::null_mut()) };
    if ok == 0 || leidos == 0 {
        return;
    }
    let linea = String::from_utf8_lossy(&buf[..leidos as usize]).trim().to_string();
    if linea.is_empty() {
        return;
    }

    // `<limite>\t<texto>` pide un numero distinto de resultados; sin tabulador,
    // la linea entera es la consulta. Asi el cliente viejo -- el de las manos,
    // que manda solo el texto -- sigue funcionando sin tocarlo.
    let (cuantos, consulta) = match linea.split_once('\t') {
        Some((n, resto)) => match n.trim().parse::<usize>() {
            Ok(v) => (v.clamp(1, MAX_DURO), resto.trim().to_string()),
            Err(_) => (MAX, linea.clone()),
        },
        None => (MAX, linea.clone()),
    };
    if consulta.is_empty() {
        return;
    }

    idx.search(&consulta, cuantos);
    let t0 = Instant::now();
    while !idx.settled() && t0.elapsed() < ESPERA {
        std::thread::sleep(Duration::from_millis(20));
    }

    let mut respuesta = String::new();
    for hit in idx.results().iter().take(cuantos) {
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
