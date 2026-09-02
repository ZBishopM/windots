//! Medidor de consumo electrico: muestrea la potencia y la integra en el tiempo.
//!
//!     consumo              muestrea sin parar y va acumulando (lo lanza el supervisor)
//!     consumo --hoy        lo gastado hoy
//!     consumo --mes        el desglose del mes
//!
//! LO QUE ESTE NUMERO ES Y LO QUE NO
//!
//! Es una ESTIMACION, no una medida de lo que marca el contador. Los sensores
//! publican la potencia de la GPU y (con LibreHardwareMonitor elevado) la del
//! paquete de la CPU. Todo lo demas del equipo -- placa, RAM, discos,
//! ventiladores, USB, RGB -- no la publica nadie, y las perdidas de la fuente
//! tampoco. En carga eso es un anadido modesto; EN REPOSO puede ser mas de la
//! mitad del total, asi que un medidor "CPU + GPU" a secas se queda corto justo
//! donde el equipo pasa la mayor parte del dia.
//!
//! Por eso los terminos que no se pueden medir estan a la vista y se
//! configuran, en vez de escondidos en una constante:
//!
//!     base_w          el resto del equipo, en vatios
//!     eficiencia_psu  lo que se paga entra por el enchufe, no por los rieles
//!     monitores_w     no salen en ningun sensor; 0 = no contarlos
//!
//! Para tener el numero de verdad hace falta un enchufe medidor. Esto sirve
//! para ver la FORMA del gasto -- que dias, que horas, cuanto sube al jugar --
//! y para una cifra aproximada mientras esos tres valores esten calibrados.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Lo acumulado en un dia.
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
struct Dia {
    /// Energia en la toma de corriente, vatios-hora.
    wh: f64,
    /// Segundos de los que se tiene medida. NO son las horas del dia: si el
    /// equipo estuvo apagado, esos segundos no cuentan, y se ve aqui.
    segundos: f64,
    /// Punta observada, en vatios.
    w_max: f64,
    muestras: u64,
}

type Almacen = std::collections::BTreeMap<String, Dia>;

/// Reloj local de Windows. Ya sabe de zona horaria y de horario de verano, y
/// no hay que parsear nada.
///
/// La primera version llamaba a `cmd /c echo %DATE%` y partia el resultado. Dos
/// fallos en uno: el formato depende del idioma -- en español trae el dia de la
/// semana delante y el parseo caia al respaldo, guardando los dias como
/// "epoch-20698" -- y ademas lanzaba un proceso EN CADA MUESTRA, que a una cada
/// diez segundos son 8.640 procesos al dia para preguntar la fecha.
#[repr(C)]
#[derive(Default)]
struct SystemTimeWin {
    ano: u16,
    mes: u16,
    dia_semana: u16,
    dia: u16,
    hora: u16,
    minuto: u16,
    segundo: u16,
    milis: u16,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetLocalTime(lpSystemTime: *mut SystemTimeWin);
}

fn hoy() -> String {
    let mut t = SystemTimeWin::default();
    unsafe { GetLocalTime(&mut t) };
    format!("{:04}-{:02}-{:02}", t.ano, t.mes, t.dia)
}

fn ruta_almacen(dia: &str) -> PathBuf {
    let mes = dia.get(..7).unwrap_or("desconocido").to_string();
    let mut p = PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default());
    p.push(".config");
    p.push("consumo");
    let _ = std::fs::create_dir_all(&p);
    // Un archivo por mes: acotado, y borrar un mes viejo no toca el resto.
    p.push(format!("{mes}.json"));
    p
}

fn cargar(dia: &str) -> Almacen {
    std::fs::read_to_string(ruta_almacen(dia))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn guardar(dia: &str, a: &Almacen) {
    let ruta = ruta_almacen(dia);
    let Ok(texto) = serde_json::to_string_pretty(a) else { return };
    // Escritura atomica: a un temporal y luego renombrar. Un corte de luz a
    // mitad de un write deja el archivo truncado, y perder el mes entero por
    // eso seria justo el fallo que este programa existe para no tener.
    let tmp = ruta.with_extension("json.tmp");
    if std::fs::write(&tmp, texto).is_ok() {
        let _ = std::fs::rename(&tmp, &ruta);
    }
}

// ------------------------------------------------------------- sensores ---

/// Potencia de la GPU. `nvidia-smi` la da sin privilegios, que es lo que la
/// hace util: es el unico sensor de potencia disponible sin elevar nada.
fn gpu_w() -> Option<f64> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let salida = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=power.draw", "--format=csv,noheader,nounits"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    String::from_utf8(salida.stdout).ok()?.trim().parse().ok()
}

