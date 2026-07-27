//! Where the launcher's entries come from.
//!
//! Two sources, because Windows has two kinds of application and neither alone
//! is enough: classic programs only ever appear as Start Menu shortcuts, and
//! Store/UWP apps have no shortcut at all -- Claude, the terminal and the
//! Command Palette itself are all invisible if you only walk the Start Menu.

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
    /// A .lnk; let the shell resolve it so its arguments and working directory
    /// are honoured.
    Shortcut(PathBuf),
    /// A packaged app, launched through its AppUserModelID.
    Aumid(String),
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
            out.push(Entry {
                name: name.to_string(),
                keywords,
                action: Action::Shortcut(p),
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
    all
}
