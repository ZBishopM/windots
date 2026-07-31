// WGC rolling-buffer recorder. Captures the primary monitor via
// Windows.Graphics.Capture and hardware-encodes to a ring of short HEVC MP4
// segments. System audio AND the microphone (WASAPI via two sysaudio-loopback
// helpers, s16le 48kHz stereo) are each written to a parallel ring of raw PCM
// files -- seg*.pcm and seg*.mic.pcm -- and mixed with the video at save time.
//
// Why audio is NOT muxed live through the encoder: windows-capture's MF
// MediaStreamSource pulls audio and video samples on a shared WinRT thread pool
// with a blocking recv per request; with audio enabled the A/V interleaving
// starves video sample delivery and the encoded video collapses from the real
// ~55 fps capture rate to ~21 unique fps (measured). Recording video-only keeps
// the full capture rate; audio is captured independently and joined on save.
//
// Segment rotation is PIPELINED so the capture thread never blocks:
//   - a "maker" thread pre-warms the next encoder (VideoEncoder::new does a
//     blocking PrepareTranscodeAsync join), so the boundary swap is instant;
//   - a "finisher" thread runs finish() (joins the transcode thread + finalizes
//     the MP4, ~1s) off the capture callback.
// Usage: shadowplay-wgc [buffer_dir]

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows_capture::{
    capture::{Context, GraphicsCaptureApiHandler},
    encoder::{AudioSettingsBuilder, ContainerSettingsBuilder, VideoEncoder, VideoSettingsBuilder},
    frame::Frame,
    graphics_capture_api::InternalCaptureControl,
    monitor::Monitor,
    settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    },
};

const SEG_SECS: u64 = 5;
/// Fotogramas por segundo del clip. UN solo numero: lo usa el codificador y lo
/// usa el marcapasos de `on_frame_arrived`. Que fueran dos cosas distintas es
/// justo lo que estaba roto.
///
/// El monitor primario de esta maquina va a 165 Hz, asi que WGC entrega ~165
/// fotogramas por segundo y hasta ahora TODOS se mandaban al codificador, que
/// estaba configurado a 60. Quien decidia cuales sobrevivian era el sumidero de
/// Media Foundation, y lo hace de la forma ingenua: se queda uno solo si su
/// marca de tiempo esta a mas de 1/60 s del ultimo que guardo. Con una fuente de
/// 165 Hz eso nunca se cumple a los 2 tics (12,1 ms < 16,7 ms), asi que siempre
/// espera 3 -> 165/3 = 55 fps. De ahi el "~55 fps" que ya estaba anotado en la
/// cabecera de este archivo sin explicacion, y los 51,9 unicos medidos sobre un
/// clip real de partida.
const TARGET_FPS: u32 = 60;
/// Periodo objetivo en unidades de 100 ns, que es lo que usa `SystemRelativeTime`.
const FRAME_PERIOD_100NS: i64 = 10_000_000 / TARGET_FPS as i64;
/// Si la captura se para (juego minimizado, pantalla en reposo) el vencimiento
/// se queda atras y al volver mandaria una rafaga intentando recuperar. Pasado
/// este hueco se resincroniza en vez de recuperar.
const PACER_RESYNC_100NS: i64 = 10_000_000; // 1 s
// 12 slots = ~60s of history for a 30s clip. It used to be 8 (~40s), which was
// enough only while every segment lasted the full SEG_SECS. Cut-on-demand ends
// segments early, so each save spends a slot on a partial one and the buffer
// shrinks; back-to-back saves eroded a 30s clip down to 19s (measured). Doubling
// the target absorbs that. Costs ~50 MB more on disk, nothing in CPU.
// Keep in sync with $RING in .config/shadowplay-wgc-save.ps1.
const RING: usize = 12;
/// Named event that asks for the current segment to be closed NOW.
///
/// Alt+F10 used to be worth 0-5 seconds of waiting: the save step could only use
/// segments that were already closed, so it either threw away everything since
/// the last boundary or sat waiting for one. Neither is necessary -- the boundary
/// swap here is O(1) thanks to the pre-warmed encoder, so it can happen on
/// demand. Measured: the outgoing MP4 is byte-complete ~80 ms after a boundary.
///
/// Auto-reset, same as `Global\rice-launcher-show`: one signal is exactly one cut.
const CUT_EVENT: &str = "Global\\rice-shadowplay-cut";
/// Written by the finisher after each `finish()`, so the saver knows the segment
/// it asked for is on disk instead of polling ffprobe until something parses.
const CUT_MARK: &str = "last-finished.txt";
/// A cut is ignored below this age. The maker keeps exactly ONE spare encoder,
/// so two cuts in quick succession would find `make_ready_rx` empty and block the
/// capture thread inside `VideoEncoder::new` (a synchronous PrepareTranscodeAsync
/// -- expensive enough that the whole maker thread exists to hide it). The cut is
/// not dropped, just deferred to this age.
const MIN_CUT_SECS: f32 = 1.0;
// HEVC target bitrate. 10 Mbps at 1080p60 is a good size/quality balance for a
// rolling replay buffer (also ~a third less continuous disk write than 15).
const BITRATE: u32 = 10_000_000;
// Fallback geometry if the monitor size query fails; real size comes from the
// primary monitor at startup (see main).
const FALLBACK_W: u32 = 1920;
const FALLBACK_H: u32 = 1080;

