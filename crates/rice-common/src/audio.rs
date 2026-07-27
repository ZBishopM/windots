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
/// Same as `init_com`, for sibling modules that also talk COM.
pub fn init_com_pub() {
    init_com();
}

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

/// Friendly name of the current default playback device, e.g.
/// `Altavoces (HyperX Cloud II Wireless)`.
pub fn current_output_name() -> Option<String> {
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
    use windows::Win32::System::Com::{CoTaskMemFree, STGM_READ};
    init_com();
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let dev = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
        let store = dev.OpenPropertyStore(STGM_READ).ok()?;
        let pv = store.GetValue(&PKEY_Device_FriendlyName).ok()?;
        let ws = PropVariantToStringAlloc(&pv).ok()?;
        let s = ws.to_string().ok();
        CoTaskMemFree(Some(ws.0 as *const core::ffi::c_void));
        s
    }
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

// ---------------------------------------------------------------- outputs
// Enumerating playback devices and changing the default one lived only inside
// micswitch, so the bar could not show a list -- it shelled out to micswitch to
// flip between two hard-coded names. With a third output in the mix (a Bluetooth
// headset that comes and goes) a two-way toggle stops making sense, so this
// moved here and both callers share it.

/// One playback endpoint.
#[derive(Clone, Debug, PartialEq)]
pub struct Endpoint {
    /// Opaque endpoint id, the handle used to make it default.
    pub id: String,
    /// e.g. `Altavoces (HyperX Cloud II Wireless)`.
    pub name: String,
    /// False for a device that is paired/known but not currently plugged in or
    /// connected -- Bluetooth headsets sit here while switched off.
    pub active: bool,
    /// Groups the endpoints belonging to one physical device. A Bluetooth
    /// headset exposes several (A2DP playback, hands-free playback, capture)
    /// and they all share this.
    pub container: Option<u128>,
}

// IPolicyConfig is how the OS's own sound flyout changes the default device.
// It is undocumented and has no published header; the CLSID/IID below and the
// vtable layout are the well-known reverse-engineered ones.
const CLSID_POLICY_CONFIG: windows::core::GUID =
    windows::core::GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
const IID_IPOLICY_CONFIG: windows::core::GUID =
    windows::core::GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);

/// Vtable up to the one method we need. SetDefaultEndpoint is slot 13:
/// IUnknown's 3, then 10 methods we never call.
#[repr(C)]
struct PolicyVtbl {
    query_interface: usize,
    add_ref: usize,
    release: unsafe extern "system" fn(*mut core::ffi::c_void) -> u32,
    _stubs: [usize; 10],
    set_default_endpoint: unsafe extern "system" fn(
        *mut core::ffi::c_void,
        windows::core::PCWSTR,
        i32,
    ) -> windows::core::HRESULT,
}

unsafe fn endpoint_id(dev: &windows::Win32::Media::Audio::IMMDevice) -> Option<String> {
    use windows::Win32::System::Com::CoTaskMemFree;
    let p = dev.GetId().ok()?;
    let s = p.to_string().ok();
    CoTaskMemFree(Some(p.0 as *const core::ffi::c_void));
    s
}

unsafe fn endpoint_name(dev: &windows::Win32::Media::Audio::IMMDevice) -> Option<String> {
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
    use windows::Win32::System::Com::{CoTaskMemFree, STGM_READ};
    let store = dev.OpenPropertyStore(STGM_READ).ok()?;
    let pv = store.GetValue(&PKEY_Device_FriendlyName).ok()?;
    let ws = PropVariantToStringAlloc(&pv).ok()?;
    let s = ws.to_string().ok();
    CoTaskMemFree(Some(ws.0 as *const core::ffi::c_void));
    s
}

unsafe fn endpoint_container(dev: &windows::Win32::Media::Audio::IMMDevice) -> Option<u128> {
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_ContainerId;
    use windows::Win32::System::Com::STGM_READ;
    let store = dev.OpenPropertyStore(STGM_READ).ok()?;
    let pv = store.GetValue(&PKEY_Device_ContainerId).ok()?;
    // VT_CLSID: the payload is a pointer to a GUID.
    let raw = pv.as_raw();
    let g = raw.Anonymous.Anonymous.Anonymous.puuid;
    if g.is_null() {
        return None;
    }
    let g = *g;
    // No to_u128 on this GUID type; pack the fields ourselves. Only used as a
    // grouping key, so any injective encoding will do.
    let mut k: u128 = ((g.data1 as u128) << 96) | ((g.data2 as u128) << 80) | ((g.data3 as u128) << 64);
    for (i, b) in g.data4.iter().enumerate() {
        k |= (*b as u128) << (56 - i * 8);
    }
    Some(k)
}

