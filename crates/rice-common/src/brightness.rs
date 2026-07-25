//! Monitor brightness over DDC/CI -- the same thing Twinkle Tray does for
//! external displays, via the Monitor Configuration API in dxva2.dll.
//!
//! DDC/CI talks to the display's own controller over the video cable, so this
//! changes the panel's real backlight rather than gamma-fading the image the way
//! software dimmers do. Monitors are slow to answer (tens of ms) and some ignore
//! DDC/CI entirely, so every call here is fallible and the caller should treat a
//! failure as "this display can't be controlled" rather than an error.

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy)]
struct PhysicalMonitor {
    handle: isize,
    description: [u16; 128],
}

#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn EnumDisplayMonitors(hdc: isize, clip: *const core::ffi::c_void, proc_: MonEnumProc, data: isize) -> i32;
    fn GetMonitorInfoW(mon: isize, mi: *mut MonitorInfo) -> i32;
}

#[cfg(windows)]
#[link(name = "dxva2")]
extern "system" {
    fn GetNumberOfPhysicalMonitorsFromHMONITOR(mon: isize, count: *mut u32) -> i32;
    fn GetPhysicalMonitorsFromHMONITOR(mon: isize, count: u32, arr: *mut PhysicalMonitor) -> i32;
    fn DestroyPhysicalMonitors(count: u32, arr: *mut PhysicalMonitor) -> i32;
    fn GetMonitorBrightness(h: isize, min: *mut u32, cur: *mut u32, max: *mut u32) -> i32;
    fn SetMonitorBrightness(h: isize, brightness: u32) -> i32;
}

#[cfg(windows)]
type MonEnumProc = extern "system" fn(isize, isize, *mut Rect, isize) -> i32;

#[cfg(windows)]
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[cfg(windows)]
#[repr(C)]
struct MonitorInfo {
    cb: u32,
    rc_monitor: Rect,
    rc_work: Rect,
    flags: u32,
}

/// One controllable display.
#[derive(Clone, Debug)]
pub struct Display {
    /// HMONITOR, so callers can line this up with a monitor rect.
    pub hmonitor: isize,
    /// Left edge in virtual-desktop coordinates -- the same value glaze-bar and
    /// GlazeWM use to identify a monitor.
    pub x: i32,
    pub current: u32,
    pub min: u32,
    pub max: u32,
}

impl Display {
    /// Brightness as 0.0..=1.0 within this monitor's own reported range.
    pub fn fraction(&self) -> f32 {
        let span = (self.max.saturating_sub(self.min)).max(1) as f32;
        ((self.current.saturating_sub(self.min)) as f32 / span).clamp(0.0, 1.0)
    }
}

#[cfg(windows)]
static mut FOUND: Vec<(isize, i32)> = Vec::new();

#[cfg(windows)]
extern "system" fn enum_cb(mon: isize, _hdc: isize, _r: *mut Rect, _d: isize) -> i32 {
    unsafe {
        let mut mi = MonitorInfo {
            cb: std::mem::size_of::<MonitorInfo>() as u32,
            rc_monitor: Rect::default(),
            rc_work: Rect::default(),
            flags: 0,
        };
        let x = if GetMonitorInfoW(mon, &mut mi) != 0 { mi.rc_monitor.left } else { 0 };
        #[allow(static_mut_refs)]
        FOUND.push((mon, x));
    }
    1
}

/// Every display that answers DDC/CI, ordered left to right.
///
/// Each call re-queries the monitor, which costs tens of milliseconds per
/// display -- fine on a click, not something to do per frame.
#[cfg(windows)]
pub fn displays() -> Vec<Display> {
    let mons: Vec<(isize, i32)> = unsafe {
        #[allow(static_mut_refs)]
        {
            FOUND.clear();
            EnumDisplayMonitors(0, std::ptr::null(), enum_cb, 0);
            FOUND.clone()
        }
    };

    let mut out = Vec::new();
    for (hmon, x) in mons {
        unsafe {
            let mut n = 0u32;
            if GetNumberOfPhysicalMonitorsFromHMONITOR(hmon, &mut n) == 0 || n == 0 {
                continue;
            }
            let mut phys = vec![
                PhysicalMonitor { handle: 0, description: [0; 128] };
                n as usize
            ];
            if GetPhysicalMonitorsFromHMONITOR(hmon, n, phys.as_mut_ptr()) == 0 {
                continue;
            }
            for p in &phys {
                let (mut mn, mut cur, mut mx) = (0u32, 0u32, 0u32);
                if GetMonitorBrightness(p.handle, &mut mn, &mut cur, &mut mx) != 0 {
                    out.push(Display { hmonitor: hmon, x, current: cur, min: mn, max: mx });
                }
            }
            DestroyPhysicalMonitors(n, phys.as_mut_ptr());
        }
    }
    out.sort_by_key(|d| d.x);
    out
}

/// Set brightness on one display, given a 0.0..=1.0 fraction of its range.
#[cfg(windows)]
pub fn set(d: &Display, fraction: f32) -> bool {
    let span = (d.max.saturating_sub(d.min)) as f32;
    let value = d.min + (fraction.clamp(0.0, 1.0) * span).round() as u32;
    unsafe {
        let mut n = 0u32;
        if GetNumberOfPhysicalMonitorsFromHMONITOR(d.hmonitor, &mut n) == 0 || n == 0 {
            return false;
        }
        let mut phys = vec![PhysicalMonitor { handle: 0, description: [0; 128] }; n as usize];
        if GetPhysicalMonitorsFromHMONITOR(d.hmonitor, n, phys.as_mut_ptr()) == 0 {
            return false;
        }
        let mut ok = false;
        for p in &phys {
            if SetMonitorBrightness(p.handle, value) != 0 {
                ok = true;
            }
        }
        DestroyPhysicalMonitors(n, phys.as_mut_ptr());
        ok
    }
}

#[cfg(not(windows))]
pub fn displays() -> Vec<Display> {
    Vec::new()
}
#[cfg(not(windows))]
pub fn set(_d: &Display, _f: f32) -> bool {
    false
}