type Err = Box<dyn std::error::Error + Send + Sync>;

struct Flags {
    dir: String,
    audio_idx: Arc<AtomicUsize>,
    /// Puesta a true por el hilo que escucha CUT_EVENT.
    cut: Arc<AtomicBool>,
    w: u32,
    h: u32,
}

struct Rec {
    enc: Option<VideoEncoder>,
    seg_start: Instant,
    idx: usize,
    // Hand finished segments off the capture thread; finish() blocks ~1s.
    finish_tx: Sender<(VideoEncoder, usize)>,
    // Request the next segment's encoder and receive it pre-warmed.
    make_req_tx: Sender<usize>,
    make_ready_rx: Receiver<VideoEncoder>,
    // Shared with the audio thread so its PCM file rotates in lockstep with video.
    audio_idx: Arc<AtomicUsize>,
    // Raised from outside (Alt+F10) to close this segment early.
    cut: Arc<AtomicBool>,
    // Marcapasos: marca de tiempo a partir de la cual toca mandar el siguiente
    // fotograma. Avanza SIEMPRE un periodo exacto, nunca se reengancha a la
    // marca del fotograma que acaba de pasar -- esa es toda la diferencia entre
    // 60 fps y 55. Reengancharse haria que dos tics de una fuente de 165 Hz
    // (12,1 ms) nunca alcanzaran el periodo de 16,7 ms y siempre se esperaran 3.
    // Dejandolo avanzar en firme, el vencimiento se desliza dentro del tic y el
    // patron sale 3,3,3,2,3,3,3,2... que promedia 2,75 tics = 60,000 fps.
    next_due: Option<i64>,
    // Cuantos entran y cuantos salen, para poder decir el numero en vez de
    // suponerlo. Se vuelca cada SEG_SECS.
    seen: u32,
    sent: u32,
}

fn make_encoder(dir: &str, idx: usize, w: u32, h: u32) -> Result<VideoEncoder, Err> {
    let path = format!("{dir}\\seg{idx:02}.mp4");
    let _ = std::fs::remove_file(&path);
    Ok(VideoEncoder::new(
        VideoSettingsBuilder::new(w, h).frame_rate(TARGET_FPS).bitrate(BITRATE),
        // Audio muxed separately at save time (see file header) -> disable here.
        AudioSettingsBuilder::default().disabled(true),
        ContainerSettingsBuilder::default(),
        &path,
    )?)
}

