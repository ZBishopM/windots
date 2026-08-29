// WASAPI capture -> raw s16le stereo @ 48000 Hz on stdout, so the ShadowPlay save
// step can mux it with the video. Native, no third-party driver, no routing change.
//
// Two modes share the same resample/downmix/silence-fill path:
//   (default)  loopback of the default RENDER endpoint = whatever the system plays.
//   --mic      the default CAPTURE endpoint = your microphone.
// Either mode follows the ACTIVE default device: switch the mic (or the output) in
// Windows and it reopens on the new one within ~2s, no restart needed.
//
// Keeps the audio stream CONTINUOUS in two ways so ffmpeg's muxer never starves
// (which would freeze the whole recording):
//   1. When the endpoint is idle (nothing playing), WASAPI loopback delivers no
//      packets at all -> we emit wallclock-paced silence. This runs ONLY when no
//      real packets are pending, so it never injects silence between real samples
//      (that made audio choppy in an earlier version).
//   2. If the endpoint is invalidated (the default device changes, e.g. a video
//      switches audio output), we reopen it instead of dying.

use std::io::Write;
use std::time::{Duration, Instant};
use rice_common::audioshare;
use windows::core::*;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;

const OUT_RATE: f64 = 48000.0;
/// Lo toma la instancia del grabador; la de --publish-only se aparta al verlo.
///
/// Doble barra. Con una sola, `\r` es un retorno de carro valido en Rust: el
/// nombre real pasaba a ser `Global<CR>ice-audio-primary`, compilaba sin una
/// queja y funcionaba de chiripa porque los dos sitios llevaban la misma cadena
/// mal. Ni estaba en el espacio Global ni decia lo que aparentaba.
const PRIMARIO: &str = "Global\\rice-audio-primary";
/// Instancia unica del publicador de respaldo. Lo toma EL HELPER, no la barra:
/// un proceso suelta sus mutex al morir pase lo que pase, y un mutex de Win32
/// solo puede soltarlo el hilo que lo tomo.
const RESPALDO: &str = "Global\\rice-audio-backup";

fn main() -> Result<()> {
    let mic = std::env::args().any(|a| a == "--mic");
    // --publish-only: no hay nadie al otro lado de stdout. Lo lanzan las barras
    // cuando el grabador no esta corriendo y por tanto nadie publica el audio del
    // sistema. Se apaga solo cuando dejan de leerle, para no quedar de huerfano
    // si muere quien lo lanzo (que puede morir sin correr su Drop).
    let solo_publicar = std::env::args().any(|a| a == "--publish-only");

    // El audio del sistema se publica SIEMPRE en memoria compartida, lo lea
    // alguien o no. Antes cada consumidor abria su propia captura WASAPI: dos
    // barras (para el espectro) mas el grabador, tres clientes del mismo
    // endpoint. El microfono no se comparte porque solo lo usa el grabador.
    // UN SOLO escritor en el anillo, y con prioridad clara.
    //
    // Hay dos clases de instancia y no da igual cual publique: la del grabador
    // vive siempre (esta supervisada) y ya captura de todas formas para los
    // clips; la de --publish-only solo existe para tapar el hueco cuando el
    // grabador no esta. Sin desempate, la barra arranca antes en el login,
    // levanta la suya, y luego llega la del grabador y quedan DOS escribiendo el
    // mismo anillo -- que es peor que las tres capturas que esto venia a quitar
    // (paso, medido: tres procesos donde deberian ser dos).
    //
    // Asi que la del grabador toma PRIMARIO y publica; la de --publish-only se
    // aparta en cuanto ese mutex aparece, al arrancar y cada dos segundos.
    let publicador = if mic {
        None
    } else if solo_publicar {
        // Instancia unica lo PRIMERO: las dos barras pueden lanzarnos a la vez y
        // solo puede quedar uno escribiendo el anillo.
        rice_common::win::single_instance_or_exit(RESPALDO);
        if rice_common::win::mutex_taken(PRIMARIO) {
            return Ok(()); // ya publica el del grabador: sobramos
        }
        audioshare::Publisher::create(audioshare::SYS_NAME, OUT_RATE as u32)
    } else {
        // Tomar PRIMARIO no basta: hay que ESPERAR a que el de respaldo se vaya.
        //
        // El de respaldo tiene RESPALDO, no PRIMARIO, asi que esto lo consigue
        // siempre y se ponia a publicar en el acto. Durante los hasta 2 s que
        // aquel tarda en mirar PRIMARIO habia DOS escribiendo el mismo anillo, y
        // fotogramas de dos fuentes intercalados son ruido para quien lea.
        //
        // Tomar PRIMARIO es la senal de intencion; soltar RESPALDO es su acuse.
        rice_common::win::single_instance(PRIMARIO);
        let espera = Instant::now();
        while rice_common::win::mutex_taken(RESPALDO) && espera.elapsed().as_secs() < 4 {
            std::thread::sleep(Duration::from_millis(50));
        }
        audioshare::Publisher::create(audioshare::SYS_NAME, OUT_RATE as u32)
    };
    // Que no vuelva a fallar en silencio: la primera version pedia el mapeo en
    // Global\, que un proceso sin elevar no puede crear, y el unico sintoma era
    // que las barras se quedaban sin espectro.
    if !mic && publicador.is_none() {
        eprintln!("no pude crear el anillo compartido {}", audioshare::SYS_NAME);
    }

    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        let stdout = std::io::stdout();
        let mut sink = Sink {
            out: std::io::BufWriter::with_capacity(1 << 16, stdout.lock()),
            stage: Vec::with_capacity(4096),
            publicador,
            a_stdout: !solo_publicar,
        };
        // Reopen on any capture error (device invalidated / format change) so the
        // pipe to the reader never permanently closes.
        loop {
            if let Err(e) = capture(&mut sink, mic, solo_publicar) {
                eprintln!("{}: {e:?}; reopening endpoint", if mic { "mic" } else { "loopback" });
                std::thread::sleep(Duration::from_millis(300));
            }
        }
    }
}

