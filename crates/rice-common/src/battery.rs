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
    /// Tension de la celda en milivoltios, cuando el dispositivo la publica.
    /// El HyperX si: son los bytes 5-6 de la respuesta de bateria, y se mueven
    /// unos pocos mV entre lecturas como corresponde a un ADC de verdad.
    pub voltaje_mv: Option<u16>,
    /// Puntos de porcentaje por hora, con signo: positivo cargando, negativo
    /// gastando. Se DERIVA de la pendiente del porcentaje en el tiempo, porque
    /// el dispositivo no da corriente (ver `POTENCIA_POR_QUE`).
    pub ritmo_pct_h: Option<f32>,
}

/// Por que no hay milliamperios ni vatios, para que la interfaz lo diga en vez
/// de callarse.
pub const POTENCIA_POR_QUE: &str =
    "el casco publica carga y tension, pero no corriente; sin amperios no hay vatios que calcular";

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
    // Drenar el input report pendiente ANTES de escribir. Y tiene que ser
    // `get_input_report` (HidD_GetInputReport), no `get_feature_report`
    // (HidD_GetFeature): son llamadas distintas y esta coleccion solo entiende
    // la primera.
    //
    // Con la equivocada, `get_feature_report` devuelve "Funcion incorrecta" y
    // el `write` que va detras muere con ERROR_GEN_FAILURE (0x1F) -- SIEMPRE,
    // aunque el casco este encendido y sonando. Medido: 3 de 3 fallos con
    // feature, 3 de 3 lecturas correctas con input.
    let mut preparar = [0u8; 64];
    preparar[0] = 0x06;
    let _ = dev.get_input_report(&mut preparar);

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

/// En que estado esta el HyperX. La diferencia importa: sin dongle no hay
/// nada que enseñar, pero un fallo suelto de lectura no debe borrar de la
/// pantalla un dato que sigue siendo bueno.
pub enum EstadoHyperx {
    /// Ni dongle ni coleccion: no hay dispositivo del que hablar.
    SinDongle,
    /// El dongle esta, pero no contesto. Casco apagado, o una lectura perdida.
    NoResponde,
    Ok(Bateria),
}

/// Bateria del HyperX, o `None` si el casco esta apagado o el dongle fuera.
///
/// Hace E/S: enumera los HID del sistema y hace dos dialogos con el dongle.
/// No se llama desde nada que pinte -- para eso esta `todas()`, que lee la
/// cache. Llamarla al construir la lista de dispositivos metia medio segundo
/// de espera en cada clic del panel.
pub fn hyperx() -> Option<Bateria> {
    match hyperx_estado() {
        EstadoHyperx::Ok(b) => Some(b),
        _ => None,
    }
}

