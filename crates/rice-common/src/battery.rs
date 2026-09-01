//! Bateria de los auriculares inalambricos, por dos caminos distintos porque
//! ninguno de los dos fabricantes usa el estandar.
//!
//! Windows no sabe la bateria de ninguno de los dos: se reviso propiedad por
//! propiedad en el arbol de dispositivos y no hay ninguna. Ni el HyperX (que va
//! por su dongle de 2,4 GHz, no por Bluetooth) ni los AirPods (que no
//! implementan el servicio GATT de bateria) la publican donde el sistema mire.
//!
//! **HyperX Cloud II Wireless** -- HID contra la coleccion "vendor-defined" del
//! dongle. Protocolo del driver de HeadsetControl para la revision Kingston
//! (`lib/devices/hyperx_cloud_2_wireless_kingston.hpp`). Fiable y sincrono: se
//! pregunta y contesta en ~100 ms.
//!
//! **AirPods** -- se escuchan sus anuncios BLE. Apple mete el estado en los
//! datos de fabricante (company id 0x004C) como "proximity pairing message",
//! tipo 0x07. El camino bueno seria el protocolo propietario sobre L2CAP, que
//! es lo que hace librepods en Linux, pero **Windows no expone sockets L2CAP a
//! espacio de usuario** (Winsock solo ofrece RFCOMM), asi que queda descartado.
//!
//! Consecuencia honesta de ir por el anuncio: los AirPods emiten el 0x07 al
//! abrir el estuche o en emparejamiento, no de forma continua. Por eso la
//! lectura se cachea CON su antiguedad, y la interfaz puede decir "hace rato"
//! en vez de inventar un numero.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[cfg(windows)]
use windows::core::PCWSTR;
#[cfg(windows)]
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0};
#[cfg(windows)]
use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Clase {
    HyperX,
    AirPods,
}

