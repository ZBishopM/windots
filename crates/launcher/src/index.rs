//! Where the launcher's entries come from.
//!
//! Two sources, because Windows has two kinds of application and neither alone
//! is enough: classic programs only ever appear as Start Menu shortcuts, and
//! Store/UWP apps have no shortcut at all -- Claude, the terminal and the
//! Command Palette itself are all invisible if you only walk the Start Menu.

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Entry {
    /// What the user reads and types against.
    pub name: String,
    /// Extra text to match on (folder name, publisher) without showing it.
    pub keywords: String,
    /// How to start it.
    pub action: Action,
}

#[derive(Clone, Debug)]
pub enum Action {
    /// A Start Menu entry, already RESOLVED to its target.
    ///
    /// Deliberately not the .lnk path. Measured on this machine, handing a .lnk
    /// to `ShellExecuteW` blocks **7,067 ms**, while the .exe it points at
    /// blocks **16 ms** -- 440x. The shell re-runs its whole link resolution
    /// (distributed link tracking, volume lookup) on every single launch, and
    /// that was the entire "applications take ten seconds to start" problem.
    ///
    /// Resolving happens once while indexing, with tracking explicitly
    /// disabled, so the cost is paid at startup and never again.
    Shortcut { target: PathBuf, args: String, dir: String },
    /// A packaged app, launched through its AppUserModelID.
    Aumid(String),
    /// A system action. Anything the shell can already start, spelled out --
    /// see `commands::builtin`.
    Command { target: String, args: String },
}

/// Start Menu shortcuts, user's and machine-wide.
fn shortcuts() -> Vec<Entry> {
    let mut out = Vec::new();
    let roots = [
        std::env::var("APPDATA").ok().map(|p| PathBuf::from(p).join(r"Microsoft\Windows\Start Menu\Programs")),
        std::env::var("ProgramData").ok().map(|p| PathBuf::from(p).join(r"Microsoft\Windows\Start Menu\Programs")),
    ];
    for root in roots.into_iter().flatten() {
        walk(&root, &root, &mut out);
    }
    out
}

fn walk(dir: &Path, root: &Path, out: &mut Vec<Entry>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, root, out);
        } else if p.extension().map(|x| x.eq_ignore_ascii_case("lnk")).unwrap_or(false) {
            let Some(name) = p.file_stem().and_then(|s| s.to_str()) else { continue };
            // The containing folder is usually the vendor ("Mozilla Firefox",
            // "NVIDIA Corporation"), which is worth matching on but not worth
            // showing -- it is noise in a list of application names.
            let keywords = p
                .parent()
                .and_then(|d| d.strip_prefix(root).ok())
                .and_then(|d| d.to_str())
                .unwrap_or("")
                .to_string();
            let (target, args, dir) = resolve_lnk(&p);
            out.push(Entry {
                name: name.to_string(),
                keywords,
                action: Action::Shortcut { target, args, dir },
            });
        }
    }
}

/// Packaged (Store/UWP) apps, from the shell's AppsFolder namespace.
///
/// This is the same list the Start menu shows and the only way to reach apps
/// that have no shortcut on disk. Each item's parsing name IS its AUMID.
fn packaged() -> Vec<Entry> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{
        IShellItem, SHCreateItemFromParsingName, SIGDN_NORMALDISPLAY, SIGDN_DESKTOPABSOLUTEPARSING,
    };

    let mut out = Vec::new();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let path: Vec<u16> = "shell:AppsFolder".encode_utf16().chain(Some(0)).collect();
        let Ok(folder) = SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(path.as_ptr()), None)
        else {
            return out;
        };
        let Ok(items) = folder.BindToHandler::<_, windows::Win32::UI::Shell::IEnumShellItems>(
            None,
            &windows::Win32::UI::Shell::BHID_EnumItems,
        ) else {
            return out;
        };
        loop {
            let mut fetched = [None; 1];
            let mut got = 0u32;
            if items.Next(&mut fetched, Some(&mut got)).is_err() || got == 0 {
                break;
            }
            let Some(item) = fetched[0].take() else { break };
            let (Ok(name), Ok(aumid)) = (
                item.GetDisplayName(SIGDN_NORMALDISPLAY),
                item.GetDisplayName(SIGDN_DESKTOPABSOLUTEPARSING),
            ) else {
                continue;
            };
            let (Ok(name), Ok(aumid)) = (name.to_string(), aumid.to_string()) else { continue };
            out.push(Entry {
                name,
                keywords: String::new(),
                action: Action::Aumid(aumid),
            });
        }
    }
    out
}