/// Igual que `hyperx()` pero diciendo POR QUE no hay lectura.
pub fn hyperx_estado() -> EstadoHyperx {
    let Ok(api) = hidapi::HidApi::new() else {
        return EstadoHyperx::NoResponde;
    };
    // La coleccion util es la "vendor-defined" (usage page >= 0xFF00). Las otras
    // del mismo dongle son control de consumidor y no responden.
    let Some(info) = api.device_list().find(|d| {
        d.vendor_id() == HYPERX_VID && d.product_id() == HYPERX_PID && d.usage_page() >= 0xFF00
    }) else {
        return EstadoHyperx::SinDongle;
    };
    let Ok(dev) = api.open_path(&info.path().to_owned()) else {
        return EstadoHyperx::NoResponde;
    };

    // El turno cubre los dos dialogos: partirlo dejaria que la otra barra se
    // colara entre el nivel y el estado de carga.
    let Some(_turno) = TurnoHid::tomar() else {
        return EstadoHyperx::NoResponde;
    };
    let Some(resp) = hyperx_consulta(&dev, CMD_NIVEL) else {
        return EstadoHyperx::NoResponde;
    };
    let cargando = hyperx_consulta(&dev, CMD_CARGANDO)
        .map(|r| r[4] != 0)
        .unwrap_or(false);

    EstadoHyperx::Ok(Bateria {
        clase: Clase::HyperX,
        nivel: Some(resp[7].min(100)),
        cargando,
        partes: Vec::new(),
        salud: None,
        edad: Duration::ZERO,
        // Big-endian. Medido en reposo al 52%: 3746-3751 mV, que es justo donde
        // esta una celda de litio a media carga.
        voltaje_mv: Some(u16::from_be_bytes([resp[5], resp[6]])),
        ritmo_pct_h: None, // lo pone la cache, que es quien tiene historial
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

/// Tamaño del "proximity pairing message" bueno: 27 bytes, de los que los 16
/// ultimos van cifrados. Otras longitudes son otros mensajes de Apple, no este.
const LARGO_PROXIMITY: usize = 27;

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
///   [7]=nibble ALTO: banderas de carga; nibble BAJO: estuche
///
/// El byte 7 estuvo al reves hasta que se capturo un anuncio de verdad: valia
/// 0xBA, que leido mal daba "estuche 11" -- un nivel que no existe, porque el
/// maximo es 10. Con el orden bueno son banderas 0xB y estuche 0xA = 100%.
fn parsear_anuncio(b: &[u8]) -> Option<Bateria> {
    // Tipo 0x07 NO basta. Capturado en el aire un 0x07 ajeno de 19 bytes
    // (07 11 06 A5 30 ...) que con solo esa comprobacion se colaba: el parser
    // lo leia como "izquierdo 100%, derecho 50%, estuche 0%" y lo enseñaba
    // como si fueran tus AirPods. Un numero inventado desde el aparato de otra
    // persona es peor que no enseñar nada.
    //
    // El mensaje bueno mide 27 bytes y declara 0x19 (25) en su byte de
    // longitud. Se exigen las dos cosas: un tamaño distinto significa otra
    // variante del mensaje, cuyos desplazamientos no conocemos.
    if b.len() < LARGO_PROXIMITY || b[0] != 0x07 || b[1] != 0x19 {
        return None;
    }
    let invertido = b[5] & 0x20 != 0;
    let (mut uno, mut otro) = (b[6] >> 4, b[6] & 0x0F);
    if invertido {
        std::mem::swap(&mut uno, &mut otro);
    }
    let izq = nibble_a_pct(uno);
    let der = nibble_a_pct(otro);
    let carga = b[7] >> 4;
    let estuche = nibble_a_pct(b[7] & 0x0F);

    let mut partes = Vec::new();
    for (nombre, v) in [("izquierdo", izq), ("derecho", der), ("estuche", estuche)] {
        if let Some(v) = v {
            partes.push((nombre.to_string(), v));
        }
    }

    Some(Bateria {
        // Los AirPods no publican tension en el anuncio; solo niveles y carga.
        voltaje_mv: None,
        ritmo_pct_h: None,
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

/// ¿Estan los AirPods conectados a este PC ahora mismo?
///
/// Oir su anuncio BLE NO significa que lo esten: el estuche se anuncia igual
/// con los auriculares guardados dentro, que es justo como se capturo el
/// primer paquete bueno. Para la barra lo que vale es que Windows los tenga
/// conectados como dispositivo de audio.
static AIRPODS_CONECTADOS: OnceLock<Mutex<(bool, Option<Instant>)>> = OnceLock::new();

fn bandera_conectados() -> &'static Mutex<(bool, Option<Instant>)> {
    AIRPODS_CONECTADOS.get_or_init(|| Mutex::new((false, None)))
}

/// Cada cuanto se vuelve a preguntar al Bluetooth.
///
/// Enumerarlo abre un filtro KS por endpoint, asi que no es una consulta
/// barata. `refrescar()` lo hacia en CADA pasada, y esa pasada baja a una cada
/// diez segundos cuando el casco no contesta -- justo cuando menos falta hace,
/// porque el casco apagado no dice nada de si los AirPods estan puestos.
/// Conectar unos auriculares no es algo que pase varias veces por minuto.
const CADA_CUANTO_BT: Duration = Duration::from_secs(30);

fn mirar_si_conectados() -> bool {
    crate::bluetooth::devices()
        .iter()
        .any(|d| d.connected && d.name.to_lowercase().contains("airpods"))
}

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

/// Ultima lectura del HyperX. La E/S la hace un solo hilo con `refrescar()` y
/// el resto del programa lee de aqui.
///
/// La separacion no es adorno: `hyperx()` enumera todos los HID del sistema y
/// habla con el dongle, y eso colgaba medio segundo el hilo que arma la lista
/// de dispositivos en cada clic del panel.
/// Cuantas lecturas fallidas seguidas hacen falta para dar el casco por
/// apagado y quitarlo de la barra.
///
/// Una sola no vale: un dialogo HID se pierde de vez en cuando, y borrar el
/// numero por eso es lo que hacia que parpadeara. Tres seguidas, con el sondeo
/// cada minuto, son unos tres minutos de silencio -- para entonces el casco
/// esta apagado de verdad, no distraido.
const FALLOS_PARA_DARLO_POR_IDO: u8 = 3;

#[derive(Default)]
struct CacheHyperx {
    ultima: Option<(Bateria, Instant)>,
    fallos: u8,
    /// (cuando, porcentaje) de las ultimas lecturas, para sacar el ritmo. El
    /// dispositivo no da corriente, asi que la velocidad de carga solo puede
    /// salir de ver como se mueve el porcentaje.
    historial: Vec<(Instant, u8)>,
}

static ULTIMO_HYPERX: OnceLock<Mutex<CacheHyperx>> = OnceLock::new();

fn cache_hyperx() -> &'static Mutex<CacheHyperx> {
    ULTIMO_HYPERX.get_or_init(|| Mutex::new(CacheHyperx::default()))
}

/// Puntos por hora entre el primer y el ultimo punto del historial.
///
/// Hace falta una ventana de al menos 12 minutos y un cambio real de al menos
/// un punto: con menos, el ruido de redondeo del propio porcentaje daria
/// ritmos inventados de cientos de puntos por hora.
fn ritmo(historial: &[(Instant, u8)]) -> Option<f32> {
    let (t0, p0) = *historial.first()?;
    let (t1, p1) = *historial.last()?;
    let horas = t1.duration_since(t0).as_secs_f32() / 3600.0;
    if horas < 0.2 || p0 == p1 {
        return None;
    }
    Some((p1 as f32 - p0 as f32) / horas)
}

/// Vuelve a preguntar por HID y guarda el resultado. Es lo unico que hace E/S.
///
/// Una lectura fallida NO borra la anterior. Antes si, y por eso el numero
/// saltaba a "--" y volvia: un dialogo HID perdido -- que pasa -- borraba un
/// dato que seguia siendo bueno. Un 53% de hace dos minutos es informacion
/// util; un "--" no es ninguna.
///
/// Lo unico que la borra es que desaparezca el dongle, porque entonces no hay
/// dispositivo del que hablar.
/// Solo el estado de carga, sin tocar el nivel.
///
/// Es UN dialogo con el dongle en vez de dos, y es el unico dato que cambia de
/// golpe: enchufas el cable y ya esta. El porcentaje, en cambio, se mueve en
/// minutos. Separarlos permite mirar la carga a menudo sin pagar el coste de
/// releer todo.
///
/// No toca el historial ni la marca de tiempo de la lectura: esto no es una
/// lectura completa, solo la mitad que corre prisa.
pub fn refrescar_carga() -> bool {
    let Ok(api) = hidapi::HidApi::new() else { return false };
    let Some(info) = api.device_list().find(|d| {
        d.vendor_id() == HYPERX_VID && d.product_id() == HYPERX_PID && d.usage_page() >= 0xFF00
    }) else {
        return false;
    };
    let Ok(dev) = api.open_path(&info.path().to_owned()) else { return false };
    let Some(_turno) = TurnoHid::tomar() else { return false };
    let Some(resp) = hyperx_consulta(&dev, CMD_CARGANDO) else { return false };
    let cargando = resp[4] != 0;

    let mut c = cache_hyperx().lock().unwrap();
    let Some((b, _)) = c.ultima.as_mut() else { return false };
    if b.cargando != cargando {
        // Enchufar o desenchufar corta la racha: promediar una descarga con
        // una carga no describe ninguna de las dos.
        b.cargando = cargando;
        c.historial.clear();
    }
    true
}

/// Devuelve `true` si consiguio lectura nueva. Quien llama lo usa para
/// reintentar antes de lo normal: un fallo suele significar que acabas de
/// apagar el casco, y esperar al siguiente minuto deja el numero en pantalla
/// mucho despues de que el casco ya no este.
pub fn refrescar() -> bool {
    {
        let g = bandera_conectados().lock().unwrap();
        let toca = g.1.map(|t| t.elapsed() >= CADA_CUANTO_BT).unwrap_or(true);
        if toca {
            // El candado se suelta para la consulta y se vuelve a tomar: tenerlo
            // cogido durante una enumeracion de Bluetooth bloquearia a quien
            // solo quiere pintar.
            drop(g);
            let visto = mirar_si_conectados();
            let mut g = bandera_conectados().lock().unwrap();
            *g = (visto, Some(Instant::now()));
        }
    }
    let estado = hyperx_estado();
    let mut c = cache_hyperx().lock().unwrap();
    match estado {
        EstadoHyperx::Ok(b) => {
            let ahora = Instant::now();
            if let Some(pct) = b.nivel {
                // El historial se reinicia al enchufar o desenchufar: mezclar
                // una racha de descarga con una de carga daria un ritmo que no
                // es ninguno de los dos.
                let cambio_de_sentido = c
                    .ultima
                    .as_ref()
                    .map(|(prev, _)| prev.cargando != b.cargando)
                    .unwrap_or(false);
                if cambio_de_sentido {
                    c.historial.clear();
                }
                c.historial.push((ahora, pct));
                // Hora y media de ventana es de sobra para un ritmo estable, y
                // evita que el historial crezca sin fin.
                let corte = ahora - Duration::from_secs(5400);
                c.historial.retain(|(t, _)| *t >= corte);
            }
            c.ultima = Some((b, ahora));
            c.fallos = 0;
        }
        // Sin dongle no hay dispositivo del que hablar: fuera de la barra ya.
        EstadoHyperx::SinDongle => {
            *c = CacheHyperx::default();
            return false;
        }
        EstadoHyperx::NoResponde => {
            c.fallos = c.fallos.saturating_add(1);
            if c.fallos >= FALLOS_PARA_DARLO_POR_IDO {
                *c = CacheHyperx::default();
            }
            return false;
        }
    }
    true
}

/// Lo ultimo que se leyo del HyperX, con la edad y el ritmo puestos al dia.
fn hyperx_cacheado() -> Option<Bateria> {
    let c = cache_hyperx().lock().unwrap();
    let (b, cuando) = c.ultima.as_ref()?;
    let mut b = b.clone();
    b.edad = cuando.elapsed();
    b.ritmo_pct_h = ritmo(&c.historial);
    Some(b)
}

/// Los dispositivos que estan CONECTADOS ahora, que es lo que va a la barra.
///
/// El criterio es distinto para cada uno, porque "conectado" significa cosas
/// distintas: el HyperX esta conectado si su dongle contesta -- no hay otra
/// forma de saberlo, no va por Bluetooth. Los AirPods, si Windows los tiene
/// conectados como salida de audio; su anuncio BLE no basta, porque el estuche
/// se anuncia igual con los auriculares dentro.
///
/// Solo lee memoria: la E/S la hizo `refrescar()`.
pub fn conectadas() -> Vec<Bateria> {
    let mut v = Vec::new();
    if let Some(b) = hyperx_cacheado() {
        v.push(b);
    }
    if bandera_conectados().lock().unwrap().0 {
        if let Some(b) = airpods() {
            v.push(b);
        }
    }
    v
}

/// Los dos, para el panel de dispositivos. Solo lee memoria: no hace E/S, asi
/// que se puede llamar desde donde haga falta sin frenar nada.
pub fn todas() -> Vec<Bateria> {
    let mut v = Vec::new();
    if let Some(b) = hyperx_cacheado() {
        v.push(b);
    }
    if let Some(b) = airpods() {
        v.push(b);
    }
    v
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

    /// Un anuncio del tamaño bueno, con los bytes que importan puestos.
    fn anuncio(estado: u8, nibbles: u8, carga_y_estuche: u8) -> [u8; LARGO_PROXIMITY] {
        let mut b = [0u8; LARGO_PROXIMITY];
        b[0] = 0x07;
        b[1] = 0x19;
        b[2] = 0x01;
        b[3] = 0x0E; // modelo: AirPods Pro
        b[4] = 0x20;
        b[5] = estado;
        b[6] = nibbles;
        b[7] = carga_y_estuche;
        b
    }

    /// El nivel que representa al dispositivo es el auricular mas bajo, y el
    /// estuche no entra en esa cuenta aunque este lleno.
    #[test]
    fn manda_el_auricular_mas_bajo() {
        let b = anuncio(0x00, 0x83, 0x0A); // sin cargar, estuche al 100%
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
        let b = anuncio(0x20, 0x83, 0x0A);
        let bat = parsear_anuncio(&b).expect("deberia parsear");
        assert_eq!(bat.partes[0], ("izquierdo".to_string(), 30));
        assert_eq!(bat.partes[1], ("derecho".to_string(), 80));
        assert_eq!(bat.nivel, Some(30), "el minimo no cambia al invertir");
    }

    /// Helper: un historial con puntos a N minutos en el pasado.
    fn hist(puntos: &[(u64, u8)]) -> Vec<(Instant, u8)> {
        let ahora = Instant::now();
        puntos
            .iter()
            .map(|(min, pct)| (ahora - Duration::from_secs(min * 60), *pct))
            .collect()
    }

    /// Media hora subiendo 10 puntos son 20 puntos por hora.
    #[test]
    fn ritmo_cargando() {
        let r = ritmo(&hist(&[(30, 40), (0, 50)])).expect("deberia dar ritmo");
        assert!((r - 20.0).abs() < 0.5, "esperaba ~+20, fue {r}");
    }

    /// Gastando, el signo es negativo.
    #[test]
    fn ritmo_descargando() {
        let r = ritmo(&hist(&[(60, 80), (0, 74)])).expect("deberia dar ritmo");
        assert!((r + 6.0).abs() < 0.5, "esperaba ~-6, fue {r}");
    }

    /// Una ventana corta no da ritmo: el redondeo del propio porcentaje
    /// inventaria cientos de puntos por hora.
    #[test]
    fn ventana_corta_no_da_ritmo() {
        assert_eq!(ritmo(&hist(&[(5, 50), (0, 51)])), None);
    }

    /// Sin cambio de porcentaje no hay ritmo que reportar.
    #[test]
    fn sin_cambio_no_hay_ritmo() {
        assert_eq!(ritmo(&hist(&[(60, 50), (0, 50)])), None);
    }

    /// Un anuncio que no es del tipo 0x07 no se toca.
    #[test]
    fn ignora_otros_tipos() {
        assert!(parsear_anuncio(&[0x10, 0x06, 0x0D, 0x1D, 0x7E, 0xE5, 0x99, 0x68]).is_none());
        assert!(parsear_anuncio(&[0x07]).is_none(), "demasiado corto");
    }

    /// El anuncio REAL de los AirPods del dueño, capturado al abrir el estuche.
    /// Es la referencia: los desplazamientos se comprueban contra esto y no
    /// contra documentacion de terceros.
    #[test]
    fn anuncio_real_capturado() {
        let real = [
            0x07, 0x19, 0x01, 0x27, 0x20, 0x55, 0xAA, 0xBA, 0x11, 0x00, 0x04, 0x7E, 0x5B, 0x46,
            0x2F, 0xDA, 0x29, 0xC7, 0x3F, 0x90, 0x6F, 0x09, 0xE0, 0xE8, 0x67, 0x81, 0x95,
        ];
        let bat = parsear_anuncio(&real).expect("el anuncio real debe parsear");
        // 0x6=0xAA: los dos auriculares al 100%.
        assert_eq!(bat.nivel, Some(100));
        assert_eq!(bat.partes[0], ("izquierdo".to_string(), 100));
        assert_eq!(bat.partes[1], ("derecho".to_string(), 100));
        // 0x7=0xBA: banderas 0xB, estuche 0xA = 100%. Leido al reves daba 11,
        // que no es un nivel valido.
        assert_eq!(bat.partes[2], ("estuche".to_string(), 100));
        assert!(bat.cargando, "las banderas 0xB dicen que estaban cargando");
    }

    /// Regresion: un 0x07 ajeno, capturado de verdad en el aire, de otra
    /// variante y otro tamaño. Antes se colaba y se mostraba como si fueran
    /// tus AirPods al 50%.
    #[test]
    fn rechaza_el_0x07_ajeno_capturado() {
        let ajeno = [
            0x07, 0x11, 0x06, 0xA5, 0x30, 0x13, 0xA5, 0x0C, 0x17, 0xC9, 0x7D, 0xA5, 0x90, 0xE6,
            0xD7, 0x4B, 0x92, 0x4A, 0x65,
        ];
        assert!(
            parsear_anuncio(&ajeno).is_none(),
            "un 0x07 de otra variante no debe leerse como bateria"
        );
    }

    /// Un auricular guardado en el estuche sale como desconocido, no como 0%.
    #[test]
    fn auricular_guardado_no_es_cero() {
        let b = anuncio(0x00, 0xF5, 0x09);
        let bat = parsear_anuncio(&b).expect("deberia parsear");
        assert_eq!(bat.nivel, Some(50), "solo cuenta el que si reporta");
        assert_eq!(bat.partes.len(), 2, "izquierdo desconocido no aparece");
    }
}
