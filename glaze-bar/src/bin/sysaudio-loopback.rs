// WASAPI loopback capture of the default render endpoint (whatever the system
// plays, incl. a USB headset) -> raw s16le stereo @ 48000 Hz on stdout, so the
// ShadowPlay ffmpeg can mux it with the video. Native, no third-party driver,
// no routing change. Pads silence in real time so audio stays A/V-synced even
// when nothing is playing.

use std::io::Write;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;

const OUT_RATE: f64 = 48000.0;

fn main() -> Result<()> {
    unsafe { run() }
}

unsafe fn run() -> Result<()> {
    CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;

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

    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::with_capacity(1 << 16, stdout.lock());

    let start = std::time::Instant::now();
    let mut out_frames: u64 = 0; // stereo frames written at OUT_RATE
    let ratio = OUT_RATE / in_rate; // out samples per in sample
    let mut resamp_pos = 0.0f64; // fractional read position for linear resample
    let mut prev_l = 0.0f32;
    let mut prev_r = 0.0f32;

    loop {
        // Drain all currently available packets.
        loop {
            let avail = capture.GetNextPacketSize()?;
            if avail == 0 {
                break;
            }
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
                    write_frame(&mut out, l, r)?;
                    out_frames += 1;
                } else {
                    // emit output samples until resamp_pos passes this input frame
                    while resamp_pos < 1.0 {
                        let t = resamp_pos as f32;
                        write_frame(&mut out, prev_l + (l - prev_l) * t, prev_r + (r - prev_r) * t)?;
                        out_frames += 1;
                        resamp_pos += 1.0 / ratio;
                    }
                    resamp_pos -= 1.0;
                }
                prev_l = l;
                prev_r = r;
            }
            capture.ReleaseBuffer(frames)?;
        }

        // Real-time silence padding to keep the stream continuous.
        let expected = (start.elapsed().as_secs_f64() * OUT_RATE) as u64;
        if out_frames < expected {
            let pad = (expected - out_frames).min(OUT_RATE as u64); // cap at 1s
            for _ in 0..pad {
                write_frame(&mut out, 0.0, 0.0)?;
            }
            out_frames += pad;
        }
        out.flush().ok();
        std::thread::sleep(std::time::Duration::from_millis(5));
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
