// WGC rolling-buffer recorder. Captures the primary monitor via
// Windows.Graphics.Capture and hardware-encodes to a ring of short HEVC MP4
// segments. System audio AND the microphone (WASAPI via two sysaudio-loopback
// helpers, s16le 48kHz stereo) go to parallel rings of raw PCM, and are mixed
// with the video at save time.
//
// ALL THREE RINGS LIVE IN RAM and only reach the disk when a cut asks for them.
// The buffer used to write ~1.5 MB/s continuously -- 127 GB a day -- to keep 60
// seconds of history for a feature used a handful of times a day. Measured after
// the move: 0.1 KB/s at rest, and a cut dumps all 12 segments in 56 ms. It costs
// ~150 MB of RAM (388 against 240 MB private).
//
// The dump writes plain seg{NN}.mp4 / .pcm / .mic.pcm files with each segment's
// real close time as the mtime, so the save script sees exactly what it saw when
// the ring was on disk and did not have to change. That mtime is not cosmetic:
// the saver derives each segment's duration from the gap between two of them.
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

use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Instant, SystemTime};
use windows::Storage::Streams::{DataReader, IRandomAccessStream, InMemoryRandomAccessStream};
use windows::core::Interface;
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
// Nueve y no doce. El anillo guardaba 60 s (12 x 5 s) mientras el guardado
// solo pide 30 -- `$OBJETIVO` en shadowplay-wgc-save.ps1 -- asi que la mitad
// del anillo de video era RAM que no se llegaba a guardar nunca.
//
// La cuenta de nueve: seis segmentos completos hacen los 30 s, el septimo es el
// que se corta a media vida al pedir el corte, el octavo es el hueco que el
// grabador deja pre-vaciado, y el noveno es margen. Medido antes de tocarlo: un
// clip real de 31,6 s uso siete de los doce.
const RING: usize = 9;
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
    ring: Ring,
    audio: AudioRing,
    w: u32,
    h: u32,
}

