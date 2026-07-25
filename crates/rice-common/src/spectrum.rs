//! A tiny live audio spectrum, for drawing a few bars inside the island.
//!
//! Same source as cava -- the sibling `sysaudio-loopback.exe`, which writes
//! s16le 48 kHz stereo on stdout -- but reduced to a handful of bands and a
//! small FFT, since the island shows something like eight bars a few pixels
//! wide, not a full-width visualiser.
//!
//! Everything runs on its own thread; `Spectrum::levels()` is a cheap read of
//! the latest published values.

use std::io::Read;
use std::sync::{Arc, Mutex};

const FFT_SIZE: usize = 1024;
const SAMPLE_RATE: f32 = 48000.0;
const MIN_FREQ: f32 = 40.0;
const MAX_FREQ: f32 = 12000.0;
/// Temporal smoothing: how much of the previous frame is kept.
const SMOOTH: f32 = 0.55;
/// Decay applied when the reader stalls, so bars fall instead of freezing.
const DECAY: f32 = 0.90;

pub struct Spectrum {
    levels: Arc<Mutex<Vec<f32>>>,
    /// Kept so the helper is killed when the bar exits rather than orphaned.
    child: Arc<Mutex<Option<std::process::Child>>>,
}

impl Spectrum {
    /// Start capturing. `bands` is how many bars to publish.
    pub fn start(bands: usize) -> Self {
        let levels = Arc::new(Mutex::new(vec![0.0; bands]));
        let child: Arc<Mutex<Option<std::process::Child>>> = Arc::new(Mutex::new(None));
        let out = levels.clone();
        let child2 = child.clone();
        std::thread::spawn(move || {
            let exe = crate::win::sibling_exe("sysaudio-loopback.exe");
            loop {
                use std::os::windows::process::CommandExt;
                let spawned = std::process::Command::new(&exe)
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::null())
                    .creation_flags(crate::win::CREATE_NO_WINDOW)
                    .spawn();
                let Ok(mut ch) = spawned else {
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    continue;
                };
                let Some(mut stdout) = ch.stdout.take() else { continue };
                *child2.lock().unwrap() = Some(ch);

                let mut planner = rustfft::FftPlanner::<f32>::new();
                let fft = planner.plan_fft_forward(FFT_SIZE);
                let hann: Vec<f32> = (0..FFT_SIZE)
                    .map(|i| {
                        0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (FFT_SIZE as f32 - 1.0)).cos()
                    })
                    .collect();
                let mut ring = vec![0f32; FFT_SIZE];
                let mut raw = vec![0u8; FFT_SIZE * 4]; // s16 stereo
                let mut scratch: Vec<rustfft::num_complex::Complex<f32>> =
                    vec![rustfft::num_complex::Complex::new(0.0, 0.0); FFT_SIZE];
                let mut smoothed = vec![0f32; bands];
                let mut agc = 1.0f32;

                loop {
                    if stdout.read_exact(&mut raw).is_err() {
                        break; // helper died -> respawn
                    }
                    // Downmix to mono into the ring.
                    for i in 0..FFT_SIZE {
                        let l = i16::from_le_bytes([raw[i * 4], raw[i * 4 + 1]]) as f32 / 32768.0;
                        let r = i16::from_le_bytes([raw[i * 4 + 2], raw[i * 4 + 3]]) as f32 / 32768.0;
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

                // Reader broke: let the bars fall rather than freeze mid-air.
                {
                    let mut l = out.lock().unwrap();
                    for v in l.iter_mut() {
                        *v *= DECAY;
                    }
                }
                if let Some(mut c) = child2.lock().unwrap().take() {
                    let _ = c.kill();
                }
                std::thread::sleep(std::time::Duration::from_millis(800));
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