// Background worker: finalize (flush + write moov) encoders handed to it. This is
// the ~1s blocking call that must stay off the capture callback thread.
fn spawn_finisher(dir: String) -> Sender<(VideoEncoder, usize)> {
    let (tx, rx) = channel::<(VideoEncoder, usize)>();
    std::thread::spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        while let Ok((enc, idx)) = rx.recv() {
            let t = Instant::now();
            match enc.finish() {
                Ok(()) => {
                    // Escribir-y-renombrar: quien guarda sondea este archivo y no
                    // puede leerlo a medio escribir.
                    let tmp = format!("{dir}\\{CUT_MARK}.tmp");
                    let dst = format!("{dir}\\{CUT_MARK}");
                    if std::fs::write(&tmp, format!("seg{idx:02}")).is_ok() {
                        let _ = std::fs::rename(&tmp, &dst);
                    }
                    eprintln!("seg{idx:02} finished in {} ms", t.elapsed().as_millis());
                }
                Err(e) => eprintln!("segment finish failed: {e}"),
            }
        }
    });
    tx
}

// Background worker: pre-build encoders for requested segment indices. Creating an
// encoder blocks on PrepareTranscodeAsync, so we do it ~5s ahead of the boundary.
fn spawn_maker(dir: String, w: u32, h: u32) -> (Sender<usize>, Receiver<VideoEncoder>) {
    let (req_tx, req_rx) = channel::<usize>();
    let (ready_tx, ready_rx) = channel::<VideoEncoder>();
    std::thread::spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        while let Ok(idx) = req_rx.recv() {
            // Cronometrado porque el unico numero que habia sobre esto en todo el
            // repo era un "~1s" en un comentario, sin medicion detras -- y de su
            // coste depende que un corte bajo demanda sea seguro.
            let t = Instant::now();
            match make_encoder(&dir, idx, w, h) {
                Ok(e) => {
                    eprintln!("seg{idx:02} encoder ready in {} ms", t.elapsed().as_millis());
                    if ready_tx.send(e).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("segment make failed: {e}");
                    break;
                }
            }
        }
    });
    (req_tx, ready_rx)
}

