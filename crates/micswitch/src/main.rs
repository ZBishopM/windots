// Cycle the default RECORDING (capture) device between the active mics and print
// the new device's friendly name on stdout. The recorder's mic helper auto-follows
// the default, so this switches what gets recorded (and what every app uses).
//
// Setting the default endpoint has no public API -> we call the undocumented
// IPolicyConfig::SetDefaultEndpoint via its known CLSID/IID and vtable slot.

use std::ffi::c_void;
use windows::core::*; // PROPVARIANT, PWSTR, PCWSTR, GUID, HRESULT, IUnknown, Interface
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::PropertiesSystem::*; // IPropertyStore, PROPERTYKEY

const CLSID_POLICY_CONFIG: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
const IID_IPOLICY_CONFIG: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);
// Only cycle between the real mics (ignore Steam/Oculus/VoiceMeeter virtual
// inputs). Matched case-insensitively against the device friendly name, and read
// from ~/.config/rice.json ("mics") so new hardware doesn't mean a recompile.
// Defaults to hyperx + snowball when the file is absent.
fn mics() -> &'static [String] {
    &rice_common::settings::Settings::get().mics
}

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
    // `--output` cycles the default PLAYBACK device instead of the microphone.
    // Same COM path, just the other data flow -- which is all AudioSwitch was
    // running in the background to do.
    let argv: Vec<String> = std::env::args().collect();
    let output = argv.iter().any(|a| a == "--output" || a == "-o");
    let flow = if output { eRender } else { eCapture };
    // `--list` prints the endpoints; `--set <substring>` jumps straight to one.
    // Cycling is fine for two mics, but there are a dozen playback endpoints here
    // (virtual cables, HDMI outputs, VoiceMeeter, Steam), so blind cycling is not
    // a usable way to pick one.
    let list = argv.iter().any(|a| a == "--list" || a == "-l");
    let want = argv
        .iter()
        .position(|a| a == "--set" || a == "-s")
        .and_then(|i| argv.get(i + 1))
        .map(|s| s.to_lowercase());
    // `--level` imprime el nivel de cada endpoint; `--level <0-100>` lo fija en
    // el actual. Es el porcentaje que se ve en la pagina de sonido de Windows
    // (el escalar del endpoint), NO el "refuerzo de microfono" de +10/+20/+30 dB,
    // que es una ganancia aparte y no se toca aqui: sube tambien el ruido de
    // fondo, que es justo lo que no se quiere en una toma de voz.
    let nivel_pos = argv.iter().position(|a| a == "--level" || a == "-v");
    let nivel_nuevo: Option<f32> = nivel_pos
        .and_then(|i| argv.get(i + 1))
        .and_then(|s| s.parse::<f32>().ok())
        .map(|v| (v / 100.0).clamp(0.0, 1.0));

    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;

        // All active endpoints for this direction, in enumeration order.
        let coll = enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)?;
        let count = coll.GetCount()?;
        if count == 0 {
            eprintln!("no active {} devices", if output { "playback" } else { "capture" });
            return Ok(());
        }
        let mut ids = Vec::new();
        let mut names = Vec::new();
        for i in 0..count {
            let dev = coll.Item(i)?;
            ids.push(endpoint_id(&dev).unwrap_or_default());
            names.push(friendly_name(&dev).unwrap_or_else(|| "?".into()));
        }

        let cur_id_de = |e: &IMMDeviceEnumerator| {
            e.GetDefaultAudioEndpoint(flow, eConsole)
                .ok()
                .and_then(|d| endpoint_id(&d))
                .unwrap_or_default()
        };

        if nivel_pos.is_some() {
            let cur_id = cur_id_de(&enumerator);
            // Con valor: se fija en el predeterminado (o en el que diga --set).
            if let Some(v) = nivel_nuevo {
                let objetivo = want
                    .as_ref()
                    .and_then(|w| (0..names.len()).find(|&i| names[i].to_lowercase().contains(w.as_str())))
                    .or_else(|| (0..ids.len()).find(|&i| ids[i] == cur_id));
                let Some(i) = objetivo else {
                    eprintln!("no encontre el dispositivo");
                    return Ok(());
                };
                let vol: IAudioEndpointVolume = coll.Item(i as u32)?.Activate(CLSCTX_ALL, None)?;
                vol.SetMasterVolumeLevelScalar(v, std::ptr::null())?;
                println!("{}: {:.0}%", names[i], v * 100.0);
                return Ok(());
            }
            // Sin valor: solo informar, de todos.
            for i in 0..names.len() {
                let marca = if ids[i] == cur_id { "*" } else { " " };
                match coll
                    .Item(i as u32)
                    .and_then(|d| d.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None))
                    .and_then(|v| v.GetMasterVolumeLevelScalar().map(|s| (v, s)))
                {
                    Ok((v, s)) => {
                        let mudo = v.GetMute().map(|m| m.as_bool()).unwrap_or(false);
                        println!(
                            "{marca} {:>3.0}%{} {}",
                            s * 100.0,
                            if mudo { " [MUDO]" } else { "       " },
                            names[i]
                        );
                    }
                    Err(_) => println!("{marca}   ?%        {}", names[i]),
                }
            }
            return Ok(());
        }

        if list {
            let cur_id = cur_id_de(&enumerator);
            for (i, n) in names.iter().enumerate() {
                println!("{} {}", if ids[i] == cur_id { "*" } else { " " }, n);
            }
            return Ok(());
        }

        // Mics are filtered to the configured allowlist (ignoring Steam/Oculus/
        // VoiceMeeter virtual inputs); outputs cycle through everything active,
        // since there is no equivalent noise to filter out.
        let picks: Vec<usize> = if let Some(w) = &want {
            let m: Vec<usize> = (0..names.len())
                .filter(|&i| names[i].to_lowercase().contains(w.as_str()))
                .collect();
            if m.is_empty() {
                eprintln!("no device matching '{w}'");
                return Ok(());
            }
            m
        } else if output {
            (0..names.len()).collect()
        } else {
            (0..names.len())
                .filter(|&i| {
                    let n = names[i].to_lowercase();
                    mics().iter().any(|m| n.contains(m.as_str()))
                })
                .collect()
        };
        if picks.is_empty() {
            eprintln!("no configured mic active (see mics in ~/.config/rice.json)");
            return Ok(());
        }

        // Current default -> next real mic (wraps). If the default isn't one of them,
        // jump to the first real mic.
        let cur = enumerator
            .GetDefaultAudioEndpoint(flow, eConsole)
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