/// GET a pelo contra localhost. Es la unica peticion HTTP del programa y va a
/// una direccion fija, asi que no hace falta un cliente de verdad.
fn get(url: &str) -> Option<String> {
    let resto = url.strip_prefix("http://")?;
    let (host_puerto, ruta) = match resto.find('/') {
        Some(i) => (&resto[..i], &resto[i..]),
        None => (resto, "/"),
    };
    let mut flujo = std::net::TcpStream::connect_timeout(
        &host_puerto.parse().ok()?,
        Duration::from_millis(800),
    )
    .ok()?;
    let _ = flujo.set_read_timeout(Some(Duration::from_secs(2)));
    let peticion = format!("GET {ruta} HTTP/1.0\r\nHost: {host_puerto}\r\nConnection: close\r\n\r\n");
    flujo.write_all(peticion.as_bytes()).ok()?;
    let mut cuerpo = String::new();
    flujo.read_to_string(&mut cuerpo).ok()?;
    // Separar cabeceras del cuerpo.
    let i = cuerpo.find("\r\n\r\n")?;
    Some(cuerpo[i + 4..].to_string())
}

/// Potencia del paquete de CPU segun LibreHardwareMonitor.
///
/// Devuelve `None` cuando LHM no esta -- que es lo normal si no corre elevado:
/// su servidor web usa HttpListener sobre `+:8085` y eso pide privilegios, y
/// los sensores de potencia de CPU necesitan su driver. Sin LHM el medidor
/// sigue funcionando, solo que sin el termino de CPU, y lo dice.
fn cpu_w(url: &str) -> Option<f64> {
    if url.is_empty() {
        return None;
    }
    let json = get(url)?;
    let v: serde_json::Value = serde_json::from_str(&json).ok()?;
    let mut mejor: Option<f64> = None;
    buscar_potencia_cpu(&v, false, &mut mejor);
    mejor
}

/// Recorre el arbol de LHM buscando el sensor de potencia del paquete de CPU.
///
/// El arbol es {Text, Children, Value} anidado y su forma cambia entre
/// versiones, asi que se busca por nombre en vez de por una ruta fija: se entra
/// en la rama de la CPU y dentro se coge "Package"/"CPU Package".
fn buscar_potencia_cpu(v: &serde_json::Value, dentro_cpu: bool, mejor: &mut Option<f64>) {
    let texto = v.get("Text").and_then(|t| t.as_str()).unwrap_or("");
    let bajo = texto.to_lowercase();
    // Las ramas de nodo llevan el tipo en "Text" de sus hijos; basta con saber
    // si ya se esta dentro de un procesador.
    let ahora_cpu = dentro_cpu
        || bajo.contains("intel")
        || bajo.contains("amd ")
        || bajo.contains("ryzen")
        || bajo.contains("core i");
    if ahora_cpu && (bajo == "package" || bajo.contains("cpu package")) {
        if let Some(w) = v
            .get("Value")
            .and_then(|s| s.as_str())
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.replace(',', ".").parse::<f64>().ok())
        {
            // Puede haber varios; el mayor es el del paquete entero.
            *mejor = Some(mejor.unwrap_or(0.0).max(w));
        }
    }
    if let Some(hijos) = v.get("Children").and_then(|c| c.as_array()) {
        for h in hijos {
            buscar_potencia_cpu(h, ahora_cpu, mejor);
        }
    }
}

// -------------------------------------------------------------- informe ---

fn moneda(cfg: &rice_common::settings::Consumo, wh: f64) -> String {
    if cfg.precio_kwh <= 0.0 {
        return "sin precio configurado".into();
    }
    format!("{}{:.2}", cfg.moneda, wh / 1000.0 * cfg.precio_kwh)
}

