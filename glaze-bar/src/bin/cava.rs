// Terminal audio spectrum visualizer (CAVA-style) for Windows. Pulls system
// audio from the sibling sysaudio-loopback.exe (WASAPI loopback, s16le 48 kHz
// stereo), runs a windowed FFT, and draws log-frequency bars with a vertical
// truecolor gradient. Targets 165 fps. Quit: q / Esc / Ctrl+C.

use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use rustfft::{num_complex::Complex, FftPlanner};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SAMPLE_RATE: f32 = 48000.0;
const FFT_SIZE: usize = 2048;
const MIN_FREQ: f32 = 30.0;
const MAX_FREQ: f32 = 16000.0;
const TARGET_FPS: u64 = 165;
const GRAVITY: f32 = 0.86; // fraction a bar keeps each frame while falling
const BAR_W: usize = 2;
const GAP: usize = 1;

#[link(name = "winmm")]
extern "system" {
    fn timeBeginPeriod(uperiod: u32) -> u32;
}

fn main() {
    // Windows sleep granularity is ~15ms by default -> would cap us near 64 fps.
    // Raise the timer resolution to 1ms so the frame pacing can hit 165 fps.
    unsafe {
        timeBeginPeriod(1);
    }

    let ring = Arc::new(Mutex::new(vec![0f32; FFT_SIZE]));
    spawn_audio(ring.clone());

    let mut stdout = std::io::stdout();
    let _ = terminal::enable_raw_mode();
    let _ = execute!(stdout, EnterAlternateScreen, Hide);
    // Restore the terminal even on panic (panic=abort still runs the hook).
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let mut so = std::io::stdout();
        let _ = execute!(so, Show, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
        prev(info);
    }));

    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);
    let hann: Vec<f32> = (0..FFT_SIZE)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (FFT_SIZE as f32 - 1.0)).cos())
        .collect();
    let blocks = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

    let frame_time = Duration::from_micros(1_000_000 / TARGET_FPS);
    let mut bars: Vec<f32> = Vec::new();
    let mut agc = 1.0f32;

    'main: loop {
        let t0 = Instant::now();

        // Quit keys.
        while event::poll(Duration::ZERO).unwrap_or(false) {
            if let Ok(Event::Key(k)) = event::read() {
                let quit = matches!(k.code, KeyCode::Char('q') | KeyCode::Esc)
                    || (k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL));
                if quit {
                    break 'main;
                }
            }
        }

        let (cols, rows) = terminal::size().unwrap_or((80, 24));
        let (cols, rows) = (cols as usize, rows as usize);
        if cols < 4 || rows < 2 {
            std::thread::sleep(frame_time);
            continue;
        }
        let unit = BAR_W + GAP;
        let nbars = ((cols + GAP) / unit).max(1);
        if bars.len() != nbars {
            bars = vec![0.0; nbars];
        }

        // FFT of the latest window.
        let samples = { ring.lock().unwrap().clone() };
        let mut buf: Vec<Complex<f32>> = (0..FFT_SIZE)
            .map(|i| Complex::new(samples.get(i).copied().unwrap_or(0.0) * hann[i], 0.0))
            .collect();
        fft.process(&mut buf);
        let mags: Vec<f32> = buf[..FFT_SIZE / 2].iter().map(|c| c.norm()).collect();

        // Log-frequency bins -> bar targets, with fast-attack / gravity-release.
        let mut frame_peak = 0.0f32;
        for b in 0..nbars {
            let f_lo = MIN_FREQ * (MAX_FREQ / MIN_FREQ).powf(b as f32 / nbars as f32);
            let f_hi = MIN_FREQ * (MAX_FREQ / MIN_FREQ).powf((b + 1) as f32 / nbars as f32);
            let bin_lo = ((f_lo / SAMPLE_RATE * FFT_SIZE as f32) as usize).max(1);
            let bin_hi = ((f_hi / SAMPLE_RATE * FFT_SIZE as f32) as usize)
                .max(bin_lo + 1)
                .min(FFT_SIZE / 2);
            let mut v = 0.0f32;
            for m in &mags[bin_lo..bin_hi] {
                v = v.max(*m);
            }
            let target = v.sqrt() * agc; // sqrt compresses the dynamic range
            frame_peak = frame_peak.max(target);
            if target > bars[b] {
                bars[b] = target;
            } else {
                bars[b] *= GRAVITY;
            }
        }

        // Auto-gain: normalize the tallest bar toward ~0.9. Attack fast when
        // clipping, release slowly when quiet; never touch it on silence.
        if frame_peak > 0.001 {
            let want = 0.9 / frame_peak * agc;
            agc += (want - agc) * if want < agc { 0.30 } else { 0.02 };
            agc = agc.clamp(0.001, 100_000.0);
        }

        // Render one string, write once.
        let mut frame = String::with_capacity(cols * rows * 4 + 32);
        frame.push_str("\x1b[H");
        for tr in 0..rows {
            let from_bottom = rows - 1 - tr;
            let frac = from_bottom as f32 / (rows as f32 - 1.0).max(1.0);
            let (cr, cg, cb) = grad(frac);
            frame.push_str(&format!("\x1b[38;2;{cr};{cg};{cb}m"));
            for b in 0..nbars {
                let h = (bars[b].min(1.0) * rows as f32 * 8.0) as i32;
                let level = (h - (from_bottom as i32) * 8).clamp(0, 8) as usize;
                let ch = blocks[level];
                for _ in 0..BAR_W {
                    frame.push(ch);
                }
                if b + 1 < nbars {
                    for _ in 0..GAP {
                        frame.push(' ');
                    }
                }
            }
            frame.push_str("\x1b[0m");
            if tr + 1 < rows {
                frame.push_str("\r\n");
            }
        }
        let _ = stdout.write_all(frame.as_bytes());
        let _ = stdout.flush();

        let dt = t0.elapsed();
        if dt < frame_time {
            std::thread::sleep(frame_time - dt);
        }
    }

    let _ = execute!(stdout, Show, LeaveAlternateScreen);
    let _ = terminal::disable_raw_mode();
}

// Vertical gradient: deep blue at the bottom -> bright cyan near the top.
fn grad(t: f32) -> (u8, u8, u8) {
    let lerp = |a: f32, b: f32| (a + (b - a) * t) as u8;
    (lerp(40.0, 150.0), lerp(90.0, 230.0), lerp(200.0, 255.0))
}

fn spawn_audio(ring: Arc<Mutex<Vec<f32>>>) {
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
            return;
        };
        let Some(mut out) = child.stdout.take() else { return };
        let mut buf = [0u8; 8192];
        loop {
            match out.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let frames = n / 4;
                    let mut r = ring.lock().unwrap();
                    for i in 0..frames {
                        let l = i16::from_le_bytes([buf[i * 4], buf[i * 4 + 1]]) as f32 / 32768.0;
                        let rr = i16::from_le_bytes([buf[i * 4 + 2], buf[i * 4 + 3]]) as f32 / 32768.0;
                        r.push((l + rr) * 0.5);
                    }
                    let len = r.len();
                    if len > FFT_SIZE {
                        r.drain(0..len - FFT_SIZE);
                    }
                }
            }
        }
    });
}