struct Rec {
    enc: Option<VideoEncoder>,
    seg_start: Instant,
    idx: usize,
    // Hand finished segments off the capture thread; finish() blocks ~1s.
    finish_tx: Sender<Done>,
    // Request the next segment's encoder and receive it pre-warmed.
    make_req_tx: Sender<usize>,
    make_ready_rx: Receiver<(VideoEncoder, SendStream)>,
    // Flujo en RAM donde escribe el codificador actual.
    stream: SendStream,
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

/// Carries a WinRT stream between threads.
///
/// windows-rs 0.62 does not mark interface pointers `Send`, and rightly so in
/// general. It is sound HERE for a specific reason: only two threads ever CALL a
/// method on the stream -- the maker (which creates it) and the finisher (which
/// reads it out) -- and both run `CoInitializeEx(MTA)`. Inside one MTA any
/// object is callable from any thread with no marshalling. The capture thread
/// touches the handle but never calls through it: it only moves it from the
/// maker's channel to the finisher's, and moving a pointer is not a call.
///
/// The crate does the same thing for its D3D surfaces (`SendDirectX` in
/// encoder.rs) for the same reason.
struct SendStream(IRandomAccessStream);
unsafe impl Send for SendStream {}

/// A finished segment, still in RAM.
struct Seg {
    /// The complete MP4, moov included. Bytes and not the stream: the finisher
    /// reads it out once, right after `finish()`, instead of every cut re-reading
    /// all twelve streams through a `DataReader`.
    bytes: Vec<u8>,
    /// When it was closed. Written back as the file's mtime on dump, because the
    /// save script derives each segment's duration from the gap between two
    /// mtimes; dumping the whole ring at once would otherwise stamp them all with
    /// the same instant and collapse a 30s clip down to one segment.
    closed: SystemTime,
}

/// The ring, in RAM. `None` is a slot not filled yet (cold start) or one whose
/// encoder is currently writing into it.
type Ring = Arc<std::sync::Mutex<Vec<Option<Seg>>>>;

fn make_encoder(w: u32, h: u32) -> Result<(VideoEncoder, SendStream), Err> {
    // In RAM, not on disk. This buffer used to write ~1.5 MB/s continuously --
    // 127 GB a day -- for a feature used a handful of times a day. Nothing
    // reaches the disk now until a cut asks for it.
    let stream: IRandomAccessStream = InMemoryRandomAccessStream::new()?.cast()?;
    let enc = VideoEncoder::new_from_stream(
        VideoSettingsBuilder::new(w, h).frame_rate(TARGET_FPS).bitrate(BITRATE),
        // Audio muxed separately at save time (see file header) -> disable here.
        AudioSettingsBuilder::default().disabled(true),
        ContainerSettingsBuilder::default(),
        stream.clone(),
    )?;
    Ok((enc, SendStream(stream)))
}

/// Read a finished stream out to a `Vec`. The crate itself shows this idiom in
/// `encoder.rs`; there is no cheaper way -- a WinRT stream does not expose its
/// backing buffer.
fn stream_bytes(s: &IRandomAccessStream) -> Result<Vec<u8>, Err> {
    let size = s.Size()?;
    let reader = DataReader::CreateDataReader(&s.GetInputStreamAt(0)?)?;
    reader.LoadAsync(size as u32)?.join()?;
    let mut bytes = vec![0u8; size as usize];
    reader.ReadBytes(&mut bytes)?;
    Ok(bytes)
}

/// Write the whole ring to disk as `segNN.mp4`, plus the audio rings as
/// `segNN.pcm` / `segNN.mic.pcm`. Only ever called on a cut.
///
/// Slots holding nothing get their stale file removed, so the save script can
/// never pick up a segment left over from a previous lap of the ring.
fn dump_ring(dir: &str, ring: &Ring, audio: &AudioRing) -> usize {
    let mut written = 0;
    // El audio se COPIA y se suelta el cerrojo antes de tocar el disco.
    //
    // Los dos hilos de audio escriben en ese mismo cerrojo cada vez que el tubo
    // les entrega un bloque; retenerlo durante los ~56 ms de escritura los dejaba
    // bloqueados todo ese rato. Sobrevivia porque 56 ms son unos 10 KB y el tubo
    // aguanta, pero era margen prestado. La copia son ~23 MB y ~5 ms.
    //
    // El anillo de video no necesita lo mismo: su cerrojo solo lo toca este hilo.
    let a: Audio = {
        let g = audio.lock().unwrap();
        Audio { sys: g.sys.clone(), mic: g.mic.clone() }
    };
    let guard = ring.lock().unwrap();
    for (i, slot) in guard.iter().enumerate() {
        let mp4 = format!("{dir}\\seg{i:02}.mp4");
        let pcm = format!("{dir}\\seg{i:02}.pcm");
        let mic = format!("{dir}\\seg{i:02}.mic.pcm");
        match slot {
            Some(seg) => {
                if std::fs::write(&mp4, &seg.bytes).is_ok() {
                    // La fecha ES el dato del que el script de guardado saca la
                    // duracion. Sin esto los doce archivos saldrian con la misma
                    // y el clip quedaria en un solo segmento.
                    if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&mp4) {
                        let _ = f.set_times(std::fs::FileTimes::new().set_modified(seg.closed));
                    }
                    written += 1;
                }
                let _ = std::fs::write(&pcm, &a.sys[i]);
                let _ = std::fs::write(&mic, &a.mic[i]);
            }
            None => {
                let _ = std::fs::remove_file(&mp4);
                let _ = std::fs::remove_file(&pcm);
                let _ = std::fs::remove_file(&mic);
            }
        }
    }
    written
}

/// The audio rings, in RAM alongside the video one. `sys` is system audio,
/// `mic` the microphone; both s16le 48k stereo, one `Vec` per segment slot.
struct Audio {
    sys: Vec<Vec<u8>>,
    mic: Vec<Vec<u8>>,
}
type AudioRing = Arc<std::sync::Mutex<Audio>>;

/// What the capture thread hands over at a boundary.
struct Done {
    enc: VideoEncoder,
    idx: usize,
    stream: SendStream,
    /// True when this boundary came from Alt+F10 rather than the 5s timer, and
    /// so the ring has to reach the disk.
    cut: bool,
}

// Background worker: finalize (flush + write moov) encoders handed to it. This is
// the ~1s blocking call that must stay off the capture callback thread.
fn spawn_finisher(dir: String, ring: Ring, audio: AudioRing) -> Sender<Done> {
    let (tx, rx) = channel::<Done>();
    std::thread::spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        while let Ok(d) = rx.recv() {
            let t = Instant::now();
            match d.enc.finish() {
                Ok(()) => {
                    // El flujo ya tiene el MP4 entero, atomo moov incluido.
                    match stream_bytes(&d.stream.0) {
                        Ok(bytes) => {
                            if let Ok(mut g) = ring.lock() {
                                g[d.idx] = Some(Seg { bytes, closed: SystemTime::now() });
                            }
                        }
                        Err(e) => eprintln!("seg{:02} read from RAM failed: {e}", d.idx),
                    }
                    let fin = t.elapsed().as_millis();
                    if d.cut {
                        // Solo aqui se toca el disco.
                        let w = Instant::now();
                        let n = dump_ring(&dir, &ring, &audio);
                        // Escribir-y-renombrar: quien guarda sondea este archivo
                        // y no puede leerlo a medio escribir. Va DESPUES del
                        // volcado: es la senal de que el anillo ya esta en disco.
                        let tmp = format!("{dir}\\{CUT_MARK}.tmp");
                        let dst = format!("{dir}\\{CUT_MARK}");
                        if std::fs::write(&tmp, format!("seg{:02}", d.idx)).is_ok() {
                            let _ = std::fs::rename(&tmp, &dst);
                        }
                        eprintln!(
                            "seg{:02} finished in {fin} ms; dumped {n} segments in {} ms",
                            d.idx,
                            w.elapsed().as_millis()
                        );
                    } else {
                        eprintln!("seg{:02} finished in {fin} ms", d.idx);
                    }
                }
                Err(e) => eprintln!("segment finish failed: {e}"),
            }
        }
    });
    tx
}