fn informe(solo_hoy: bool) {
    let cfg = rice_common::settings::Settings::live().consumo.clone();
    let d = hoy();
    let a = cargar(&d);
    if a.is_empty() {
        println!("todavia no hay medidas (el medidor las escribe cada minuto)");
        return;
    }
    let dias: Vec<(&String, &Dia)> = if solo_hoy {
        a.iter().filter(|(k, _)| **k == d).collect()
    } else {
        a.iter().collect()
    };
    if dias.is_empty() {
        println!("hoy ({d}) todavia sin medidas");
        return;
    }
    println!("{:<12} {:>9} {:>10} {:>9} {:>8}", "dia", "kWh", "coste", "medido", "punta");
    let (mut twh, mut tseg) = (0.0, 0.0);
    for (k, v) in &dias {
        println!(
            "{:<12} {:>9.3} {:>10} {:>8.1}h {:>7.0}W",
            k,
            v.wh / 1000.0,
            moneda(&cfg, v.wh),
            v.segundos / 3600.0,
            v.w_max
        );
        twh += v.wh;
        tseg += v.segundos;
    }
    if dias.len() > 1 {
        println!("{:<12} {:>9.3} {:>10} {:>8.1}h", "TOTAL", twh / 1000.0, moneda(&cfg, twh), tseg / 3600.0);
    }
    println!();
    println!("estimacion: GPU medida + CPU (si LHM esta) + base_w {} W, dividido por", cfg.base_w);
    println!("eficiencia_psu {} y sumando monitores_w {} W. Ajusta esos tres en", cfg.eficiencia_psu, cfg.monitores_w);
    println!("rice.json contra un enchufe medidor si quieres que cuadre con el recibo.");
    if cfg.precio_kwh <= 0.0 {
        println!();
        println!("pon consumo.precio_kwh en rice.json (mira tu recibo) para ver el coste.");
    }
}

// ------------------------------------------------------------ muestreo ---

fn medir() {
    let cfg = rice_common::settings::Settings::live().consumo.clone();
    let intervalo = Duration::from_secs(cfg.intervalo_s.max(1));
    // Tope del hueco que se acepta al integrar. Si entre dos muestras pasa mas
    // que esto, el equipo estuvo suspendido o el proceso parado: se cuenta solo
    // hasta el tope en vez de multiplicar la potencia actual por las ocho horas
    // que estuvo dormido, que es como un medidor ingenuo se inventa un dia
    // entero de gasto.
    let tope = intervalo * 3;

    let mut dia = hoy();
    let mut almacen = cargar(&dia);
    let mut ultimo = Instant::now();
    let mut sin_guardar = Duration::ZERO;

    loop {
        std::thread::sleep(intervalo);
        let ahora = Instant::now();
        let dt = (ahora - ultimo).min(tope);
        ultimo = ahora;

        let gpu = gpu_w().unwrap_or(0.0);
        let cpu = cpu_w(&cfg.lhm_url).unwrap_or(0.0);
        // La fuente pierde una parte de lo que entrega, y lo que se factura es
        // lo que entra por el enchufe. Los monitores van fuera de esa division:
        // tienen su propia fuente.
        let componentes = gpu + cpu + cfg.base_w;
        let pared = componentes / cfg.eficiencia_psu.clamp(0.5, 1.0) + cfg.monitores_w;

        // El dia puede cambiar a mitad de muestreo; entonces se cierra el
        // anterior en disco antes de empezar el nuevo.
        let d = hoy();
        if d != dia {
            guardar(&dia, &almacen);
            dia = d;
            almacen = cargar(&dia);
        }

        let e = almacen.entry(dia.clone()).or_default();
        e.wh += pared * dt.as_secs_f64() / 3600.0;
        e.segundos += dt.as_secs_f64();
        e.w_max = e.w_max.max(pared);
        e.muestras += 1;

        // Guardar cada minuto y no en cada muestra: un corte deja fuera como
        // mucho un minuto de cuentas, y no se castiga al SSD con 8.640
        // escrituras diarias por un archivo de dos kilobytes.
        sin_guardar += dt;
        if sin_guardar >= Duration::from_secs(60) {
            guardar(&dia, &almacen);
            sin_guardar = Duration::ZERO;
        }
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    // Mismo criterio que micswitch: un argumento desconocido aborta en vez de
    // caer al comportamiento por defecto, que aqui seria quedarse midiendo para
    // siempre sin que nadie lo esperase.
    const CONOCIDAS: [&str; 4] = ["--hoy", "--mes", "--help", "-h"];
    let sueltos: Vec<&str> = argv[1..]
        .iter()
        .map(|a| a.as_str())
        .filter(|a| !CONOCIDAS.contains(a))
        .collect();
    let uso = "consumo -- estimador de gasto electrico

  consumo          muestrea y acumula (lo lanza el supervisor)
  consumo --hoy    lo gastado hoy
  consumo --mes    el desglose del mes

Los datos van a ~/.config/consumo/AAAA-MM.json.";
    if argv.iter().any(|a| a == "--help" || a == "-h") {
        println!("{uso}");
        return;
    }
    if !sueltos.is_empty() {
        eprintln!("consumo: argumento no reconocido: {}", sueltos.join(" "));
        eprintln!("{uso}");
        std::process::exit(2);
    }
    if argv.iter().any(|a| a == "--hoy") {
        informe(true);
    } else if argv.iter().any(|a| a == "--mes") {
        informe(false);
    } else {
        medir();
    }
}