impl GraphicsCaptureApiHandler for Rec {
    type Flags = Flags;
    type Error = Err;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        let dir = ctx.flags.dir;
        let audio_idx = ctx.flags.audio_idx;
        let cut = ctx.flags.cut;
        let (w, h) = (ctx.flags.w, ctx.flags.h);
        std::fs::create_dir_all(&dir).ok();
        // Stale marker from a previous run: the saver would otherwise read it as
        // an answer to the cut it is about to ask for.
        let _ = std::fs::remove_file(format!("{dir}\\{CUT_MARK}"));
        // seg 0 built inline (capture isn't running yet, so no stall to hide).
        let enc = make_encoder(&dir, 0, w, h)?;
        let finish_tx = spawn_finisher(dir.clone());
        let (make_req_tx, make_ready_rx) = spawn_maker(dir, w, h);
        // Pre-warm seg 1 so the first boundary swap is instant.
        make_req_tx.send(1).ok();
        Ok(Self {
            enc: Some(enc),
            seg_start: Instant::now(),
            idx: 0,
            finish_tx,
            make_req_tx,
            make_ready_rx,
            audio_idx,
            cut,
            next_due: None,
            seen: 0,
            sent: 0,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _ctrl: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        // Marcapasos a TARGET_FPS. Sin esto se le mandaban al codificador los
        // ~165 fps del monitor y era el sumidero de MF quien elegia cuales
        // sobrevivian, con la regla ingenua que acaba dando 55. Ver TARGET_FPS.
        self.seen += 1;
        let ts = frame.timestamp()?.Duration;
        let due = *self.next_due.get_or_insert(ts);
        if ts < due {
            return Ok(()); // demasiado pronto para el objetivo: se descarta
        }
        self.next_due = Some(if ts - due > PACER_RESYNC_100NS {
            ts + FRAME_PERIOD_100NS // hubo parada larga: reenganchar
        } else {
            due + FRAME_PERIOD_100NS // avance en firme
        });
        self.sent += 1;
        self.enc.as_mut().unwrap().send_frame(frame)?;

        // A cut requested from outside (Alt+F10) closes the segment right here
        // instead of at the next 5s boundary. Same branch, same O(1) swap -- the
        // only reason this was ever worth 0-5 seconds of waiting is that nothing
        // could ask for it.
        let age = self.seg_start.elapsed();
        let asked = self.cut.load(Ordering::Relaxed);
        if age.as_secs() >= SEG_SECS || (asked && age.as_secs_f32() >= MIN_CUT_SECS) {
            // Cleared only when actually honoured: a cut arriving in the first
            // second stays pending and fires at MIN_CUT_SECS rather than being
            // silently dropped.
            if asked {
                self.cut.store(false, Ordering::Relaxed);
            }
            let secs = age.as_secs_f32().max(0.001);
            eprintln!(
                "seg{:02} {:.2}s  wgc {:.1}/s -> codificados {:.1}/s",
                self.idx,
                secs,
                self.seen as f32 / secs,
                self.sent as f32 / secs
            );
            self.seen = 0;
            self.sent = 0;
            // Swap in the pre-warmed encoder (requested ~5s ago -> recv is instant),
            // then hand the old one to the finisher. The whole boundary is O(1).
            let next = self.make_ready_rx.recv()?;
            let old = self.enc.replace(next).unwrap();
            let old_idx = self.idx;
            self.finish_tx.send((old, old_idx)).ok();
            self.idx = (self.idx + 1) % RING;
            // Move the audio PCM file to the same index (keeps A/V aligned).
            self.audio_idx.store(self.idx, Ordering::Relaxed);
            // Pre-warm the segment after this one for the next boundary.
            let following = (self.idx + 1) % RING;
            self.make_req_tx.send(following).ok();
            self.seg_start = Instant::now();
        }
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        if let Some(e) = self.enc.take() {
            let _ = e.finish();
        }
        Ok(())
    }
}

// Read s16le PCM from a sysaudio-loopback child and write it to a per-segment file,
// reopening a fresh one whenever the capture thread advances audio_idx at a segment
// boundary. mic=false captures system audio -> seg{idx}.pcm; mic=true captures the
// microphone (--mic) -> seg{idx}.mic.pcm. Both are wallclock-paced (silence-filled
// when idle), so they line up with the video and with each other; the save step
// mixes them. Mixing at save (not live) is why the mic can't drag the framerate.
fn spawn_audio(dir: String, audio_idx: Arc<AtomicUsize>, mic: bool) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let ext = if mic { "mic.pcm" } else { "pcm" };
    let lb = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.join("sysaudio-loopback.exe")))
        .unwrap_or_else(|| "sysaudio-loopback.exe".into());
    std::thread::spawn(move || {
        let mut cmd = std::process::Command::new(lb);
        cmd.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW);
        if mic {
            cmd.arg("--mic");
        }
        let Ok(mut child) = cmd.spawn() else {
            eprintln!("could not start {}", if mic { "mic" } else { "loopback" });
            return;
        };
        let Some(mut out) = child.stdout.take() else { return };
        let mut cur = usize::MAX;
        let mut file: Option<std::fs::File> = None;
        let mut buf = [0u8; 4096];
        loop {
            match out.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let want = audio_idx.load(Ordering::Relaxed);
                    if want != cur {
                        file = std::fs::File::create(format!("{dir}\\seg{want:02}.{ext}")).ok();
                        cur = want;
                    }
                    if let Some(f) = file.as_mut() {
                        let _ = f.write_all(&buf[..n]);
                    }
                }
            }
        }
    });
}

// Single instance. Every other rice binary has this; without it two recorders
// would write the same rolling buffer and run the hardware encoder twice, and a
// stray double-launch (Startup shortcut + supervisor) is enough to cause it.
fn claim_single_instance() -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;
    let name: Vec<u16> = "Global\\shadowplay-wgc\0".encode_utf16().collect();
    unsafe {
        let _ = CreateMutexW(None, false, PCWSTR(name.as_ptr()));
        GetLastError() != ERROR_ALREADY_EXISTS
    }
}