/// Salida doble: el tubo de siempre y el anillo compartido.
struct Sink<W: Write> {
    out: W,
    /// Fotogramas acumulados desde el ultimo volcado, para publicar por lotes en
    /// vez de fotograma a fotograma (48.000 escrituras por segundo a memoria
    /// compartida no tendrian sentido).
    stage: Vec<i16>,
    publicador: Option<audioshare::Publisher>,
    a_stdout: bool,
}

impl<W: Write> Sink<W> {
    fn frame(&mut self, l: f32, r: f32) {
        let li = (l.clamp(-1.0, 1.0) * 32767.0) as i16;
        let ri = (r.clamp(-1.0, 1.0) * 32767.0) as i16;
        if self.a_stdout {
            // One 4-byte write per frame, not two 2-byte ones: at 48 kHz stereo
            // that halves the call count on the hottest path in the project.
            let mut buf = [0u8; 4];
            buf[0..2].copy_from_slice(&li.to_le_bytes());
            buf[2..4].copy_from_slice(&ri.to_le_bytes());
            let _ = self.out.write_all(&buf);
        }
        if self.publicador.is_some() {
            self.stage.push(li);
            self.stage.push(ri);
        }
    }

    /// Devuelve false si el tubo se cerro, que es como se detecta que el lector
    /// se fue.
    fn flush(&mut self) -> bool {
        if let Some(p) = &self.publicador {
            if !self.stage.is_empty() {
                p.write(&self.stage);
                self.stage.clear();
            }
        }
        if !self.a_stdout {
            return true;
        }
        self.out.flush().is_ok()
    }
}

