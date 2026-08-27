// Terminal audio spectrum visualizer (CAVA-style) for Windows. Pulls system
// audio from the sibling sysaudio-loopback.exe (WASAPI loopback, s16le 48 kHz
// stereo), runs a windowed FFT, and draws log-frequency bars with a vertical
// truecolor gradient. Targets 165 fps. Quit: q / Esc / Ctrl+C.
//
// Motion is smoothed in three cheap O(bars) passes so it flows instead of
// flickering: a temporal EMA on the spectrum (kills frame jitter), monstercat
// spatial spreading (neighbours move together -> smooth hills, not chaos), and a
// per-bar spring-damper that gives a natural rise + bounce and settles softly.

use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use rustfft::{num_complex::Complex, FftPlanner};
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SAMPLE_RATE: f32 = 48000.0;
const FFT_SIZE: usize = 2048;
const MIN_FREQ: f32 = 30.0;
const MAX_FREQ: f32 = 16000.0;
const TARGET_FPS: u64 = 165;
const BAR_W: usize = 2;
const GAP: usize = 1;

// Smoothing / motion.
const TEMPORAL: f32 = 0.35; // weight kept from the previous spectrum (EMA)
const MONSTERCAT: f32 = 1.6; // spatial spread: a peak bleeds into neighbours / this^dist
const SPREAD: i32 = 4; // neighbour radius for the spatial pass
const SPRING_K: f32 = 210.0; // stiffness (snappiness)
const SPRING_D: f32 = 13.0; // damping (lower = more bounce)
const AGC_TARGET: f32 = 0.78; // normalize the loudest bar here (leaves room for overshoot)

