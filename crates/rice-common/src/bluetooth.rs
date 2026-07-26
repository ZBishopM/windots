//! Bluetooth *audio* devices: list them, and connect or disconnect on demand.
//!
//! Deliberately not built on the Bluetooth API. `BluetoothSetServiceState` is
//! the obvious-looking call and it is the wrong one: it is documented as
//! installing and *removing the device driver*, not as connecting, and once a
//! service is removed the endpoint disappears from Sound settings and stays gone
//! across reboots. Finding a device for it also needs `BluetoothFindFirstDevice`,
//! whose refresh path costs 1.28s per multiplier unit.
//!
//! Instead this goes through Core Audio, the same path Windows' own "Connect"
//! button uses:
//!
//!   * A paired-but-disconnected headset still exists as an audio endpoint, in
//!     state `UNPLUGGED` rather than `ACTIVE`. So enumerating endpoints with the
//!     full state mask lists devices that are switched off, and the state IS the
//!     connection status -- no Bluetooth call needed to know it.
//!   * Connecting and disconnecting is `IKsControl::KsProperty` with
//!     `KSPROPSETID_BtAudio`. Reaching the KS filter means walking the endpoint's
//!     topology to the upstream device and recognising it by a `bth` device id.
//!
//! One trap worth stating: a headset exposes several endpoints (A2DP playback,
//! hands-free playback, its microphone) and Windows only considers it fully
//! disconnected once all of them are. They are grouped by container id, and the
//! property is fired at every filter in the group.

use windows::core::{Interface, PCWSTR};
use windows::Win32::Media::Audio::{
    eAll, DEVICE_STATEMASK_ALL, DEVICE_STATE, DEVICE_STATE_ACTIVE, IDeviceTopology, IMMDevice,
    IMMDeviceEnumerator, IPart, MMDeviceEnumerator,
};
use windows::Win32::Media::KernelStreaming::{
    KSIDENTIFIER, KSIDENTIFIER_0, KSIDENTIFIER_0_0, KSPROPERTY_TYPE_GET,
};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL};

// KSPROPSETID_BtAudio and its two one-shot properties. Not exported by the
// `windows` crate's KernelStreaming module, so spelled out here.
const KSPROPSETID_BT_AUDIO: windows::core::GUID =
    windows::core::GUID::from_u128(0x7fa06c40_b8f6_4c7e_8556_e8c33a12e54d);
const KSPROPERTY_ONESHOT_RECONNECT: u32 = 0;
const KSPROPERTY_ONESHOT_DISCONNECT: u32 = 1;

/// A Bluetooth audio device, as one row in the UI.
pub struct Device {
    /// Groups this device's endpoints. Stable across connect/disconnect.
    pub container: u128,
    /// Best human name we could find for it.
    pub name: String,
    /// True when at least one of its endpoints is live.
    pub connected: bool,
    /// The endpoint to make default once it is connected. Deliberately the A2DP
    /// one: the hands-free endpoint is 8/16kHz mono and sounds broken for music.
    pub output_id: Option<String>,
    /// One KS filter per endpoint, all fired together.
    controls: Vec<windows::Win32::Media::KernelStreaming::IKsControl>,
}

impl std::fmt::Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Device")
            .field("name", &self.name)
            .field("connected", &self.connected)
            .field("output_id", &self.output_id)
            .finish()
    }
}

unsafe fn device_id_of(dev: &IMMDevice) -> Option<String> {
    let p = dev.GetId().ok()?;
    let s = p.to_string().ok();
    CoTaskMemFree(Some(p.0 as *const core::ffi::c_void));
    s
}

unsafe fn friendly_name(dev: &IMMDevice) -> Option<String> {
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
    use windows::Win32::System::Com::STGM_READ;
    let store = dev.OpenPropertyStore(STGM_READ).ok()?;
    let pv = store.GetValue(&PKEY_Device_FriendlyName).ok()?;
    let ws = PropVariantToStringAlloc(&pv).ok()?;
    let s = ws.to_string().ok();
    CoTaskMemFree(Some(ws.0 as *const core::ffi::c_void));
    s
}

unsafe fn container_of(dev: &IMMDevice) -> Option<u128> {
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_ContainerId;
    use windows::Win32::System::Com::STGM_READ;
    let store = dev.OpenPropertyStore(STGM_READ).ok()?;
    let pv = store.GetValue(&PKEY_Device_ContainerId).ok()?;
    let g = pv.as_raw().Anonymous.Anonymous.Anonymous.puuid;
    if g.is_null() {
        return None;
    }
    let g = *g;
    let mut k: u128 =
        ((g.data1 as u128) << 96) | ((g.data2 as u128) << 80) | ((g.data3 as u128) << 64);
    for (i, b) in g.data4.iter().enumerate() {
        k |= (*b as u128) << (56 - i * 8);
    }
    Some(k)
}

/// Device id of whatever sits upstream of this endpoint in the audio topology.
/// For a Bluetooth endpoint that is the KS filter exposed by the bth driver.
unsafe fn upstream_id(dev: &IMMDevice) -> Option<String> {
    let topo: IDeviceTopology = dev.Activate(CLSCTX_ALL, None).ok()?;
    for i in 0..topo.GetConnectorCount().ok()? {
        let Ok(conn) = topo.GetConnector(i) else { continue };
        // Err here just means nothing is connected upstream.
        let Ok(other) = conn.GetConnectedTo() else { continue };
        let Ok(part) = other.cast::<IPart>() else { continue };
        let Ok(other_topo) = part.GetTopologyObject() else { continue };
        let Ok(pw) = other_topo.GetDeviceId() else { continue };
        let s = pw.to_string().ok();
        CoTaskMemFree(Some(pw.0 as *const core::ffi::c_void));
        if let Some(s) = s {
            return Some(s);
        }
    }
    None
}