/// Everything the launcher can start, de-duplicated.
///
/// Store/UWP apps are OFF by default: `launcher.index_store_apps` in rice.json
/// turns them back on. They bring in a long tail of things that are not really
/// applications -- settings pages, help links, "Visit <vendor>" web shortcuts --
/// and every one of them has to be launched indirectly through its AUMID, which
/// also means they cannot be started elevated.
///
/// The cost of leaving them out is real and worth stating: an app that ships
/// ONLY as a packaged app then cannot be found here at all. On this machine that
/// includes Claude and Windows Terminal.
pub fn build() -> Vec<Entry> {
    let mut all = shortcuts();
    if rice_common::settings::Settings::live().launcher.index_store_apps {
        all.extend(packaged());
    }
    // A packaged app often ALSO has a Start Menu shortcut; showing both is just
    // two identical rows with different launch paths.
    all.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    all.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    // Commands last, outside the dedup: they are not applications, and one must
    // not disappear because some installer shipped a shortcut with the same
    // name. Order does not matter -- the matcher scores, it does not scan.
    all.extend(crate::commands::builtin());
    all
}

/// A .lnk's target, arguments and working directory.
///
/// Loads the link and reads the stored path WITHOUT calling `Resolve`. That is
/// the whole point: `Resolve` is what performs distributed link tracking --
/// hunting for a moved target across volumes and the network -- and it is where
/// the seven seconds went. The stored path is right in every case that matters;
/// when it is not, the .lnk itself is kept as a fallback and the shell can do
/// its slow thing exactly once.
#[cfg(windows)]
fn resolve_lnk(lnk: &Path) -> (PathBuf, String, String) {
    use windows::core::{Interface, PCWSTR};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

    let fallback = (lnk.to_path_buf(), String::new(), String::new());
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let Ok(link) = CoCreateInstance::<_, IShellLinkW>(&ShellLink, None, CLSCTX_INPROC_SERVER)
        else {
            return fallback;
        };
        let Ok(file) = link.cast::<IPersistFile>() else { return fallback };
        let w: Vec<u16> = lnk.as_os_str().encode_wide().chain(Some(0)).collect();
        // STGM_READ: sólo se lee. Y NADA de Resolve() después -- ver la nota
        // de arriba, es la llamada que cuesta los siete segundos.
        if file.Load(PCWSTR(w.as_ptr()), windows::Win32::System::Com::STGM_READ).is_err() {
            return fallback;
        }

        let mut buf = [0u16; 1024];
        let mut fd = windows::Win32::Storage::FileSystem::WIN32_FIND_DATAW::default();
        if link.GetPath(&mut buf, &mut fd, 0).is_err() {
            return fallback;
        }
        let target = wide_to_string(&buf);
        if target.is_empty() {
            // Store apps and control-panel links have no filesystem path; the
            // shell has to handle those, so the .lnk stays.
            return fallback;
        }

        let mut ab = [0u16; 1024];
        let args = if link.GetArguments(&mut ab).is_ok() { wide_to_string(&ab) } else { String::new() };
        let mut db = [0u16; 1024];
        let dir = if link.GetWorkingDirectory(&mut db).is_ok() { wide_to_string(&db) } else { String::new() };
        (PathBuf::from(target), args, dir)
    }
}

#[cfg(windows)]
fn wide_to_string(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

#[cfg(not(windows))]
fn resolve_lnk(lnk: &Path) -> (PathBuf, String, String) {
    (lnk.to_path_buf(), String::new(), String::new())
}