/// Playback endpoints. `include_inactive` also returns devices that are known
/// but not currently connected -- which is the whole point for Bluetooth, since
/// a headset that is switched off must still be listed so it can be woken up.
pub fn outputs(include_inactive: bool) -> Vec<Endpoint> {
    use windows::Win32::Media::Audio::{
        DEVICE_STATEMASK_ALL, DEVICE_STATE_ACTIVE, IMMDeviceEnumerator, MMDeviceEnumerator,
    };
    init_com();
    let mut out = Vec::new();
    unsafe {
        let Ok(en) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
        else {
            return out;
        };
        let mask = if include_inactive {
            windows::Win32::Media::Audio::DEVICE_STATE(DEVICE_STATEMASK_ALL)
        } else {
            DEVICE_STATE_ACTIVE
        };
        let Ok(coll) = en.EnumAudioEndpoints(eRender, mask) else {
            return out;
        };
        for i in 0..coll.GetCount().unwrap_or(0) {
            let Ok(dev) = coll.Item(i) else { continue };
            let (Some(id), Some(name)) = (endpoint_id(&dev), endpoint_name(&dev)) else {
                continue;
            };
            out.push(Endpoint {
                id,
                name,
                active: dev.GetState().map(|st| st == DEVICE_STATE_ACTIVE).unwrap_or(false),
                container: endpoint_container(&dev),
            });
        }
    }
    out
}

/// Endpoint id of the current default playback device.
pub fn current_output_id() -> Option<String> {
    use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator};
    init_com();
    unsafe {
        let en: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let dev = en.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
        endpoint_id(&dev)
    }
}

/// Roles a default device can hold. eConsole and eMultimedia are "where sound
/// comes out"; eCommunications is what voice apps grab, and it is deliberately
/// separate -- see `set_default_output`.
pub const ROLE_CONSOLE: i32 = 0;
pub const ROLE_MULTIMEDIA: i32 = 1;
pub const ROLE_COMMUNICATIONS: i32 = 2;

/// Make `id` the default playback device for the given roles.
///
/// Splitting the roles matters for Bluetooth headphones. Classic Bluetooth
/// cannot do stereo playback and a microphone at once: A2DP is output-only, and
/// the moment anything opens the headset's mic Windows switches the whole device
/// to hands-free, which is mono and 16kHz, and BOTH directions collapse. Making
/// a pair of earbuds the communications device is therefore how you quietly
/// ruin their sound -- the next voice call drags them into hands-free.
pub fn set_default_output_roles(id: &str, roles: &[i32]) -> bool {
    use windows::core::{Interface, PCWSTR};
    use windows::Win32::System::Com::CoCreateInstance;
    init_com();
    unsafe {
        let Ok(unk) = CoCreateInstance::<_, windows::core::IUnknown>(
            &CLSID_POLICY_CONFIG,
            None,
            CLSCTX_ALL,
        ) else {
            return false;
        };
        let mut pc: *mut core::ffi::c_void = std::ptr::null_mut();
        if unk.query(&IID_IPOLICY_CONFIG, &mut pc).is_err() || pc.is_null() {
            return false;
        }
        let vtbl = *(pc as *const *const PolicyVtbl);
        let wide: Vec<u16> = id.encode_utf16().chain(Some(0)).collect();
        let mut ok = true;
        for &role in roles {
            ok &= ((*vtbl).set_default_endpoint)(pc, PCWSTR(wide.as_ptr()), role).is_ok();
        }
        ((*vtbl).release)(pc);
        ok
    }
}

/// All three roles. Only right for a device that is also a sane microphone --
/// for headphones, prefer `set_default_output_roles` without the
/// communications role.
pub fn set_default_output(id: &str) -> bool {
    set_default_output_roles(id, &[ROLE_CONSOLE, ROLE_MULTIMEDIA, ROLE_COMMUNICATIONS])
}

/// Friendly name of the default playback device for one specific role. Roles can
/// and here deliberately do point at different devices.
pub fn output_name_for_role(role: i32) -> Option<String> {
    use windows::Win32::Media::Audio::{eCommunications, eMultimedia, IMMDeviceEnumerator, MMDeviceEnumerator};
    init_com();
    unsafe {
        let en: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let r = match role {
            ROLE_MULTIMEDIA => eMultimedia,
            ROLE_COMMUNICATIONS => eCommunications,
            _ => eConsole,
        };
        let dev = en.GetDefaultAudioEndpoint(eRender, r).ok()?;
        endpoint_name(&dev)
    }
}