impl Clase {
    /// Alias corto con el que se muestran. El nombre que les pone Windows no
    /// cabe ni ayuda: "AirPods Pro de Carlos - Find My Hands-Free".
    pub fn alias(self) -> &'static str {
        match self {
            Clase::HyperX => "hyperx cloud",
            Clase::AirPods => "airpods",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Bateria {
    pub clase: Clase,
    /// Nivel que representa al dispositivo entero: el del casco en el HyperX,
    /// el del auricular mas bajo en los AirPods -- que es el que te deja tirado
    /// primero, y por tanto el unico que sirve como numero unico.
    pub nivel: Option<u8>,
    pub cargando: bool,
    /// Desglose para el panel: ("izquierdo", 80), ("estuche", 60)...
    pub partes: Vec<(String, u8)>,
    /// Salud: capacidad restante frente a la de fabrica. Ninguno de los dos
    /// dispositivos la publica hoy (ver `SALUD_POR_QUE`); el hueco queda hecho
    /// para cuando aparezca una fuente.
    pub salud: Option<u8>,
    /// Antiguedad de la lectura. Cero en el HyperX, que se pregunta en el
    /// momento; en los AirPods puede ser de minutos.
    pub edad: Duration,
}

/// Por que `salud` viene vacia, para que lo diga la interfaz en vez de dejar un
/// hueco mudo que parece un fallo.
pub const SALUD_POR_QUE: &str =
    "ninguno de los dos publica la salud: el HyperX solo da nivel y carga, y Apple la reserva a iOS";

impl Bateria {
    pub fn alias(&self) -> &'static str {
        self.clase.alias()
    }
}

// ---------------------------------------------------------------- HyperX ---

const HYPERX_VID: u16 = 0x0951;
const HYPERX_PID: u16 = 0x1718;
const CMD_NIVEL: u8 = 0x02;
const CMD_CARGANDO: u8 = 0x03;

/// Cabecera fija de todo comando; el byte 15 lleva el comando y el 16 su
/// argumento. Sacada del driver de HeadsetControl, no deducida.
const CABECERA: [u8; 15] = [
    0x06, 0x00, 0x02, 0x00, 0x9A, 0x00, 0x00, 0x68, 0x4A, 0x8E, 0x0A, 0x00, 0x00, 0x00, 0xBB,
];

/// Turno exclusivo para hablar con el dongle, entre procesos.
///
/// Hay DOS glaze-bar corriendo (una por monitor) y las dos preguntan. Un
/// dialogo HID es escribir y despues leer, asi que si se cruzan, cada una se
/// lleva la respuesta de la otra. La cabecera lo detecta y devuelve None, pero
/// eso se ve como un numero que parpadea a "--" sin motivo.
///
/// `Local\\` y no `Global\\`: es cosa de esta sesion, y `Global\\` exigiria
/// privilegios que la barra no tiene por que pedir.
struct TurnoHid(HANDLE);

impl TurnoHid {
    fn tomar() -> Option<Self> {
        unsafe {
            let nombre: Vec<u16> = "Local\\rice-hid-hyperx".encode_utf16().chain(Some(0)).collect();
            let h = CreateMutexW(None, false, PCWSTR(nombre.as_ptr())).ok()?;
            // 3 s de espera: un dialogo completo son ~400 ms, asi que si no hay
            // turno en ese plazo es que algo se quedo colgado y es mejor
            // rendirse que bloquear el hilo de la barra.
            match WaitForSingleObject(h, 3000) {
                WAIT_OBJECT_0 | WAIT_ABANDONED => Some(TurnoHid(h)),
                _ => {
                    let _ = CloseHandle(h);
                    None
                }
            }
        }
    }
}

impl Drop for TurnoHid {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}

fn hyperx_consulta(dev: &hidapi::HidDevice, comando: u8) -> Option<[u8; 64]> {
    // El dongle quiere un input report antes de que se le escriba. Puede fallar
    // y da igual; sin el intento, la primera consulta tras enchufarlo no
    // contesta.
    let mut preparar = [0u8; 64];
    preparar[0] = 0x06;
    let _ = dev.get_feature_report(&mut preparar);

    let mut peticion = [0u8; 62];
    peticion[..CABECERA.len()].copy_from_slice(&CABECERA);
    peticion[15] = comando;
    dev.write(&peticion).ok()?;
    std::thread::sleep(Duration::from_millis(100));

    let mut resp = [0u8; 64];
    let n = dev.read_timeout(&mut resp, 1000).ok()?;
    // Respuesta valida: [0x0B, _, 0xBB, comando, ...]. Sin comprobarlo se
    // tomaria por bateria cualquier reporte de teclas multimedia que pase.
    if n < 8 || resp[0] != 0x0B || resp[2] != 0xBB || resp[3] != comando {
        return None;
    }
    Some(resp)
}

/// Bateria del HyperX, o `None` si el casco esta apagado o el dongle fuera.
pub fn hyperx() -> Option<Bateria> {
    let api = hidapi::HidApi::new().ok()?;
    // La coleccion util es la "vendor-defined" (usage page >= 0xFF00). Las otras
    // del mismo dongle son control de consumidor y no responden.
    let ruta = api
        .device_list()
        .find(|d| {
            d.vendor_id() == HYPERX_VID && d.product_id() == HYPERX_PID && d.usage_page() >= 0xFF00
        })?
        .path()
        .to_owned();
    let dev = api.open_path(&ruta).ok()?;

    // El turno cubre los dos dialogos: partirlo dejaria que la otra barra se
    // colara entre el nivel y el estado de carga.
    let _turno = TurnoHid::tomar()?;
    let nivel = hyperx_consulta(&dev, CMD_NIVEL)?[7].min(100);
    let cargando = hyperx_consulta(&dev, CMD_CARGANDO)
        .map(|r| r[4] != 0)
        .unwrap_or(false);

    Some(Bateria {
        clase: Clase::HyperX,
        nivel: Some(nivel),
        cargando,
        partes: Vec::new(),
        salud: None,
        edad: Duration::ZERO,
    })
}

// --------------------------------------------------------------- AirPods ---

struct CacheAirPods {
    bateria: Bateria,
    visto: Instant,
    /// Ultimo anuncio en crudo. Se guarda para poder corregir los
    /// desplazamientos contra un paquete real: el formato es ingenieria inversa
    /// de terceros, no una especificacion publicada por Apple.
    crudo: String,
}

static AIRPODS: OnceLock<Mutex<Option<CacheAirPods>>> = OnceLock::new();
static ESCUCHA: OnceLock<()> = OnceLock::new();

fn cache() -> &'static Mutex<Option<CacheAirPods>> {
    AIRPODS.get_or_init(|| Mutex::new(None))
}

