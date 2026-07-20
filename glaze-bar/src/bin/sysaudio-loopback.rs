// WASAPI loopback capture of the default render endpoint (whatever the system
// plays, incl. a USB headset) -> raw s16le stereo @ 48000 Hz on stdout, so the
// ShadowPlay ffmpeg can mux it with the video. Native, no third-party driver,
// no routing change.
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
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;

const OUT_RATE: f64 = 48000.0;

fn main() -> Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        let stdout = std::io::stdout();
        let mut out = std::io::BufWriter::with_capacity(1 << 16, stdout.lock());
        // Reopen on any capture error (device invalidated / format change) so the
        // pipe to ffmpeg never permanently closes.
        loop {
            if let Err(e) = capture(&mut out) {
                eprintln!("loopback: {e:?}; reopening endpoint");
                std::thread::sleep(Duration::from_millis(300));
            }
        }
    }
}

unsafe fn capture<W: Write>(out: &mut W) -> Result<()> {
    let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
    let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;

    let pwfx = client.GetMixFormat()?;
    let wf = *pwfx;
    let in_rate = wf.nSamplesPerSec as f64;
    let in_ch = wf.nChannels as usize;
    let bits = wf.wBitsPerSample;
    let is_float = wf.wFormatTag == 3 /* IEEE_FLOAT */
        || (wf.wFormatTag == 0xFFFE /* EXTENSIBLE */ && bits == 32);
    eprintln!("loopback: {in_rate} Hz, {in_ch} ch, {bits} bit, float={is_float}");

    // 200ms shared buffer, loopback mode.
    let hns_buffer: i64 = 2_000_000;
    client.Initialize(
        AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_LOOPBACK,
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
    let mut last_emit = Instant::now(); // wallclock ref for idle silence pacing

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
                    write_frame(out, l, r)?;
                } else {
                    while resamp_pos < 1.0 {
                        let t = resamp_pos as f32;
                        write_frame(out, prev_l + (l - prev_l) * t, prev_r + (r - prev_r) * t)?;
                        resamp_pos += 1.0 / ratio;
                    }
                    resamp_pos -= 1.0;
                }
                prev_l = l;
                prev_r = r;
            }
            capture.ReleaseBuffer(frames)?;
        }

        if got_real {
            // Real audio flowed this pass -> reset the silence clock so we never
            // inject silence between real samples (the old choppy-audio bug).
            last_emit = Instant::now();
        } else {
            // Endpoint idle: no loopback packets at all. Emit wallclock-paced
            // silence so ffmpeg's audio timeline keeps advancing and its muxer
            // doesn't stall (which froze the whole recording).
            let elapsed = last_emit.elapsed().as_secs_f64().min(0.5); // cap bursts
            let need = (elapsed * OUT_RATE) as usize;
            if need > 0 {
                for _ in 0..need {
                    write_frame(out, 0.0, 0.0)?;
                }
                last_emit = Instant::now();
            }
        }
        out.flush().ok();
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[inline]
fn write_frame<W: Write>(out: &mut W, l: f32, r: f32) -> Result<()> {
    let li = (l.clamp(-1.0, 1.0) * 32767.0) as i16;
    let ri = (r.clamp(-1.0, 1.0) * 32767.0) as i16;
    out.write_all(&li.to_le_bytes()).ok();
    out.write_all(&ri.to_le_bytes()).ok();
    Ok(())
}