fn main() {
    if !claim_single_instance() {
        eprintln!("another shadowplay-wgc is already recording");
        return;
    }
    let dir = std::env::args().nth(1).unwrap_or_else(|| {
        format!(
            "{}\\ShadowPlay\\wgc-buffer",
            std::env::var("USERPROFILE").unwrap_or_default()
        )
    });
    std::fs::create_dir_all(&dir).ok();

    let audio_idx = Arc::new(AtomicUsize::new(0));
    spawn_audio(dir.clone(), audio_idx.clone(), false); // system audio -> seg*.pcm
    spawn_audio(dir.clone(), audio_idx.clone(), true); //  microphone   -> seg*.mic.pcm

    // Cut-on-demand listener. Costs nothing while idle: parked in
    // WaitForSingleObject(INFINITE), never scheduled until someone signals.
    let cut = Arc::new(AtomicBool::new(false));
    match rice_common::win::NamedEvent::create(CUT_EVENT) {
        Some(ev) => {
            let flag = cut.clone();
            std::thread::spawn(move || loop {
                if !ev.wait() {
                    // A bad handle returns instantly, forever. Sleeping keeps
                    // that from becoming a spin loop (same guard as launcher).
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    continue;
                }
                flag.store(true, Ordering::Relaxed);
            });
        }
        // Not fatal: rotation still happens every SEG_SECS, so Alt+F10 degrades
        // to the old behaviour instead of the recorder refusing to run.
        None => eprintln!("could not create {CUT_EVENT}; cuts on demand disabled"),
    }

    let monitor = Monitor::primary().expect("no primary monitor");
    // Match the encoder to the monitor's real resolution so frames never hit the
    // crate's padded-surface fallback (an extra per-frame GPU copy).
    let w = monitor.width().unwrap_or(FALLBACK_W);
    let h = monitor.height().unwrap_or(FALLBACK_H);
    // El aviso de arriba, en codigo: si la propiedad no existe (Windows
    // anterior), crear la sesion con Custom falla entera, asi que se comprueba.
    let min_update = if windows_capture::graphics_capture_api::GraphicsCaptureApi::
        is_minimum_update_interval_supported()
        .unwrap_or(false)
    {
        MinimumUpdateIntervalSettings::Custom(std::time::Duration::from_micros(100))
    } else {
        eprintln!("MinUpdateInterval no soportado: el techo se queda en refresco/3");
        MinimumUpdateIntervalSettings::Default
    };
    let settings = Settings::new(
        monitor,
        CursorCaptureSettings::Default,
        DrawBorderSettings::WithoutBorder,
        SecondaryWindowSettings::Default,
        // NO Default. Ese es el techo de 55 fps.
        //
        // Medido: con Default, WGC entrega exactamente 55,0 fotogramas por
        // segundo, tres segmentos seguidos sin variacion, con el escritorio
        // cambiando a tope. 55,0 = 165/3, y el monitor primario va a 165 Hz. El
        // intervalo minimo por defecto es 1/60 s, y en una pantalla de 165 Hz el
        // primer refresco que pasa de 16,67 ms es el TERCERO (18,18 ms), asi que
        // el techo cae a 165/3 en vez de quedarse en 60.
        //
        // Con el minimo casi a cero WGC entrega los 165 y el marcapasos de
        // on_frame_arrived elige 60 exactos. Los otros dos tercios se descartan
        // antes de tocar el codificador, asi que no cuestan codificacion.
        min_update,
        DirtyRegionSettings::Default,
        ColorFormat::Rgba8,
        Flags { dir, audio_idx, cut, w, h },
    );
    eprintln!("wgc recorder (video + parallel-pcm audio) started");
    Rec::start(settings).expect("capture failed");
}