/// Un nibble del anuncio: 0..10 son decenas de por ciento; 0x0F es "no se sabe"
/// (auricular guardado en el estuche, o fuera de alcance).
fn nibble_a_pct(v: u8) -> Option<u8> {
    match v {
        0..=10 => Some(v * 10),
        _ => None,
    }
}

/// Traduce un "proximity pairing message" (tipo 0x07) a niveles.
///
/// Distribucion, segun la ingenieria inversa publica (librepods, furiousMAC):
///   [0]=0x07 tipo   [1]=longitud   [2]=prefijo   [3..5]=modelo
///   [5]=estado (el bit 0x20 dice cual auricular es el primario)
///   [6]=nibble alto y nibble bajo con los dos auriculares
///   [7]=nibble alto: estuche; nibble bajo: bits de carga
fn parsear_anuncio(b: &[u8]) -> Option<Bateria> {
    if b.len() < 8 || b[0] != 0x07 {
        return None;
    }
    let invertido = b[5] & 0x20 != 0;
    let (mut uno, mut otro) = (b[6] >> 4, b[6] & 0x0F);
    if invertido {
        std::mem::swap(&mut uno, &mut otro);
    }
    let izq = nibble_a_pct(uno);
    let der = nibble_a_pct(otro);
    let estuche = nibble_a_pct(b[7] >> 4);
    let carga = b[7] & 0x0F;

    let mut partes = Vec::new();
    for (nombre, v) in [("izquierdo", izq), ("derecho", der), ("estuche", estuche)] {
        if let Some(v) = v {
            partes.push((nombre.to_string(), v));
        }
    }

    Some(Bateria {
        clase: Clase::AirPods,
        // El que manda es el mas bajo de los dos auriculares. El estuche no
        // cuenta: tener el estuche lleno no evita que se te corte la musica.
        nivel: [izq, der].into_iter().flatten().min(),
        cargando: carga & 0b0011 != 0,
        partes,
        salud: None,
        edad: Duration::ZERO,
    })
}

/// Arranca el oyente BLE una sola vez. Es pasivo: no conecta con nada, solo
/// mira los anuncios que los AirPods sueltan al aire.
#[cfg(windows)]
pub fn iniciar_escucha_airpods() {
    if ESCUCHA.set(()).is_err() {
        return; // ya estaba escuchando
    }
    std::thread::spawn(|| {
        use windows::Devices::Bluetooth::Advertisement::*;
        use windows::Foundation::TypedEventHandler;
        use windows::Storage::Streams::DataReader;

        let Ok(w) = BluetoothLEAdvertisementWatcher::new() else {
            return;
        };
        // Escaneo activo: sin el, Windows no pide el scan response y varios
        // anuncios llegan recortados.
        let _ = w.SetScanningMode(BluetoothLEScanningMode::Active);
        let _ = w.Received(&TypedEventHandler::new(
            |_, args: &Option<BluetoothLEAdvertisementReceivedEventArgs>| {
                let Some(args) = args.as_ref() else {
                    return Ok(());
                };
                for md in args.Advertisement()?.ManufacturerData()? {
                    if md.CompanyId()? != 0x004C {
                        continue;
                    }
                    let buf = md.Data()?;
                    let mut bytes = vec![0u8; buf.Length()? as usize];
                    DataReader::FromBuffer(&buf)?.ReadBytes(&mut bytes)?;
                    if let Some(bateria) = parsear_anuncio(&bytes) {
                        let crudo = bytes.iter().map(|x| format!("{x:02X}")).collect();
                        *cache().lock().unwrap() = Some(CacheAirPods {
                            bateria,
                            visto: Instant::now(),
                            crudo,
                        });
                    }
                }
                Ok(())
            },
        ));
        if w.Start().is_err() {
            return;
        }
        // El watcher deja de escuchar cuando se suelta, asi que el hilo se
        // queda dormido manteniendolo vivo. No es un bucle de sondeo: los
        // anuncios entran por el callback cuando el aire los trae.
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    });
}

