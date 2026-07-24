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
use windows::core::*;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;

const OUT_RATE: f64 = 48000.0;

fn main() -> Result<()> {
    let mic = std::env::args().any(|a| a == "--mic");
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        let stdout = std::io::stdout();
        let mut out = std::io::BufWriter::with_capacity(1 << 16, stdout.lock());
        // Reopen on any capture error (device invalidated / format change) so the
        // pipe to the reader never permanently closes.
        loop {
            if let Err(e) = capture(&mut out, mic) {
                eprintln!("{}: {e:?}; reopening endpoint", if mic { "mic" } else { "loopback" });
                std::thread::sleep(Duration::from_millis(300));
            }
        }
    }
}

unsafe fn capture<W: Write>(out: &mut W, mic: bool) -> Result<()> {
    let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    // Mic = default capture endpoint, no loopback flag. Otherwise = default render
    // endpoint captured in loopback mode.
    let (flow, stream_flags) = if mic {
        (eCapture, 0u32)
    } else {
        (eRender, AUDCLNT_STREAMFLAGS_LOOPBACK)
    };
    let device = enumerator.GetDefaultAudioEndpoint(flow, eConsole)?;
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
        "{}: {in_rate} Hz, {in_ch} ch, {bits} bit, float={is_float}",
        if mic { "mic" } else { "loopback" }
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
                    write_frame(out, l, r)?;
                    frames_out += 1;
                } else {
                    while resamp_pos < 1.0 {
                        let t = resamp_pos as f32;
                        write_frame(out, prev_l + (l - prev_l) * t, prev_r + (r - prev_r) * t)?;
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
                write_frame(out, 0.0, 0.0)?;
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
        // If the reader (cava / ffmpeg) is gone, the pipe write fails -> exit
        // instead of busy-looping forever as an orphan (that was the CPU leak).
        if out.flush().is_err() {
            std::process::exit(0);
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

#[inline]
fn write_frame<W: Write>(out: &mut W, l: f32, r: f32) -> Result<()> {
    let li = (l.clamp(-1.0, 1.0) * 32767.0) as i16;
    let ri = (r.clamp(-1.0, 1.0) * 32767.0) as i16;
    out.write_all(&li.to_le_bytes()).ok();
    out.write_all(&ri.to_le_bytes()).ok();
    Ok(())
}