unsafe fn capture<W: Write>(out: &mut Sink<W>, mic: bool, solo_publicar: bool) -> Result<()> {
    let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    // Mic = default capture endpoint, no loopback flag. Otherwise = default render
    // endpoint captured in loopback mode.
    let (flow, stream_flags) = if mic {
        (eCapture, 0u32)
    } else {
        (eRender, AUDCLNT_STREAMFLAGS_LOOPBACK)
    };
    // Con `record_mic` puesto se busca ese microfono por nombre; si no esta
    // conectado se cae al predeterminado, que es peor que nada pero mejor que
    // no grabar. Ver el comentario del ajuste en rice-common/settings.rs.
    let preferido = if mic { rice_common::settings::Settings::load().record_mic } else { String::new() };
    let device = match buscar_endpoint(&enumerator, flow, &preferido) {
        Some(d) => d,
        None => {
            if !preferido.is_empty() {
                eprintln!("mic: no encontre ninguno que contenga {preferido:?}; uso el predeterminado");
            }
            enumerator.GetDefaultAudioEndpoint(flow, eConsole)?
        }
    };
    let dev_id = endpoint_id(&device); // to detect a later default-device switch
    let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;

    let pwfx = client.GetMixFormat()?;
    let wf = *pwfx;
    let in_rate = wf.nSamplesPerSec as f64;
    let in_ch = wf.nChannels as usize;
    let bits = wf.wBitsPerSample;
    let is_float = wf.wFormatTag == 3 /* IEEE_FLOAT */
        || (wf.wFormatTag == 0xFFFE /* EXTENSIBLE */ && bits == 32);
    eprintln!(
        "{}: \"{}\" | {in_rate} Hz, {in_ch} ch, {bits} bit, float={is_float}",
        if mic { "mic" } else { "loopback" },
        endpoint_name(&device).unwrap_or_else(|| "(sin nombre)".into())
    );

    // 200ms shared buffer.
    let hns_buffer: i64 = 2_000_000;
    client.Initialize(
        AUDCLNT_SHAREMODE_SHARED,
        stream_flags,
        hns_buffer,
        0,
        pwfx,
        None,
    )?;
    let capture: IAudioCaptureClient = client.GetService()?;
    client.Start()?;

    let ratio = OUT_RATE / in_rate; // out samples per in sample
    let mut resamp_pos = 0.0f64; // fractional read position for linear resample
    let mut prev_l = 0.0f32;
    let mut prev_r = 0.0f32;

    // Keep output at wallclock real-time: count every frame written and, only
    // when GENUINELY idle, top up with silence to the wallclock-expected count.
    // The idle threshold is the key: WASAPI hands us audio in ~10ms packets, so a
    // single empty poll is NORMAL during active playback -- filling silence then
    // (as an earlier version did) injects micro-gaps between real samples and
    // stretches the audio (distortion + drift). We only fill after a sustained
    // gap with zero packets, which only happens when nothing is playing.
    let start = Instant::now();
    let mut frames_out: u64 = 0;
    let mut last_packet = Instant::now();
    let mut last_dev_check = start;
    // Solo se usan en --publish-only; ver el final del bucle.
    let mut ultimas_lecturas = 0u64;
    let mut sin_lectores = start;
    // Cronometro PROPIO. Reusar last_dev_check no valia: se reasigna a `now` doce
    // lineas mas arriba en esta misma vuelta, asi que la condicion de abajo nunca
    // llegaba a ser cierta y toda la comprobacion periodica era codigo muerto.
    let mut ultimo_chequeo_primario = start;
    const IDLE_SECS: f64 = 0.15;

    loop {
        let mut got_real = false;
        // Drain all currently available packets.
        loop {
            let avail = capture.GetNextPacketSize()?;
            if avail == 0 {
                break;
            }
            got_real = true;
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut frames: u32 = 0;
            let mut flags: u32 = 0;
            capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
            let silent = (flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;

            // Read interleaved -> downmix to stereo f32.
            let n = frames as usize;
            for i in 0..n {
                let (mut l, mut r) = (0.0f32, 0.0f32);
                if !silent && !data.is_null() {
                    if is_float {
                        let base = (data as *const f32).add(i * in_ch);
                        l = *base;
                        r = if in_ch > 1 { *base.add(1) } else { l };
                    } else if bits == 16 {
                        let base = (data as *const i16).add(i * in_ch);
                        l = *base as f32 / 32768.0;
                        r = if in_ch > 1 { *base.add(1) as f32 / 32768.0 } else { l };
                    }
                }
                // Linear resample from in_rate -> OUT_RATE.
                if (ratio - 1.0).abs() < 1e-6 {
                    out.frame(l, r);
                    frames_out += 1;
                } else {
                    while resamp_pos < 1.0 {
                        let t = resamp_pos as f32;
                        out.frame(prev_l + (l - prev_l) * t, prev_r + (r - prev_r) * t);
                        frames_out += 1;
                        resamp_pos += 1.0 / ratio;
                    }
                    resamp_pos -= 1.0;
                }
                prev_l = l;
                prev_r = r;
            }
            capture.ReleaseBuffer(frames)?;
        }

        let now = Instant::now();
        if got_real {
            last_packet = now;
        } else if now.duration_since(last_packet).as_secs_f64() > IDLE_SECS {
            // Genuinely idle (no packets for >150ms): top up silence to wallclock
            // so ffmpeg's audio timeline keeps advancing (otherwise its muxer
            // starves and the whole recording freezes).
            let expected = (now.duration_since(start).as_secs_f64() * OUT_RATE) as u64;
            while frames_out < expected {
                out.frame(0.0, 0.0);
                frames_out += 1;
            }
        }
        // Follow the active default device: if the user switches the mic (HyperX <->
        // Snowball) or the output, reopen on the new default within ~2s.
        if now.duration_since(last_dev_check).as_secs_f64() > 2.0 {
            last_dev_check = now;
            if let Ok(cur) = enumerator.GetDefaultAudioEndpoint(flow, eConsole) {
                if endpoint_id(&cur) != dev_id {
                    return Ok(());
                }
            }
        }
        // Flushing every tick looks like it defeats the BufWriter, and it does --
        // deliberately. cava consumes this stream for a realtime visualiser, and
        // letting a 64 KB buffer fill at 192 KB/s would add ~340ms of lag. 200
        // flushes/s is cheap; the per-frame writes were the expensive part.
        // It doubles as the liveness check: once the reader (cava / ffmpeg) is
        // gone the write fails and we exit instead of running on as an orphan.
        if !out.flush() {
            std::process::exit(0);
        }
        // En --publish-only no hay tubo que se cierre, asi que la senal de que
        // sobramos es que nadie lea el anillo. Sin esto, un helper lanzado por
        // una barra que luego muere de golpe se quedaria capturando para nadie.
        if solo_publicar {
            // Llego el del grabador: nos apartamos para no escribir dos en el
            // mismo anillo.
            if now.duration_since(ultimo_chequeo_primario).as_secs_f64() > 2.0 {
                ultimo_chequeo_primario = now;
                if rice_common::win::mutex_taken(PRIMARIO) {
                    std::process::exit(0);
                }
            }
            if let Some(p) = &out.publicador {
                let r = p.reads();
                if r == ultimas_lecturas {
                    if now.duration_since(sin_lectores).as_secs() >= 30 {
                        std::process::exit(0);
                    }
                } else {
                    ultimas_lecturas = r;
                    sin_lectores = now;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

// Endpoint id string of a device (COM-allocated -> freed here). Used to notice when
// the default device changes so we can reopen on the new one.
unsafe fn endpoint_id(dev: &IMMDevice) -> Option<String> {
    let p = dev.GetId().ok()?;
    let s = p.to_string().ok();
    CoTaskMemFree(Some(p.0 as *const core::ffi::c_void));
    s
}

/// Primer endpoint ACTIVO cuyo nombre contenga `quiere` (sin distinguir
/// mayusculas). `quiere` vacio devuelve None, que el llamador trata como
/// "usa el predeterminado" -- asi el camino de siempre no cambia.
unsafe fn buscar_endpoint(
    enumerator: &IMMDeviceEnumerator,
    flow: EDataFlow,
    quiere: &str,
) -> Option<IMMDevice> {
    if quiere.is_empty() {
        return None;
    }
    let quiere = quiere.to_lowercase();
    // Solo DEVICE_STATE_ACTIVE: un endpoint deshabilitado o desconectado se
    // enumera igual y activarlo falla, que es exactamente el caso del headset
    // inalambrico apagado.
    let col = enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE).ok()?;
    for i in 0..col.GetCount().ok()? {
        let d = col.Item(i).ok()?;
        if endpoint_name(&d).is_some_and(|n| n.to_lowercase().contains(&quiere)) {
            return Some(d);
        }
    }
    None
}

/// Nombre legible del endpoint, p. ej. "Microfono (HyperX Cloud II Wireless)".
///
/// Existe porque su ausencia costo una tarde: esta maquina tiene SEIS entradas
/// de captura (HyperX, Blue Snowball, VB-Cable, Oculus, Steam, RODE) y aqui se
/// abre la que Windows tenga como predeterminada. Si esa no es la que el
/// usuario usa, se graba ruido de sala -- indistinguible desde fuera de "el
/// microfono no funciona", porque el log solo decia el formato (48000 Hz,
/// 2 ch) y eso es identico en todas.
unsafe fn endpoint_name(dev: &IMMDevice) -> Option<String> {
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::System::Com::StructuredStorage::PropVariantClear;
    let store = dev.OpenPropertyStore(STGM_READ).ok()?;
    let mut v = store.GetValue(&PKEY_Device_FriendlyName).ok()?;
    let s = v.to_string();
    let _ = PropVariantClear(&mut v);
    Some(s)
}