#[cfg(not(windows))]
pub fn iniciar_escucha_airpods() {}

/// Ultimo estado conocido de los AirPods, con su antiguedad puesta al dia.
pub fn airpods() -> Option<Bateria> {
    let guard = cache().lock().unwrap();
    let c = guard.as_ref()?;
    let mut b = c.bateria.clone();
    b.edad = c.visto.elapsed();
    Some(b)
}

/// El ultimo anuncio en crudo, para poder corregir los desplazamientos contra
/// un paquete de verdad.
pub fn airpods_crudo() -> Option<String> {
    cache().lock().unwrap().as_ref().map(|c| c.crudo.clone())
}

// ----------------------------------------------------------------- ambos ---

/// Los dos, para el panel de dispositivos. El que no este disponible no sale.
pub fn todas() -> Vec<Bateria> {
    let mut v = Vec::new();
    if let Some(b) = hyperx() {
        v.push(b);
    }
    if let Some(b) = airpods() {
        v.push(b);
    }
    v
}

/// La del dispositivo que esta sonando ahora, que es la que va en la barra.
/// Se decide por la salida predeterminada, no por cual tiene mas bateria.
#[cfg(feature = "audio")]
pub fn en_uso() -> Option<Bateria> {
    let salida = crate::audio::current_output_name()?.to_lowercase();
    if salida.contains("airpods") {
        airpods()
    } else if salida.contains("hyperx") || salida.contains("cloud") {
        hyperx()
    } else {
        None
    }
}

#[cfg(test)]
mod pruebas {
    use super::*;

    /// Nibble 0x0F es "desconocido", no 150%.
    #[test]
    fn nibble_desconocido() {
        assert_eq!(nibble_a_pct(0x0F), None);
        assert_eq!(nibble_a_pct(10), Some(100));
        assert_eq!(nibble_a_pct(0), Some(0));
    }

    /// El nivel que representa al dispositivo es el auricular mas bajo, y el
    /// estuche no entra en esa cuenta aunque este lleno.
    #[test]
    fn manda_el_auricular_mas_bajo() {
        // tipo, long, prefijo, modelo x2, estado, nibbles(8,3), estuche(10)+carga
        let b = [0x07, 0x19, 0x01, 0x0E, 0x20, 0x00, 0x83, 0xA0];
        let bat = parsear_anuncio(&b).expect("deberia parsear");
        assert_eq!(bat.nivel, Some(30));
        assert_eq!(bat.partes, vec![
            ("izquierdo".to_string(), 80),
            ("derecho".to_string(), 30),
            ("estuche".to_string(), 100),
        ]);
    }

    /// El bit 0x20 del estado intercambia cual auricular es cual.
    #[test]
    fn el_bit_de_primario_invierte() {
        let b = [0x07, 0x19, 0x01, 0x0E, 0x20, 0x20, 0x83, 0xA0];
        let bat = parsear_anuncio(&b).expect("deberia parsear");
        assert_eq!(bat.partes[0], ("izquierdo".to_string(), 30));
        assert_eq!(bat.partes[1], ("derecho".to_string(), 80));
        assert_eq!(bat.nivel, Some(30), "el minimo no cambia al invertir");
    }

    /// Un anuncio que no es del tipo 0x07 no se toca.
    #[test]
    fn ignora_otros_tipos() {
        assert!(parsear_anuncio(&[0x10, 0x06, 0x0D, 0x1D, 0x7E, 0xE5, 0x99, 0x68]).is_none());
        assert!(parsear_anuncio(&[0x07]).is_none(), "demasiado corto");
    }

    /// Un auricular guardado en el estuche sale como desconocido, no como 0%.
    #[test]
    fn auricular_guardado_no_es_cero() {
        let b = [0x07, 0x19, 0x01, 0x0E, 0x20, 0x00, 0xF5, 0x90];
        let bat = parsear_anuncio(&b).expect("deberia parsear");
        assert_eq!(bat.nivel, Some(50), "solo cuenta el que si reporta");
        assert_eq!(bat.partes.len(), 2, "izquierdo desconocido no aparece");
    }
}
