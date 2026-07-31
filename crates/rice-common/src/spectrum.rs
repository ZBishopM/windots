//! A tiny live audio spectrum, for drawing a few bars inside the island.
//!
//! Lee del anillo compartido de `audioshare`, no de un `sysaudio-loopback.exe`
//! propio. ANTES cada barra lanzaba el suyo: con dos monitores eso eran dos
//! capturas WASAPI del audio del sistema, mas la del grabador, tres clientes del
//! mismo endpoint para el mismo flujo. Ahora captura uno y lee quien quiera.
//!
//! Si nadie publica -- el caso normal es que el grabador no este corriendo --
//! se lanza uno de respaldo. Que no haya dos lo garantiza el HELPER, con su
//! propia instancia unica, no la barra: ver `ensure_publisher`. Ese publicador se
//! apaga solo cuando dejan de leerle, asi que no queda de huerfano aunque su
//! lanzador muera de golpe.
//!
//! Todo corre en su propio hilo; `Spectrum::levels()` es una lectura barata de
//! los ultimos valores publicados.

use std::sync::{Arc, Mutex};

const FFT_SIZE: usize = 1024;
const SAMPLE_RATE: f32 = 48000.0;
const MIN_FREQ: f32 = 40.0;
const MAX_FREQ: f32 = 12000.0;
/// Temporal smoothing: how much of the previous frame is kept.
const SMOOTH: f32 = 0.55;
/// Decay applied when the reader stalls, so bars fall instead of freezing.
const DECAY: f32 = 0.90;
/// Cadencia de lectura. 20 ms da 50 analisis por segundo, de sobra para ocho
/// barras que ademas van suavizadas.
const TICK_MS: u64 = 20;
/// Mínimo entre intentos de lanzar el publicador de respaldo. Es sólo un freno
/// contra ráfagas: quien de verdad garantiza que no haya dos es el propio helper,
/// que toma su instancia única al arrancar.
const SPAWN_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(5);

pub struct Spectrum {
    levels: Arc<Mutex<Vec<f32>>>,
    /// Se guarda para matar al publicador de respaldo si esta barra lo lanzo y
    /// se cierra ordenadamente. Si muere de golpe, el propio helper se apaga al
    /// ver que nadie lee.
    child: Arc<Mutex<Option<std::process::Child>>>,
}

/// Lanza el publicador de respaldo si no hay ninguno.
///
/// La exclusion NO se decide aqui, y esa es la correccion.
///
/// La primera version tomaba un mutex con nombre en la barra antes de lanzar. No
/// vale, por dos motivos que se tapaban entre si:
///
///   1. `single_instance` devuelve false tambien cuando el mutex lo tiene ESTE
///      mismo proceso -- `CreateMutexW` contesta ERROR_ALREADY_EXISTS igual --
///      asi que tras el primer lanzamiento la barra se autobloqueaba.
///   2. Un mutex de Win32 solo puede soltarlo el hilo que lo tomo, asi que
///      tampoco servia guardarlo para soltarlo cuando muriera el hijo.
///
/// Resultado: en cuanto el publicador de respaldo se apagaba -- cosa que hace
/// nada mas aparecer el grabador -- ninguna barra volvia a lanzar otro, y si
/// luego moria el grabador el espectro se quedaba mudo hasta reiniciar la barra.
///
/// Ahora la exclusion la hace el HELPER, que toma su propia instancia unica al
/// arrancar y se va si ya hay otra. Un proceso suelta sus mutex al morir, pase lo
/// que pase. Las dos barras pueden lanzar a la vez: el perdedor sale al instante.
#[cfg(windows)]
fn ensure_publisher(
    child: &Arc<Mutex<Option<std::process::Child>>>,
    ultimo: &mut std::time::Instant,
) {
    {
        let mut g = child.lock().unwrap();
        if let Some(c) = g.as_mut() {
            if matches!(c.try_wait(), Ok(Some(_))) {
                *g = None;
            } else {
                return; // el nuestro sigue vivo
            }
        }
    }
    if ultimo.elapsed() < SPAWN_COOLDOWN {
        return;
    }
    *ultimo = std::time::Instant::now();
    use std::os::windows::process::CommandExt;
    let exe = crate::win::sibling_exe("sysaudio-loopback.exe");
    if let Ok(c) = std::process::Command::new(exe)
        .arg("--publish-only")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(crate::win::CREATE_NO_WINDOW)
        .spawn()
    {
        *child.lock().unwrap() = Some(c);
    }
}

#[cfg(not(windows))]
fn ensure_publisher(_c: &Arc<Mutex<Option<std::process::Child>>>, _u: &mut std::time::Instant) {}

