// Cycle the default RECORDING (capture) device between the active mics and print
// the new device's friendly name on stdout. The recorder's mic helper auto-follows
// the default, so this switches what gets recorded (and what every app uses).
//
// Setting the default endpoint has no public API -> we call the undocumented
// IPolicyConfig::SetDefaultEndpoint via its known CLSID/IID and vtable slot.

use std::ffi::c_void;
use windows::core::*; // PROPVARIANT, PWSTR, PCWSTR, GUID, HRESULT, IUnknown, Interface
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::PropertiesSystem::*; // IPropertyStore, PROPERTYKEY

const CLSID_POLICY_CONFIG: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
const IID_IPOLICY_CONFIG: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);
// Only cycle between the real mics (ignore Steam/Oculus/VoiceMeeter virtual inputs).
// Matched case-insensitively against the device friendly name.
const MICS: [&str; 2] = ["hyperx", "snowball"];

// PKEY_Device_FriendlyName
const PKEY_FRIENDLY: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 14,
};

// Vtable of IPolicyConfig up to the one method we need. SetDefaultEndpoint sits at
// slot 13: IUnknown (3) + 10 IPolicyConfig methods we never call.
#[repr(C)]
struct PolicyVtbl {
    query_interface: usize,
    add_ref: usize,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    _stubs: [usize; 10],
    set_default_endpoint: unsafe extern "system" fn(*mut c_void, PCWSTR, i32) -> HRESULT,
}

// Endpoint id string of a device (COM-allocated -> freed here).
unsafe fn endpoint_id(dev: &IMMDevice) -> Option<String> {
    let p = dev.GetId().ok()?;
    let s = p.to_string().ok();
    CoTaskMemFree(Some(p.0 as *const c_void));
    s
}

unsafe fn friendly_name(dev: &IMMDevice) -> Option<String> {
    let store: IPropertyStore = dev.OpenPropertyStore(STGM_READ).ok()?;
    let pv: PROPVARIANT = store.GetValue(&PKEY_FRIENDLY).ok()?;
    let ws = PropVariantToStringAlloc(&pv).ok()?;
    let s = ws.to_string().ok();
    CoTaskMemFree(Some(ws.0 as *const c_void));
    s
}

fn main() -> Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;

        // All active capture endpoints, in enumeration order.
        let coll = enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)?;
        let count = coll.GetCount()?;
        if count == 0 {
            eprintln!("no active capture devices");
            return Ok(());
        }
        let mut ids = Vec::new();
        let mut names = Vec::new();
        for i in 0..count {
            let dev = coll.Item(i)?;
            ids.push(endpoint_id(&dev).unwrap_or_default());
            names.push(friendly_name(&dev).unwrap_or_else(|| "?".into()));
        }

        // Keep only the real mics (HyperX / Snowball), in enumeration order.
        let picks: Vec<usize> = (0..names.len())
            .filter(|&i| {
                let n = names[i].to_lowercase();
                MICS.iter().any(|m| n.contains(m))
            })
            .collect();
        if picks.is_empty() {
            eprintln!("no HyperX/Snowball mic active");
            return Ok(());
        }

        // Current default -> next real mic (wraps). If the default isn't one of them,
        // jump to the first real mic.
        let cur = enumerator
            .GetDefaultAudioEndpoint(eCapture, eConsole)
            .ok()
            .and_then(|d| endpoint_id(&d))
            .unwrap_or_default();
        let next = match picks.iter().position(|&i| ids[i] == cur) {
            Some(p) => picks[(p + 1) % picks.len()],
            None => picks[0],
        };

        // Set it default for all three roles via IPolicyConfig.
        let unk: IUnknown = CoCreateInstance(&CLSID_POLICY_CONFIG, None, CLSCTX_ALL)?;
        let mut pc: *mut c_void = std::ptr::null_mut();
        unk.query(&IID_IPOLICY_CONFIG, &mut pc).ok()?;
        let vtbl = *(pc as *const *const PolicyVtbl);
        let wid: Vec<u16> = ids[next].encode_utf16().chain(Some(0)).collect();
        for role in [0i32, 1, 2] {
            let _ = ((*vtbl).set_default_endpoint)(pc, PCWSTR(wid.as_ptr()), role);
        }
        ((*vtbl).release)(pc);

        // Print the new device name for the notification.
        println!("{}", names[next]);
    }
    Ok(())
}
