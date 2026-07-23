// WGC rolling-buffer recorder. Captures the primary monitor via
// Windows.Graphics.Capture and hardware-encodes to a ring of short HEVC MP4
// segments. System audio (WASAPI loopback via the sysaudio-loopback helper,
// s16le 48kHz stereo) is written to a parallel ring of raw PCM files and muxed
// with the video only at save time.
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
use std::sync::atomic::{AtomicUsize, Ordering};
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
const RING: usize = 8;
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
    w: u32,
    h: u32,
}

struct Rec {
    enc: Option<VideoEncoder>,
    seg_start: Instant,
    idx: usize,
    // Hand finished segments off the capture thread; finish() blocks ~1s.
    finish_tx: Sender<VideoEncoder>,
    // Request the next segment's encoder and receive it pre-warmed.
    make_req_tx: Sender<usize>,
    make_ready_rx: Receiver<VideoEncoder>,
    // Shared with the audio thread so its PCM file rotates in lockstep with video.
    audio_idx: Arc<AtomicUsize>,
}

fn make_encoder(dir: &str, idx: usize, w: u32, h: u32) -> Result<VideoEncoder, Err> {
    let path = format!("{dir}\\seg{idx:02}.mp4");
    let _ = std::fs::remove_file(&path);
    Ok(VideoEncoder::new(
        VideoSettingsBuilder::new(w, h).frame_rate(60).bitrate(BITRATE),
        // Audio muxed separately at save time (see file header) -> disable here.
        AudioSettingsBuilder::default().disabled(true),
        ContainerSettingsBuilder::default(),
        &path,
    )?)
}

// Background worker: finalize (flush + write moov) encoders handed to it. This is
// the ~1s blocking call that must stay off the capture callback thread.
fn spawn_finisher() -> Sender<VideoEncoder> {
    let (tx, rx) = channel::<VideoEncoder>();
    std::thread::spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        while let Ok(enc) = rx.recv() {
            if let Err(e) = enc.finish() {
                eprintln!("segment finish failed: {e}");
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
            match make_encoder(&dir, idx, w, h) {
                Ok(e) => {
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
        let (w, h) = (ctx.flags.w, ctx.flags.h);
        std::fs::create_dir_all(&dir).ok();
        // seg 0 built inline (capture isn't running yet, so no stall to hide).
        let enc = make_encoder(&dir, 0, w, h)?;
        let finish_tx = spawn_finisher();
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
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _ctrl: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        self.enc.as_mut().unwrap().send_frame(frame)?;

        if self.seg_start.elapsed().as_secs() >= SEG_SECS {
            // Swap in the pre-warmed encoder (requested ~5s ago -> recv is instant),
            // then hand the old one to the finisher. The whole boundary is O(1).
            let next = self.make_ready_rx.recv()?;
            let old = self.enc.replace(next).unwrap();
            self.finish_tx.send(old).ok();
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

// Read s16le PCM from the loopback child and write it to seg{idx}.pcm, reopening a
// fresh file whenever the capture thread advances audio_idx at a segment boundary.
fn spawn_audio(dir: String, audio_idx: Arc<AtomicUsize>) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let lb = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.join("sysaudio-loopback.exe")))
        .unwrap_or_else(|| "sysaudio-loopback.exe".into());
    std::thread::spawn(move || {
        let Ok(mut child) = std::process::Command::new(lb)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
        else {
            eprintln!("could not start loopback");
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
                        file = std::fs::File::create(format!("{dir}\\seg{want:02}.pcm")).ok();
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

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| {
        format!(
            "{}\\ShadowPlay\\wgc-buffer",
            std::env::var("USERPROFILE").unwrap_or_default()
        )
    });
    std::fs::create_dir_all(&dir).ok();

    let audio_idx = Arc::new(AtomicUsize::new(0));
    spawn_audio(dir.clone(), audio_idx.clone());

    let monitor = Monitor::primary().expect("no primary monitor");
    // Match the encoder to the monitor's real resolution so frames never hit the
    // crate's padded-surface fallback (an extra per-frame GPU copy).
    let w = monitor.width().unwrap_or(FALLBACK_W);
    let h = monitor.height().unwrap_or(FALLBACK_H);
    let settings = Settings::new(
        monitor,
        CursorCaptureSettings::Default,
        DrawBorderSettings::WithoutBorder,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Default,
        DirtyRegionSettings::Default,
        ColorFormat::Rgba8,
        Flags { dir, audio_idx, w, h },
    );
    eprintln!("wgc recorder (video + parallel-pcm audio) started");
    Rec::start(settings).expect("capture failed");
}
