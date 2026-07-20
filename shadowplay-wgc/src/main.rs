// WGC rolling-buffer recorder: captures the primary monitor via Windows.Graphics
// .Capture and hardware-encodes to a ring of short MP4 segments (HEVC). A save
// step concats the last N segments into a replay clip. Rotates the encoder every
// SEG_SECS so each segment starts on a keyframe -> concat -c copy works.
// Usage: shadowplay-wgc [buffer_dir]   (default ~/ShadowPlay/wgc-buffer)

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

type Err = Box<dyn std::error::Error + Send + Sync>;

struct Rec {
    enc: Option<VideoEncoder>,
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
        AudioSettingsBuilder::default().disabled(true), // WGC monitor audio is empty; mux our loopback on save instead
        ContainerSettingsBuilder::default(),
        &path,
    )?)
}

impl GraphicsCaptureApiHandler for Rec {
    type Flags = String; // buffer dir
    type Error = Err;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        let dir = ctx.flags;
        std::fs::create_dir_all(&dir).ok();
        let (w, h) = (1920, 1080);
        let enc = make_encoder(&dir, 0, w, h)?;
        Ok(Self {
            enc: Some(enc),
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
        self.enc.as_mut().unwrap().send_frame(frame)?;
        if self.seg_start.elapsed().as_secs() >= SEG_SECS {
            // Finalize the current segment and open the next slot in the ring.
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

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| {
        format!(
            "{}\\ShadowPlay\\wgc-buffer",
            std::env::var("USERPROFILE").unwrap_or_default()
        )
    });
    std::fs::create_dir_all(&dir).ok();
    let monitor = Monitor::primary().expect("no primary monitor");
    let settings = Settings::new(
        monitor,
        CursorCaptureSettings::Default,
        DrawBorderSettings::WithoutBorder, // no yellow WGC recording border
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Default,
        DirtyRegionSettings::Default,
        ColorFormat::Rgba8,
        dir,
    );
    eprintln!("wgc rolling recorder started");
    Rec::start(settings).expect("capture failed");
}