/// `bthenum` is A2DP, `bthhfenum` hands-free. Anything without `bth` in its
/// device id is not a Bluetooth audio endpoint.
fn is_bluetooth(upstream: &str) -> bool {
    let u = upstream.to_ascii_lowercase();
    u.contains("bthenum") || u.contains("bthhfenum") || u.contains(r"\bth")
}

fn is_a2dp(upstream: &str) -> bool {
    upstream.to_ascii_lowercase().contains("bthenum")
}

/// Every Bluetooth audio device Windows knows about, connected or not.
pub fn devices() -> Vec<Device> {
    crate::audio::init_com_pub();
    let mut by_container: std::collections::HashMap<u128, Device> = Default::default();
    unsafe {
        let Ok(en) =
            CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
        else {
            return Vec::new();
        };
        // Both directions: a headset's microphone is a separate endpoint and it
        // has to be disconnected too, or Windows keeps the link up.
        let Ok(coll) = en.EnumAudioEndpoints(eAll, DEVICE_STATE(DEVICE_STATEMASK_ALL)) else {
            return Vec::new();
        };
        for i in 0..coll.GetCount().unwrap_or(0) {
            let Ok(dev) = coll.Item(i) else { continue };
            let Some(up) = upstream_id(&dev) else { continue };
            if !is_bluetooth(&up) {
                continue;
            }
            let Some(container) = container_of(&dev) else { continue };
            let active = dev.GetState().map(|s| s == DEVICE_STATE_ACTIVE).unwrap_or(false);

            let entry = by_container.entry(container).or_insert_with(|| Device {
                container,
                name: String::new(),
                connected: false,
                output_id: None,
                controls: Vec::new(),
            });
            entry.connected |= active;

            // Name: strip Windows' "Auriculares (Foo Stereo)" wrapper down to
            // something that reads like a device rather than a driver string.
            if entry.name.is_empty() {
                if let Some(n) = friendly_name(&dev) {
                    entry.name = clean_name(&n);
                }
            }
            if is_a2dp(&up) {
                if let Some(id) = device_id_of(&dev) {
                    entry.output_id = Some(id);
                }
            }
            // The KS filter lives on the upstream device, not the endpoint.
            if let Ok(filter) = en.GetDevice(PCWSTR(wide(&up).as_ptr())) {
                if let Ok(ks) = filter.Activate(CLSCTX_ALL, None) {
                    entry.controls.push(ks);
                }
            }
        }
    }
    let mut v: Vec<Device> = by_container.into_values().collect();
    v.sort_by(|a, b| b.connected.cmp(&a.connected).then(a.name.cmp(&b.name)));
    v
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// `Auriculares (AirPods Pro Stereo)` -> `AirPods Pro`.
fn clean_name(raw: &str) -> String {
    let inner = match (raw.find('('), raw.rfind(')')) {
        (Some(a), Some(b)) if b > a + 1 => &raw[a + 1..b],
        _ => raw,
    };
    let mut s = inner.trim();
    for suffix in [" Stereo", " Hands-Free AG Audio", " Hands-Free AG", " Mono"] {
        if let Some(t) = s.strip_suffix(suffix) {
            s = t;
        }
    }
    s.trim().to_string()
}

impl Device {
    fn oneshot(&self, id: u32) -> bool {
        let prop = KSIDENTIFIER {
            Anonymous: KSIDENTIFIER_0 {
                Anonymous: KSIDENTIFIER_0_0 {
                    Set: KSPROPSETID_BT_AUDIO,
                    Id: id,
                    // GET, not SET -- the property's documented usage is a Get
                    // with a NULL value, which the driver treats as the trigger.
                    Flags: KSPROPERTY_TYPE_GET,
                },
            },
        };
        let mut any = false;
        for ks in &self.controls {
            let mut returned = 0u32;
            unsafe {
                any |= ks
                    .KsProperty(
                        &prop,
                        std::mem::size_of::<KSIDENTIFIER>() as u32,
                        std::ptr::null_mut(),
                        0,
                        &mut returned,
                    )
                    .is_ok();
            }
        }
        any
    }

    /// Ask Windows to connect. Success means the driver *attempted* it -- the
    /// documentation is explicit that it is not a confirmation. Watch for the
    /// endpoint reaching ACTIVE to know it really worked.
    pub fn connect(&self) -> bool {
        self.oneshot(KSPROPERTY_ONESHOT_RECONNECT)
    }

    pub fn disconnect(&self) -> bool {
        self.oneshot(KSPROPERTY_ONESHOT_DISCONNECT)
    }
}

/// Poll until this container has a live endpoint, up to `timeout`. Connecting a
/// headset takes seconds and the transition is asynchronous, so making it the
/// default output immediately after `connect()` would target a device that is
/// still UNPLUGGED and silently do nothing.
pub fn wait_connected(container: u128, timeout: std::time::Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if devices().iter().any(|d| d.container == container && d.connected) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    false
}