impl Spectrum {
    /// Start capturing. `bands` is how many bars to publish.
    pub fn start(bands: usize) -> Self {
        let levels = Arc::new(Mutex::new(vec![0.0; bands]));
        let child: Arc<Mutex<Option<std::process::Child>>> = Arc::new(Mutex::new(None));
        let out = levels.clone();
        let child2 = child.clone();
        std::thread::spawn(move || {
            let mut planner = rustfft::FftPlanner::<f32>::new();
            let fft = planner.plan_fft_forward(FFT_SIZE);
            let hann: Vec<f32> = (0..FFT_SIZE)
                .map(|i| {
                    0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (FFT_SIZE as f32 - 1.0)).cos()
                })
                .collect();
            let mut ring = vec![0f32; FFT_SIZE];
            let mut raw = vec![0i16; FFT_SIZE * 2]; // s16 estereo intercalado
            let mut scratch: Vec<rustfft::num_complex::Complex<f32>> =
                vec![rustfft::num_complex::Complex::new(0.0, 0.0); FFT_SIZE];
            let mut smoothed = vec![0f32; bands];
            let mut agc = 1.0f32;

            #[cfg(windows)]
            let mut lector: Option<crate::audioshare::Reader> = None;
            // En el pasado para que el primer intento sea inmediato.
            let mut ultimo_intento = std::time::Instant::now() - SPAWN_COOLDOWN;

            loop {
                std::thread::sleep(std::time::Duration::from_millis(TICK_MS));

                #[cfg(windows)]
                let leido = {
                    if lector.is_none() {
                        lector = crate::audioshare::Reader::open(crate::audioshare::SYS_NAME);
                    }
                    match &lector {
                        Some(r) => {
                            // `advancing` distingue "no suena nada" -- el
                            // publicador sigue mandando silencio y el contador
                            // sube -- de "no hay publicador", que es el contador
                            // parado. Solo el segundo caso justifica lanzar uno.
                            if r.advancing() {
                                r.latest(&mut raw)
                            } else {
                                ensure_publisher(&child2, &mut ultimo_intento);
                                false
                            }
                        }
                        None => {
                            ensure_publisher(&child2, &mut ultimo_intento);
                            false
                        }
                    }
                };
                #[cfg(not(windows))]
                let leido = false;

                if !leido {
                    // Sin datos: que las barras caigan en vez de congelarse.
                    let mut l = out.lock().unwrap();
                    for v in l.iter_mut() {
                        *v *= DECAY;
                    }
                    continue;
                }

                // Downmix to mono into the ring.
                for i in 0..FFT_SIZE {
                    let l = raw[i * 2] as f32 / 32768.0;
                    let r = raw[i * 2 + 1] as f32 / 32768.0;
                    ring[i] = (l + r) * 0.5;
                }
                for i in 0..FFT_SIZE {
                    scratch[i] = rustfft::num_complex::Complex::new(ring[i] * hann[i], 0.0);
                }
                fft.process(&mut scratch);

                // Log-spaced bands, peak magnitude in each.
                let mut peak = 0.0f32;
                let mut nowv = vec![0f32; bands];
                for b in 0..bands {
                    let f_lo = MIN_FREQ * (MAX_FREQ / MIN_FREQ).powf(b as f32 / bands as f32);
                    let f_hi = MIN_FREQ * (MAX_FREQ / MIN_FREQ).powf((b + 1) as f32 / bands as f32);
                    let lo = ((f_lo / SAMPLE_RATE * FFT_SIZE as f32) as usize).max(1);
                    let hi = ((f_hi / SAMPLE_RATE * FFT_SIZE as f32) as usize)
                        .max(lo + 1)
                        .min(FFT_SIZE / 2);
                    let mut v = 0.0f32;
                    for c in &scratch[lo..hi] {
                        v = v.max(c.norm());
                    }
                    let v = v.sqrt() * agc;
                    peak = peak.max(v);
                    nowv[b] = v;
                }
                // Auto-gain so quiet tracks still move the bars.
                if peak > 0.001 {
                    let want = 0.85 / peak * agc;
                    agc += (want - agc) * if want < agc { 0.15 } else { 0.03 };
                    agc = agc.clamp(0.001, 100_000.0);
                }
                for b in 0..bands {
                    smoothed[b] = smoothed[b] * SMOOTH + nowv[b].min(1.0) * (1.0 - SMOOTH);
                }
                *out.lock().unwrap() = smoothed.clone();
            }
        });
        Self { levels, child }
    }

    /// Latest band levels, 0..1.
    pub fn levels(&self) -> Vec<f32> {
        self.levels.lock().unwrap().clone()
    }

    /// Is anything actually audible right now?
    pub fn active(&self) -> bool {
        self.levels.lock().unwrap().iter().any(|v| *v > 0.02)
    }
}

impl Drop for Spectrum {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.lock().unwrap().take() {
            let _ = c.kill();
        }
    }
}