fn main() {
    // Windows sleep granularity is ~15ms by default -> would cap us near 64 fps.
    // Raise the timer resolution to 1ms so the frame pacing can hit 165 fps.
    unsafe {
        timeBeginPeriod(1);
    }

    // --log-fps <path>: append "secs_since_start,cols,rows,fps" every time the
    // on-screen counter updates (twice a second), independent of whether it's
    // actually shown ('f' toggle). Opt-in only, for A/B'ing wezterm front_ends
    // from outside -- there's no way to read another process's rendered
    // terminal content, so this is the log-file equivalent of eyeballing the
    // 'f' readout.
    let args: Vec<String> = std::env::args().collect();
    let log_fps_path = args.iter().position(|a| a == "--log-fps").and_then(|i| args.get(i + 1).cloned());
    let log_start = Instant::now();

    let ring = Arc::new(Mutex::new(vec![0f32; FFT_SIZE]));
    // Keep the loopback child handle so we can kill it on exit. Otherwise it
    // orphans and busy-loops forever (that was the multi-loopback CPU leak).
    let child: Arc<Mutex<Option<std::process::Child>>> = Arc::new(Mutex::new(None));
    if let Some((out, c)) = spawn_loopback() {
        *child.lock().unwrap() = Some(c);
        let ring2 = ring.clone();
        std::thread::spawn(move || read_loop(ring2, out));
    }

    let mut stdout = std::io::stdout();
    let _ = terminal::enable_raw_mode();
    let _ = execute!(stdout, EnterAlternateScreen, Hide);
    let prev = std::panic::take_hook();
    let child_hook = child.clone();
    std::panic::set_hook(Box::new(move |info| {
        kill_child(&child_hook);
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
    // Per-bar state (resized when the terminal width changes).
    let mut spec: Vec<f32> = Vec::new(); // temporally-smoothed spectrum
    let mut sm: Vec<f32> = Vec::new(); // spatially-smoothed target
    let mut pos: Vec<f32> = Vec::new(); // rendered bar height (spring position)
    let mut frame = String::new(); // reused every frame (see the render section)
    let mut row_buf = String::new(); // scratch: one row, before diffing
    let mut prev_rows: Vec<String> = Vec::new(); // last painted content per row
    let mut last_cols = 0usize;
    let mut last_rows = 0usize;
    let mut vel: Vec<f32> = Vec::new(); // spring velocity
    let mut agc = 1.0f32;
    let mut last = Instant::now();
    let mut show_fps = false; // toggle with 'f'
    let mut fps = 0.0f32;
    let mut fps_frames = 0u32;
    // Split where the frame budget actually goes, accumulated over the same
    // 0.5s window as fps: compute (FFT + per-bar math + string building) vs
    // write+flush (the part that can block on wezterm's parser keeping up --
    // backpressure on a full pipe). If write dominates, the bottleneck is on
    // wezterm's side of the pipe, not cava's own math.
    let mut compute_accum = Duration::ZERO;
    let mut write_accum = Duration::ZERO;
    let mut bytes_accum = 0u64;
    let mut fps_t = Instant::now();

    'main: loop {
        let t0 = Instant::now();
        let dt = (t0 - last).as_secs_f32().clamp(0.0001, 0.02);
        last = t0;

        // Measured loop rate (updated twice a second).
        fps_frames += 1;
        let fe = fps_t.elapsed().as_secs_f32();
        if fe >= 0.5 {
            fps = fps_frames as f32 / fe;
            if let Some(path) = &log_fps_path {
                let (lc, lr) = terminal::size().unwrap_or((0, 0));
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                    let _ = writeln!(
                        f,
                        "{:.2},{},{},{:.1},{:.2},{:.2},{}",
                        log_start.elapsed().as_secs_f32(),
                        lc,
                        lr,
                        fps,
                        compute_accum.as_secs_f32() * 1000.0 / fps_frames.max(1) as f32,
                        write_accum.as_secs_f32() * 1000.0 / fps_frames.max(1) as f32,
                        bytes_accum / fps_frames.max(1) as u64,
                    );
                }
            }
            fps_frames = 0;
            fps_t = Instant::now();
            compute_accum = Duration::ZERO;
            write_accum = Duration::ZERO;
            bytes_accum = 0;
        }

        while event::poll(Duration::ZERO).unwrap_or(false) {
            if let Ok(Event::Key(k)) = event::read() {
                let quit = matches!(k.code, KeyCode::Char('q') | KeyCode::Esc)
                    || (k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL));
                if quit {
                    break 'main;
                }
                if k.code == KeyCode::Char('f') {
                    show_fps = !show_fps;
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
        if spec.len() != nbars {
            spec = vec![0.0; nbars];
            sm = vec![0.0; nbars];
            pos = vec![0.0; nbars];
            vel = vec![0.0; nbars];
        }
        // Row content is keyed on (cols, rows) via nbars/from_bottom; on any size
        // change the old rows no longer mean anything, so force a full repaint.
        if cols != last_cols || rows != last_rows {
            last_cols = cols;
            last_rows = rows;
            prev_rows = vec![String::new(); rows];
        }

        // FFT of the latest window.
        let samples = { ring.lock().unwrap().clone() };
        let mut buf: Vec<Complex<f32>> = (0..FFT_SIZE)
            .map(|i| Complex::new(samples.get(i).copied().unwrap_or(0.0) * hann[i], 0.0))
            .collect();
        fft.process(&mut buf);
        let mags: Vec<f32> = buf[..FFT_SIZE / 2].iter().map(|c| c.norm()).collect();

        // Per-bar raw magnitude (peak in a log-frequency band), sqrt-compressed.
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
            let raw = v.sqrt();
            frame_peak = frame_peak.max(raw * agc);
            // Temporal EMA: blend into the running spectrum to kill flicker.
            spec[b] = spec[b] * TEMPORAL + raw * agc * (1.0 - TEMPORAL);
        }

        // Auto-gain: normalize the loudest bar toward AGC_TARGET. Fast when
        // clipping, slow when quiet; untouched on silence (no runaway).
        if frame_peak > 0.001 {
            let want = AGC_TARGET / frame_peak * agc;
            agc += (want - agc) * if want < agc { 0.12 } else { 0.02 };
            agc = agc.clamp(0.001, 100_000.0);
        }

        // Spatial monstercat: each bar bleeds into its neighbours with distance
        // falloff, so peaks become smooth hills instead of lone spikes.
        for b in 0..nbars {
            sm[b] = spec[b];
        }
        for b in 0..nbars {
            let base = spec[b];
            for k in 1..=SPREAD {
                let f = base / MONSTERCAT.powi(k);
                let lo = b as i32 - k;
                let hi = b as i32 + k;
                if lo >= 0 && f > sm[lo as usize] {
                    sm[lo as usize] = f;
                }
                if (hi as usize) < nbars && f > sm[hi as usize] {
                    sm[hi as usize] = f;
                }
            }
        }

        // Spring-damper toward the target: smooth rise, gentle bounce, soft fall.
        for b in 0..nbars {
            let accel = (sm[b] - pos[b]) * SPRING_K - vel[b] * SPRING_D;
            vel[b] += accel * dt;
            pos[b] += vel[b] * dt;
            if pos[b] < 0.0 {
                pos[b] = 0.0;
                if vel[b] < 0.0 {
                    vel[b] = 0.0;
                }
            }
        }

        // Render each row into a scratch buffer, diff it against what's actually
        // on screen, and only emit the rows that changed (absolute cursor
        // position + content). `frame` accumulates just those.
        //
        // Why per-ROW and not per-cell: color is already a per-row gradient band
        // (one escape code per row, not per bar), so a row is the natural, free
        // diff granularity -- no extra bookkeeping to find "which cells changed
        // color" because within a row they never do.
        //
        // MEDIDO, y es la razon de esto: wezterm-gui se comia un nucleo entero
        // con cava abierto -- Y TAMBIEN EN SILENCIO ABSOLUTO, porque antes cava
        // reenviaba la pantalla COMPLETA 165 veces por segundo pasara lo que
        // pasara (~41 KB/fotograma) y wezterm tenia que parsear eso cada vez. El
        // viejo fix (no reescribir un fotograma identico byte-a-byte) mataba el
        // coste en silencio, pero con musica real CADA fotograma es distinto --
        // aunque solo cambie 1 barra de 150 -- asi que el gasto completo volvia,
        // y a pantalla grande + el resto del rice (scrollbar, decoraciones,
        // opacidad) componiendo cada frame del lado de wezterm, esa manguera de
        // ~6 MB/s sostenidos era mas de lo que el hilo que drena el pty podia
        // seguir: el bloqueo se veia como escrituras (`write_ms`) de hasta 16 ms
        // en vez de los ~1 ms normales, y el fps caia por debajo de 165 aunque el
        // computo (FFT + muelles) seguia constante en <0.3 ms. Diffing por fila
        // corta esos ~41 KB a solo las filas que de verdad cambiaron -- con el
        // muelle-amortiguador la mayoria de barras se mueven <1 nivel por
        // fotograma, asi que la mayoria de filas repiten y no se reenvian.
        frame.clear();
        for tr in 0..rows {
            let from_bottom = rows - 1 - tr;
            let frac = from_bottom as f32 / (rows as f32 - 1.0).max(1.0);
            let (cr, cg, cb) = grad(frac);
            row_buf.clear();
            let _ = write!(row_buf, "\x1b[38;2;{cr};{cg};{cb}m");
            for b in 0..nbars {
                let h = (pos[b].min(1.0) * rows as f32 * 8.0) as i32;
                let level = (h - (from_bottom as i32) * 8).clamp(0, 8) as usize;
                let ch = blocks[level];
                for _ in 0..BAR_W {
                    row_buf.push(ch);
                }
                if b + 1 < nbars {
                    for _ in 0..GAP {
                        row_buf.push(' ');
                    }
                }
            }
            row_buf.push_str("\x1b[0m");
            if prev_rows[tr] != row_buf {
                let _ = write!(frame, "\x1b[{};1H", tr + 1);
                frame.push_str(&row_buf);
                prev_rows[tr].clear();
                prev_rows[tr].push_str(&row_buf);
            }
        }
        // FPS readout, top-right (toggle with 'f'). Small (~30B) and unconditional
        // when shown, so its own digit-churn never forces a full-frame rewrite.
        if show_fps {
            let width = format!("{fps:.0}").len() + 6; // " NNN fps "
            let col = cols.saturating_sub(width).max(1);
            let _ = write!(frame, "\x1b[1;{col}H\x1b[97m {fps:.0} fps \x1b[0m");
        }
        // Everything above this line is compute (FFT, per-bar math, building
        // `frame`); everything from here to the next line is the write.
        // Timed separately so a slowdown can be pinned on one side or the
        // other instead of guessed at.
        let compute_done = Instant::now();
        compute_accum += compute_done - t0;
        if !frame.is_empty() {
            let _ = stdout.write_all(frame.as_bytes());
            let _ = stdout.flush();
            bytes_accum += frame.len() as u64;
        }
        write_accum += compute_done.elapsed();

        let elapsed = t0.elapsed();
        if elapsed < frame_time {
            std::thread::sleep(frame_time - elapsed);
        }
    }

    let _ = execute!(stdout, Show, LeaveAlternateScreen);
    let _ = terminal::disable_raw_mode();
    kill_child(&child);
}

fn kill_child(child: &Arc<Mutex<Option<std::process::Child>>>) {
    if let Ok(mut c) = child.lock() {
        if let Some(c) = c.as_mut() {
            let _ = c.kill();
        }
    }
}

// Vertical sunset gradient bottom->top: green -> brown -> orange -> red.
fn grad(t: f32) -> (u8, u8, u8) {
    const STOPS: [(f32, f32, f32, f32); 4] = [
        (0.00, 70.0, 135.0, 45.0),  // green
        (0.42, 150.0, 90.0, 38.0),  // brown
        (0.72, 235.0, 130.0, 30.0), // orange
        (1.00, 215.0, 45.0, 32.0),  // red
    ];
    let t = t.clamp(0.0, 1.0);
    let mut i = 0;
    while i + 1 < STOPS.len() && t > STOPS[i + 1].0 {
        i += 1;
    }
    let a = STOPS[i];
    let b = STOPS[(i + 1).min(STOPS.len() - 1)];
    let f = ((t - a.0) / (b.0 - a.0).max(1e-6)).clamp(0.0, 1.0);
    let lerp = |x: f32, y: f32| (x + (y - x) * f) as u8;
    (lerp(a.1, b.1), lerp(a.2, b.2), lerp(a.3, b.3))
}

fn spawn_loopback() -> Option<(std::process::ChildStdout, std::process::Child)> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let lb = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.join("sysaudio-loopback.exe")))
        .unwrap_or_else(|| "sysaudio-loopback.exe".into());
    let mut child = std::process::Command::new(lb)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .ok()?;
    let out = child.stdout.take()?;
    Some((out, child))
}

fn read_loop(ring: Arc<Mutex<Vec<f32>>>, mut out: std::process::ChildStdout) {
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
}

#[link(name = "winmm")]
extern "system" {
    fn timeBeginPeriod(uperiod: u32) -> u32;
}