// Background worker: pre-build encoders for requested segment indices. Creating an
// encoder blocks on PrepareTranscodeAsync, so we do it ~5s ahead of the boundary.
fn spawn_maker(w: u32, h: u32) -> (Sender<usize>, Receiver<(VideoEncoder, SendStream)>) {
    let (req_tx, req_rx) = channel::<usize>();
    let (ready_tx, ready_rx) = channel::<(VideoEncoder, SendStream)>();
    std::thread::spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        while let Ok(idx) = req_rx.recv() {
            // Cronometrado porque el unico numero que habia sobre esto en todo el
            // repo era un "~1s" en un comentario, sin medicion detras -- y de su
            // coste depende que un corte bajo demanda sea seguro.
            let t = Instant::now();
            match make_encoder(w, h) {
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
        // Con el anillo en RAM, los archivos del arranque anterior sobran y solo
        // pueden confundir al que guarda: se limpian de una.
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let n = e.file_name();
                let n = n.to_string_lossy();
                if n.starts_with("seg") {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        // seg 0 built inline (capture isn't running yet, so no stall to hide).
        let (enc, stream) = make_encoder(w, h)?;
        let finish_tx = spawn_finisher(dir.clone(), ctx.flags.ring, ctx.flags.audio);
        let (make_req_tx, make_ready_rx) = spawn_maker(w, h);
        // Pre-warm seg 1 so the first boundary swap is instant.
        make_req_tx.send(1).ok();
        Ok(Self {
            enc: Some(enc),
            stream,
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
            let (next, next_stream) = self.make_ready_rx.recv()?;
            let old = self.enc.replace(next).unwrap();
            let old_stream = std::mem::replace(&mut self.stream, next_stream);
            let old_idx = self.idx;
            self.finish_tx
                .send(Done { enc: old, idx: old_idx, stream: old_stream, cut: asked })
                .ok();
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

// Read s16le PCM from a sysaudio-loopback child into the RAM ring, moving to a
// fresh slot whenever the capture thread advances audio_idx at a segment
// boundary. mic=false captures system audio; mic=true captures the microphone
// (--mic). Both are wallclock-paced (silence-filled when idle), so they line up
// with the video and with each other; the save step mixes them. Mixing at save
// (not live) is why the mic can't drag the framerate.
//
// A slot is a `Vec` that gets cleared, not reallocated: after one lap of the ring
// every slot has grown to a full segment's worth (~960 KB) and stays there, so
// the steady state does no allocation at all.
fn spawn_audio(audio: AudioRing, audio_idx: Arc<AtomicUsize>, mic: bool) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
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
        let mut buf = [0u8; 4096];
        loop {
            match out.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let want = audio_idx.load(Ordering::Relaxed);
                    let Ok(mut g) = audio.lock() else { break };
                    let ring = if mic { &mut g.mic } else { &mut g.sys };
                    if want != cur {
                        // Slot nuevo: se vacia, conservando su capacidad.
                        ring[want].clear();
                        cur = want;
                    }
                    ring[want].extend_from_slice(&buf[..n]);
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
    // Los tres anillos viven en RAM y solo se vuelcan a disco en un corte.
    let ring: Ring = Arc::new(std::sync::Mutex::new((0..RING).map(|_| None).collect()));
    let audio: AudioRing = Arc::new(std::sync::Mutex::new(Audio {
        sys: vec![Vec::new(); RING],
        mic: vec![Vec::new(); RING],
    }));
    spawn_audio(audio.clone(), audio_idx.clone(), false); // audio del sistema
    spawn_audio(audio.clone(), audio_idx.clone(), true); //  microfono

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
        Flags { dir, audio_idx, cut, ring, audio, w, h },
    );
    eprintln!("wgc recorder (video + parallel-pcm audio) started");
    Rec::start(settings).expect("capture failed");
}
