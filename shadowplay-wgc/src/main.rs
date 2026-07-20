// WGC rolling-buffer recorder with audio. Captures the primary monitor via
// Windows.Graphics.Capture, hardware-encodes to a ring of short HEVC MP4
// segments, and muxes system audio pulled from the sysaudio-loopback helper
// (WASAPI loopback, s16le 48kHz stereo) via the encoder's send_audio_buffer.
// This is the OBS approach: WGC video + WASAPI audio, muxed.
// Usage: shadowplay-wgc [buffer_dir]

use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::Instant;
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
const LOOPBACK: &str = r"C:\Users\obisp\dev\glaze-bar\target\release\sysaudio-loopback.exe";

type Err = Box<dyn std::error::Error + Send + Sync>;

struct Flags {
    dir: String,
    audio: Arc<Mutex<Vec<u8>>>,
}

struct Rec {
    enc: Option<VideoEncoder>,
    audio: Arc<Mutex<Vec<u8>>>,
    seg_start: Instant,
    idx: usize,
    dir: String,
    w: u32,
    h: u32,
}

fn make_encoder(dir: &str, idx: usize, w: u32, h: u32) -> Result<VideoEncoder, Err> {
    let path = format!("{dir}\\seg{idx:02}.mp4");
    let _ = std::fs::remove_file(&path);
    Ok(VideoEncoder::new(
        VideoSettingsBuilder::new(w, h).frame_rate(60),
        // Input PCM from the loopback is s16le 48kHz stereo.
        AudioSettingsBuilder::default()
            .sample_rate(48000)
            .channel_count(2)
            .bit_per_sample(16),
        ContainerSettingsBuilder::default(),
        &path,
    )?)
}

impl GraphicsCaptureApiHandler for Rec {
    type Flags = Flags;
    type Error = Err;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        let dir = ctx.flags.dir;
        let audio = ctx.flags.audio;
        std::fs::create_dir_all(&dir).ok();
        let (w, h) = (1920, 1080);
        let enc = make_encoder(&dir, 0, w, h)?;
        Ok(Self {
            enc: Some(enc),
            audio,
            seg_start: Instant::now(),
            idx: 0,
            dir,
            w,
            h,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _ctrl: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        // Feed the system audio accumulated since the last frame, then the frame.
        let chunk = {
            let mut q = self.audio.lock().unwrap();
            std::mem::take(&mut *q)
        };
        if !chunk.is_empty() {
            self.enc.as_mut().unwrap().send_audio_buffer(&chunk, 0)?;
        }
        self.enc.as_mut().unwrap().send_frame(frame)?;

        if self.seg_start.elapsed().as_secs() >= SEG_SECS {
            self.enc.take().unwrap().finish()?;
            self.idx = (self.idx + 1) % RING;
            self.enc = Some(make_encoder(&self.dir, self.idx, self.w, self.h)?);
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

// Read s16le PCM from the loopback child into the shared queue.
fn spawn_audio(audio: Arc<Mutex<Vec<u8>>>) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::thread::spawn(move || {
        let Ok(mut child) = std::process::Command::new(LOOPBACK)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
        else {
            eprintln!("could not start loopback");
            return;
        };
        let Some(mut out) = child.stdout.take() else { return };
        let mut buf = [0u8; 16384];
        loop {
            match out.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let mut q = audio.lock().unwrap();
                    // Cap the queue so a stalled video callback can't grow it forever.
                    if q.len() < 1_920_000 {
                        q.extend_from_slice(&buf[..n]);
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

    let audio = Arc::new(Mutex::new(Vec::<u8>::new()));
    spawn_audio(audio.clone());

    let monitor = Monitor::primary().expect("no primary monitor");
    let settings = Settings::new(
        monitor,
        CursorCaptureSettings::Default,
        DrawBorderSettings::WithoutBorder,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Default,
        DirtyRegionSettings::Default,
        ColorFormat::Rgba8,
        Flags { dir, audio },
    );
    eprintln!("wgc recorder (video+audio) started");
    Rec::start(settings).expect("capture failed");
}
