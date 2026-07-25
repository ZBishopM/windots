//! Per-application volume, via the Windows Core Audio session API.
//!
//! This is what EarTrumpet does. Each app that plays sound owns an audio
//! *session* on the default render device; every session carries its own volume
//! and mute independent of the master. Sessions come and go as apps start and
//! stop playing, so the list is queried on demand rather than cached.

use windows::core::Interface;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator,
    ISimpleAudioVolume, MMDeviceEnumerator,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
};

/// One application currently holding audio sessions.
///
/// Keyed by executable name rather than pid: a browser or Discord spreads audio
/// over several processes, and a per-pid list would show the same app three
/// times with three sliders that each move a third of the sound.
#[derive(Clone, Debug)]
pub struct Session {
    /// Executable name without the path, e.g. `firefox.exe`.
    pub name: String,
    /// Every process id this app currently has a session on.
    pub pids: Vec<u32>,
    /// 0.0..=1.0 (the loudest of the app's sessions).
    pub volume: f32,
    pub muted: bool,
}

/// Our own helpers hold render sessions but are not things to mix.
fn is_internal(name: &str) -> bool {
    matches!(
        name.to_lowercase().as_str(),
        "sysaudio-loopback.exe" | "shadowplay-wgc.exe" | "cava.exe"
    )
}

/// COM has to be initialised on each thread that touches these APIs. Safe to
/// call repeatedly -- a second call on an already-initialised thread just
/// returns S_FALSE.
fn init_com() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

fn process_name(pid: u32) -> Option<String> {
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
        fn QueryFullProcessImageNameW(h: isize, flags: u32, buf: *mut u16, size: *mut u32) -> i32;
        fn CloseHandle(h: isize) -> i32;
    }
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h == 0 {
            return None;
        }
        let mut buf = [0u16; 260];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        if ok == 0 {
            return None;
        }
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        Some(full.rsplit(['\\', '/']).next().unwrap_or(&full).to_string())
    }
}

unsafe fn session_manager() -> Option<IAudioSessionManager2> {
    let enumerator: IMMDeviceEnumerator =
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
    let dev = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
    dev.Activate(CLSCTX_ALL, None).ok()
}

/// Every application currently playing (or holding) audio, loudest listed first.
///
/// The system sounds session has pid 0 and no image name; it is skipped, since
/// there is nothing useful to label or control there.
pub fn sessions() -> Vec<Session> {
    init_com();
    let mut out: Vec<Session> = Vec::new();
    unsafe {
        let Some(mgr) = session_manager() else { return out };
        let Ok(list) = mgr.GetSessionEnumerator() else { return out };
        let count = list.GetCount().unwrap_or(0);
        for i in 0..count {
            let Ok(ctl) = list.GetSession(i) else { continue };
            let Ok(ctl2) = ctl.cast::<IAudioSessionControl2>() else { continue };
            let pid = ctl2.GetProcessId().unwrap_or(0);
            if pid == 0 {
                continue;
            }
            let Some(name) = process_name(pid) else { continue };
            if is_internal(&name) {
                continue;
            }
            let Ok(vol) = ctl2.cast::<ISimpleAudioVolume>() else { continue };
            let v = vol.GetMasterVolume().unwrap_or(0.0);
            let m = vol.GetMute().map(|b| b.as_bool()).unwrap_or(false);
            match out.iter_mut().find(|s| s.name.eq_ignore_ascii_case(&name)) {
                Some(existing) => {
                    existing.pids.push(pid);
                    existing.volume = existing.volume.max(v);
                    existing.muted &= m;
                }
                None => out.push(Session { name, pids: vec![pid], volume: v, muted: m }),
            }
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// Apply `f` to every session belonging to processes named `name`.
fn for_app<F: Fn(&ISimpleAudioVolume) -> bool>(name: &str, f: F) -> bool {
    init_com();
    let mut any = false;
    unsafe {
        let Some(mgr) = session_manager() else { return false };
        let Ok(list) = mgr.GetSessionEnumerator() else { return false };
        let count = list.GetCount().unwrap_or(0);
        for i in 0..count {
            let Ok(ctl) = list.GetSession(i) else { continue };
            let Ok(ctl2) = ctl.cast::<IAudioSessionControl2>() else { continue };
            let pid = ctl2.GetProcessId().unwrap_or(0);
            if pid == 0 {
                continue;
            }
            let Some(pname) = process_name(pid) else { continue };
            // Substring match so `discord` reaches Discord.exe.
            if !pname.to_lowercase().contains(&name.to_lowercase()) {
                continue;
            }
            if let Ok(vol) = ctl2.cast::<ISimpleAudioVolume>() {
                any |= f(&vol);
            }
        }
    }
    any
}

/// Set an application's volume (0.0..=1.0) across all of its sessions.
pub fn set_app_volume(name: &str, volume: f32) -> bool {
    let v = volume.clamp(0.0, 1.0);
    for_app(name, |vol| unsafe { vol.SetMasterVolume(v, std::ptr::null()).is_ok() })
}

pub fn set_app_mute(name: &str, mute: bool) -> bool {
    for_app(name, |vol| unsafe { vol.SetMute(mute, std::ptr::null()).is_ok() })
}

// ---- master volume of the default output ----------------------------------

unsafe fn endpoint_volume() -> Option<IAudioEndpointVolume> {
    let enumerator: IMMDeviceEnumerator =
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
    let dev = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
    dev.Activate(CLSCTX_ALL, None).ok()
}

/// Master volume of the default playback device, 0.0..=1.0.
pub fn master_volume() -> Option<f32> {
    init_com();
    unsafe { endpoint_volume()?.GetMasterVolumeLevelScalar().ok() }
}

pub fn set_master_volume(v: f32) -> bool {
    init_com();
    unsafe {
        match endpoint_volume() {
            Some(e) => e
                .SetMasterVolumeLevelScalar(v.clamp(0.0, 1.0), std::ptr::null())
                .is_ok(),
            None => false,
        }
    }
}

pub fn master_muted() -> bool {
    init_com();
    unsafe {
        endpoint_volume()
            .and_then(|e| e.GetMute().ok())
            .map(|b| b.as_bool())
            .unwrap_or(false)
    }
}

pub fn set_master_mute(mute: bool) -> bool {
    init_com();
    unsafe {
        match endpoint_volume() {
            Some(e) => e.SetMute(mute, std::ptr::null()).is_ok(),
            None => false,
        }
    }
}
