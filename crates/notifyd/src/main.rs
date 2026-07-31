// Supervised daemon: a console here would flash a Windows Terminal window at
// every login and on every supervisor restart.
#![windows_subsystem = "windows"]

//! notifyd -- every Windows notification, redrawn with the rice's own toast.
//!
//! The stock blue banner is not themeable and not suppressible per-notification,
//! so the only way to make the whole system match the rice is to (a) observe
//! every notification, (b) draw our own, and (c) stop Windows drawing its own.
//!
//! (a) `UserNotificationListener` is the ONLY documented way to see notifications
//!     raised by *other* apps. It is gated behind the `userNotificationListener`
//!     capability, which can only be declared in an app manifest -- so this
//!     process needs **package identity**.
//!
//!     The trap is that none of this fails loudly. Without identity `Current()`
//!     succeeds, `RequestAccessAsync()` answers `Allowed`, and the documented
//!     behaviour of `GetNotificationsAsync` is to return an empty list -- a
//!     daemon that silently draws nothing looks exactly like a quiet machine.
//!     So the identity check here is explicit, up front, and logged.
//!
//!     Measured, though, and worth knowing before assuming the package is
//!     mandatory: on this machine (Win11, 2026-07) an UNPACKAGED notifyd got
//!     `Allowed` and 27 real notifications out of `GetNotificationsAsync`. The
//!     poll path works with no package at all. What identity reliably buys is
//!     the `NotificationChanged` subscription, which throws ELEMENT_NOT_FOUND
//!     without it -- so the package turns a 2-second poll into an instant
//!     event, and guarantees the documented behaviour instead of an
//!     undocumented one that a Windows update is free to tighten.
//!
//!     Identity comes from a *sparse package* (dotfiles\notifyd-package): a tiny
//!     MSIX carrying only AppxManifest.xml, with `uap10:AllowExternalContent`,
//!     registered against the folder this exe already lives in. Nothing moves,
//!     the installer keeps its one elevated step, and `cargo build` can keep
//!     overwriting the exe -- external content is not hashed. The other half is
//!     the `<msix>` side-by-side manifest build.rs embeds into this binary;
//!     without it the package registers and the process still has no identity.
//!
//!     notifyd must also run UNELEVATED. Windows will not deliver a COM callback
//!     into a higher integrity level, so an elevated notifyd gets
//!     RPC_S_CALL_FAILED from the subscription and quietly degrades to polling.
//!
//! (b) `shadowplay-notify.exe`, the same toast the recorder and the mic switcher
//!     use, so there is exactly one notification look in the rice.
//!
//! (c) Do Not Disturb, rather than the per-app "Show notification banners"
//!     toggles. Those do the right thing in isolation -- banner off, Notification
//!     Center on, so the listener still sees it -- but only ever for one app.
//!     There is no global version of that switch, so the per-app route means
//!     visiting Settings for every application already installed and then again
//!     for every new one, and each app you forget leaks a blue banner. DND is
//!     the same behaviour applied once, system-wide, to everything including
//!     apps installed later: it suppresses the *banner* while the notification
//!     still lands in the Notification Center and still reaches this listener.
//!     That asymmetry is the whole reason this design works.
//!
//!     The rice does NOT set it: it is a visible system-wide setting and it is
//!     the user's to choose. install.ps1 prints it, notifyd logs whether it is
//!     on, and neither touches it.
//!
//! Consequence worth knowing: with DND on, this daemon is the only thing drawing
//! notifications. If it dies, notifications go silent rather than reverting to
//! blue banners. rice-supervisor.ps1 relaunches it within 30s; the Notification
//! Center (Win+N) is the manual backstop.

use std::collections::{HashSet, VecDeque};
use std::os::windows::process::CommandExt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::Arc;
use std::time::Duration;

use windows::core::PWSTR;
use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::Management::{
    UserNotificationListener, UserNotificationListenerAccessStatus as Access,
};
use windows::UI::Notifications::{
    KnownNotificationBindings, NotificationKinds, ToastNotificationManager, ToastNotificationMode,
    UserNotification, UserNotificationChangedEventArgs, UserNotificationChangedKind,
};
use windows::Win32::Foundation::{APPMODEL_ERROR_NO_PACKAGE, ERROR_INSUFFICIENT_BUFFER};
use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::SystemInformation::GetLocalTime;

use rice_common::event::IslandEvent;
use rice_common::{args, theme, win};

/// How long a redrawn notification stays on screen. Longer than the rice's own
/// 4-5s toasts: these carry text the user has not seen before.
const DEFAULT_HOLD: f32 = 6.0;
/// Fallback poll interval. See `NotificationChanged` below for why polling is
/// not optional. Short enough that a toast still feels like a reaction if the
/// event never fires at all.
const DEFAULT_POLL: f32 = 2.0;
/// Ids remembered so a poll never redraws what the event already drew. Windows
/// keeps ~20 entries per app in the Notification Center, so this is generous.
const SEEN_MAX: usize = 512;
/// A notification storm (a chat sync, a mail client catching up) must not queue
/// minutes of toasts. Past this, drop.
const QUEUE_MAX: usize = 8;

// ---------------------------------------------------------------- log
//
// This is a windowless process: the log file is the ONLY channel it has for
// "your package is not registered". Bounded, because it runs forever.

fn log_path() -> std::path::PathBuf {
    let dir = rice_common::config::config_path("logs");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("notifyd.log")
}

fn log(msg: &str) {
    use std::io::Write;
    let path = log_path();
    if std::fs::metadata(&path).map(|m| m.len() > 128 * 1024).unwrap_or(false) {
        if let Ok(s) = std::fs::read_to_string(&path) {
            let tail: Vec<&str> = s.lines().rev().take(300).collect();
            let keep: Vec<&str> = tail.into_iter().rev().collect();
            let _ = std::fs::write(&path, keep.join("\n"));
        }
    }
    let t = unsafe { GetLocalTime() };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(
            f,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {msg}",
            t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
        );
    }
}

// ---------------------------------------------------------------- identity

/// Package full name, or `None` when this process has no package identity.
///
/// `GetCurrentPackageFullName` answers `APPMODEL_ERROR_NO_PACKAGE` when there is
/// none and `ERROR_INSUFFICIENT_BUFFER` when there is -- a zero-length buffer
/// never "succeeds", so treating anything but INSUFFICIENT_BUFFER as "no
/// identity" is correct.
fn package_full_name() -> Option<String> {
    unsafe {
        let mut len: u32 = 0;
        let rc = GetCurrentPackageFullName(&mut len, PWSTR::null());
        if rc != ERROR_INSUFFICIENT_BUFFER {
            // APPMODEL_ERROR_NO_PACKAGE is the ordinary "unpackaged" answer and
            // is handled by the caller; anything else means something stranger
            // and should not disappear.
            if rc != APPMODEL_ERROR_NO_PACKAGE {
                log(&format!("GetCurrentPackageFullName: unexpected {}", rc.0));
            }
            return None;
        }
        let mut buf = vec![0u16; len as usize];
        if GetCurrentPackageFullName(&mut len, PWSTR(buf.as_mut_ptr())).0 != 0 {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    }
}

const NO_IDENTITY_HELP: &str = "no package identity -- polling still works, but NotificationChanged \
will not subscribe, so notifications arrive up to one poll interval late and anything that never reaches \
the Notification Center is missed entirely. Register the sparse package:  Add-AppxPackage -Path \
%USERPROFILE%\\.config\\notifyd-package\\notifyd-sparse.msix -ExternalLocation \
%USERPROFILE%\\dev\\target\\release  (see dotfiles\\notifyd-package\\build.ps1)";

const DENIED_HELP: &str = "notification access denied -- Settings > Privacy & security > Notifications: \
turn on 'Let apps access your notifications', then allow 'rice notifyd'";

/// The access status as a word. Its derived `Debug` prints the raw discriminant
/// (`UserNotificationListenerAccessStatus(1)`), and this log file is the only
/// thing anyone will have to read when the daemon is not working.
fn access_name(a: Access) -> &'static str {
    match a {
        Access::Allowed => "Allowed",
        Access::Denied => "Denied",
        _ => "Unspecified",
    }
}

// ---------------------------------------------------------------- extraction

struct Toast {
    title: String,
    body: String,
    icon: &'static str,
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

/// Clip on a char boundary. The toast window is a fixed 400x108: anything longer
/// than this is drawn outside it, not wrapped.
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

/// Notifications raised by the rice itself. Without this, anything of ours that
/// ever goes through the Windows toast API would be observed here, redrawn, and
/// -- if the redraw itself raised a toast -- looped. Matched on both the display
/// name and the AUMID because a sparse-packaged app reports the manifest's
/// DisplayName, not the exe name.
const OURS: &[&str] = &[
    "notifyd",
    "shadowplay",
    "glazewm",
    "glaze-bar",
    "micswitch",
    "ws-slide",
    "rice.notifyd",
];

fn is_ours(haystack: &str) -> bool {
    OURS.iter().any(|k| haystack.contains(k))
}

/// Icon keyword for an app. Deliberately coarse: the point is that a chat ping
/// and a mail both read as themselves at a glance, not that every app gets a
/// bespoke glyph.
fn icon_for(haystack: &str) -> &'static str {
    let has = |k: &str| haystack.contains(k);
    if has("discord") || has("whatsapp") || has("telegram") || has("slack") || has("teams")
        || has("signal") || has("messenger") || has("vesktop")
    {
        "message"
    } else if has("mail") || has("outlook") || has("thunderbird") || has("gmail") || has("correo") {
        "mail"
    } else if has("calendar") || has("calendario") {
        "calendar"
    } else {
        "bell"
    }
}

/// Title + body + icon for one observed notification, or `None` when it is ours
/// or carries no text worth drawing (progress-only toasts have none).
fn extract(n: &UserNotification) -> Option<Toast> {
    let info = n.AppInfo().ok();
    let app = info
        .as_ref()
        .and_then(|a| a.DisplayInfo().ok())
        .and_then(|d| d.DisplayName().ok())
        .map(|h| h.to_string())
        .unwrap_or_default();
    let aumid = info
        .as_ref()
        .and_then(|a| a.AppUserModelId().ok())
        .map(|h| h.to_string())
        .unwrap_or_default();

    let key = format!("{app} {aumid}").to_lowercase();
    if is_ours(&key) {
        return None;
    }

    let visual = n.Notification().ok()?.Visual().ok()?;
    // Nearly every modern notification is a ToastGeneric binding; the legacy
    // ToastText0x templates are not, so fall back to whatever binding exists.
    let binding = KnownNotificationBindings::ToastGeneric()
        .ok()
        .and_then(|t| visual.GetBinding(&t).ok())
        .or_else(|| visual.Bindings().ok().and_then(|b| b.GetAt(0).ok()))?;

    let elems = binding.GetTextElements().ok()?;
    let mut texts = Vec::new();
    for i in 0..elems.Size().unwrap_or(0) {
        if let Ok(t) = elems.GetAt(i) {
            let s = t.Text().map(|h| h.to_string()).unwrap_or_default();
            let s = s.trim();
            if !s.is_empty() {
                texts.push(s.to_string());
            }
        }
    }
    if texts.is_empty() {
        return None;
    }

    // The toast has two lines. First text element is the notification's own
    // title; the rest is its body. The app name is appended to the body only
    // when it is not already visible, so "Discord / usuario: hola" doesn't come
    // out as "Discord · Discord".
    let title = texts[0].clone();
    let mut body = texts[1..].join(" \u{b7} ");
    let lower_title = title.to_lowercase();
    let lower_body = body.to_lowercase();
    let app_l = app.to_lowercase();
    if !app.is_empty() && !lower_title.contains(&app_l) && !lower_body.contains(&app_l) {
        body = if body.is_empty() {
            app.clone()
        } else {
            format!("{body}  \u{b7}  {app}")
        };
    }

    Some(Toast {
        title: clip(&title, 44),
        body: clip(&body, 96),
        icon: icon_for(&key),
    })
}

// ---------------------------------------------------------------- display

/// Draw one notification: publish it to the bar's island AND pop the toast.
///
/// Blocking on the child is deliberate -- shadowplay-notify draws at a fixed
/// screen position, so two at once would sit exactly on top of each other. This
/// runs on its own thread precisely so the wait cannot stall the scan loop or a
/// WinRT callback.
fn show(t: &Toast, hold: f32) {
    // One surface by default, not two. Publishing the island event AND popping a
    // toast for the same notification meant it arrived twice over -- three times
    // with the Windows banner still drawn underneath.
    let style = rice_common::settings::Settings::live().notification_style.to_lowercase();
    let want_island = style == "island" || style == "both";
    let want_toast = style != "island";

    // Sobre un juego a pantalla completa, SIEMPRE por la isla y nunca por el
    // toast. No es cuestion de foco -- el toast ya nace con WS_EX_NOACTIVATE y
    // se comprobo que no lo roba -- sino de que es un PROCESO NUEVO con su
    // propia ventana OpenGL, y una ventana nueva encima de un juego en pantalla
    // completa exclusiva le obliga a cambiar de modo. Eso es lo que minimizaba
    // League cada vez que llegaba una notificacion.
    //
    // La isla no crea nada: se dibuja dentro de la ventana que la barra ya tiene
    // abierta y que la barra ya mantiene visible sobre los juegos justo para
    // esto.
    let over_game = win::fullscreen_app_focused();

    let ev = IslandEvent::new(t.icon, &t.title, &t.body, &hex(theme::ACCENT));
    // Al historial SIEMPRE, decida lo que decida el estilo. Ese es justo el
    // problema que resuelve el centro de notificaciones: una notificacion salio,
    // se fue sola, y no habia donde mirar que era.
    if let Err(e) = ev.record() {
        log(&format!("no pude anotar en el historial: {e}"));
    }
    if want_island || over_game {
        let _ = ev.publish();
    }
    if !want_toast || over_game {
        return;
    }

    let exe = win::sibling_exe("shadowplay-notify.exe");
    let mut argv = ev.to_notify_args();
    argv.push("--hold".into());
    argv.push(format!("{hold}"));
    match std::process::Command::new(&exe)
        .args(&argv)
        .creation_flags(win::CREATE_NO_WINDOW)
        .spawn()
    {
        // Exit 10 means the user clicked it. We have nowhere to route that yet
        // (the source app's activation would need its AUMID re-launched), so it
        // is only noted.
        Ok(mut child) => {
            let _ = child.wait();
        }
        Err(e) => log(&format!("spawn {} failed: {e}", exe.display())),
    }
    // Small gap so consecutive toasts read as separate events rather than one
    // redraw.
    std::thread::sleep(Duration::from_millis(180));
}

fn display_thread(rx: Receiver<Toast>, pending: Arc<AtomicUsize>, hold: f32) {
    for t in rx {
        pending.fetch_sub(1, Ordering::Relaxed);
        show(&t, hold);
    }
}

// ---------------------------------------------------------------- seen ids

/// Bounded de-dup set. Both the event and the poll report the same
/// notification, and a notification sitting in the Notification Center is
/// re-listed by every single poll.
struct Seen {
    set: HashSet<u32>,
    order: VecDeque<u32>,
}

impl Seen {
    fn new() -> Self {
        Self { set: HashSet::new(), order: VecDeque::new() }
    }
    /// `true` the first time an id is offered.
    fn insert(&mut self, id: u32) -> bool {
        if !self.set.insert(id) {
            return false;
        }
        self.order.push_back(id);
        if self.order.len() > SEEN_MAX {
            if let Some(old) = self.order.pop_front() {
                self.set.remove(&old);
            }
        }
        true
    }
}

// ---------------------------------------------------------------- scanning

fn enqueue(t: Toast, tx: &Sender<Toast>, pending: &AtomicUsize) {
    if pending.load(Ordering::Relaxed) >= QUEUE_MAX {
        return;
    }
    pending.fetch_add(1, Ordering::Relaxed);
    if tx.send(t).is_err() {
        pending.fetch_sub(1, Ordering::Relaxed);
    }
}

/// One pass over everything currently in the Notification Center.
///
/// `prime` marks the pass as seeding: at startup the centre is full of
/// notifications the user already had, and replaying them as a wall of toasts
/// on every supervisor restart would be worse than the blue banners.
///
/// Returns the number of notifications the listener reported, which is what the
/// identity check is really testing (see `--check`).
fn scan(
    listener: &UserNotificationListener,
    seen: &mut Seen,
    tx: &Sender<Toast>,
    pending: &AtomicUsize,
    prime: bool,
) -> Option<u32> {
    let list = match listener
        .GetNotificationsAsync(NotificationKinds::Toast)
        .and_then(|op| op.get())
    {
        Ok(v) => v,
        Err(e) => {
            // This call is known to throw a spurious NotImplementedException
            // every so often. Swallowing it silently would hide a *permanent*
            // failure, and logging every one would write a line every couple of
            // seconds forever, so log the first and then one in a few hundred.
            static FAILS: AtomicUsize = AtomicUsize::new(0);
            let n = FAILS.fetch_add(1, Ordering::Relaxed);
            if n == 0 || n % 300 == 0 {
                log(&format!("GetNotificationsAsync failed ({n} so far): {e}"));
            }
            return None;
        }
    };
    let n = list.Size().unwrap_or(0);
    for i in 0..n {
        let Ok(un) = list.GetAt(i) else { continue };
        let Ok(id) = un.Id() else { continue };
        if !seen.insert(id) {
            continue;
        }
        if prime {
            continue;
        }
        if let Some(t) = extract(&un) {
            enqueue(t, tx, pending);
        }
    }
    Some(n)
}

/// Handle one id straight from the event, without waiting for a poll.
///
/// Not redundant with `scan`: a notification whose app opts out of the
/// Notification Center never appears in `GetNotificationsAsync`, but the event
/// still fires for it and `GetNotification(id)` still resolves it.
fn scan_one(
    listener: &UserNotificationListener,
    id: u32,
    seen: &mut Seen,
    tx: &Sender<Toast>,
    pending: &AtomicUsize,
) {
    if !seen.insert(id) {
        return;
    }
    match listener.GetNotification(id) {
        Ok(un) => {
            if let Some(t) = extract(&un) {
                enqueue(t, tx, pending);
            }
        }
        Err(e) => log(&format!("GetNotification({id}) failed: {e}")),
    }
}

// ---------------------------------------------------------------- setup

/// Report package identity, and warn once if there is none. Does NOT block.
///
/// The received wisdom is that the listener returns an empty list without
/// identity. Measured on this machine (Win11, 2026-07), unpackaged: access came
/// back `Allowed` and `GetNotificationsAsync` returned 27 real notifications. So
/// the *poll* path works with no package at all; what identity actually buys is
/// the `NotificationChanged` subscription, which throws ELEMENT_NOT_FOUND
/// without it.
///
/// Hence: warn, do not park. Parking would have turned a daemon that works with
/// a couple of seconds of latency into one that draws nothing at all, which is
/// precisely the silent failure this file exists to avoid. It also cannot exit
/// instead -- rice-supervisor relaunches a dead component every 30s, so exiting
/// would re-toast the same warning twice a minute forever.
fn report_identity() -> Option<String> {
    let name = package_full_name();
    match &name {
        Some(n) => log(&format!("package identity: {n}")),
        None => {
            log(NO_IDENTITY_HELP);
            show(
                &Toast {
                    title: "notifyd sin identidad".into(),
                    body: "solo sondeo, sin eventos - ver notifyd.log".into(),
                    icon: "warn",
                },
                8.0,
            );
        }
    }
    name
}

/// Whether Do Not Disturb is on, as a human-readable word.
///
/// Nothing here *changes* the mode -- turning DND on is the user's call and the
/// installer only prints the instruction. But knowing it is the difference
/// between "notifyd is broken" and "DND is off, so Windows is drawing its own
/// banner alongside ours", which look identical in a bug report.
fn dnd_state() -> &'static str {
    match ToastNotificationManager::GetDefault().and_then(|m| m.NotificationMode()) {
        Ok(ToastNotificationMode::Unrestricted) => "off (Windows is also drawing its own banners)",
        Ok(ToastNotificationMode::PriorityOnly) => "on (priority only)",
        Ok(ToastNotificationMode::AlarmsOnly) => "on (alarms only)",
        _ => "unknown",
    }
}

/// Ask for listener access, warning once and retrying until granted.
///
/// This one DOES park, unlike `report_identity`, and the difference is real:
/// without identity the daemon still works via polling, but without access the
/// listener genuinely reports nothing, so there is no degraded mode to run in.
/// The fix is a Settings toggle the user makes later, so it retries rather than
/// giving up.
///
/// Worth knowing: once the user has denied access, `RequestAccessAsync` returns
/// `Denied` instantly and forever without prompting again, and a revoked
/// permission makes `GetNotificationsAsync` return an empty list rather than an
/// error. An empty list is therefore ambiguous on its own, which is why the
/// status is logged next to the count.
fn await_access(listener: &UserNotificationListener) {
    let mut warned = false;
    loop {
        let status = listener
            .RequestAccessAsync()
            .and_then(|op| op.get())
            .unwrap_or(Access::Unspecified);
        if status == Access::Allowed {
            log("listener access: Allowed");
            return;
        }
        if !warned {
            warned = true;
            log(&format!("listener access: {} -- {DENIED_HELP}", access_name(status)));
            show(
                &Toast {
                    title: "notifyd sin permiso".into(),
                    body: "Privacidad > Notificaciones: permitir acceso".into(),
                    icon: "warn",
                },
                8.0,
            );
        }
        std::thread::sleep(Duration::from_secs(60));
    }
}

// ---------------------------------------------------------------- check

/// `notifyd --check`: report the state of every gate and exit.
///
/// Deliberately does NOT reuse the daemon's parking loops -- a diagnostic that
/// blocks for 60s waiting for the thing it is supposed to diagnose is not a
/// diagnostic. Every line it writes is one of the four ways this can be broken:
/// no identity, access denied, an empty list despite both (capability missing),
/// or DND off (so Windows draws its banners on top of ours).
fn run_check(tx: &Sender<Toast>, pending: &AtomicUsize) {
    let identity = package_full_name();
    log(&format!(
        "--check identity: {}",
        identity.clone().unwrap_or_else(|| format!("NONE. {NO_IDENTITY_HELP}"))
    ));

    let listener = match UserNotificationListener::Current() {
        Ok(l) => l,
        Err(e) => {
            log(&format!("--check listener: Current() failed: {e}"));
            return;
        }
    };
    let access = listener
        .RequestAccessAsync()
        .and_then(|op| op.get())
        .unwrap_or(Access::Unspecified);
    log(&format!("--check access: {}", access_name(access)));

    let mut seen = Seen::new();
    let n = scan(&listener, &mut seen, tx, pending, true).unwrap_or(0);
    log(&format!("--check visible: {n} notification(s)"));

    // Name every app currently holding a notification. Without this the only
    // answer to "why didn't my app get intercepted?" is a number, and the two
    // possible causes look identical from outside: the app is missing because it
    // never used the Windows notification system at all (it drew its own
    // window), or it is there and something downstream dropped it.
    if let Ok(list) = listener
        .GetNotificationsAsync(NotificationKinds::Toast)
        .and_then(|op| op.get())
    {
        for i in 0..list.Size().unwrap_or(0) {
            let Ok(un) = list.GetAt(i) else { continue };
            let app = un
                .AppInfo()
                .and_then(|a| a.DisplayInfo())
                .and_then(|d| d.DisplayName())
                .map(|h| h.to_string())
                .unwrap_or_else(|_| "(sin nombre)".into());
            let id = un.Id().unwrap_or(0);
            // Include the first line of text: plenty of apps register no display
            // name, and "(sin nombre)" on its own identifies nothing.
            let t = extract(&un)
                .map(|x| format!("{} / {}", x.title, x.body))
                .unwrap_or_else(|| "(sin texto)".into());
            let t: String = t.chars().take(60).collect();
            log(&format!("--check   app id={id}: {app} | {t}"));
        }
    }
    log(&format!("--check do-not-disturb: {}", dnd_state()));

    let ok = identity.is_some() && access == Access::Allowed;
    show(
        &Toast {
            title: "notifyd".into(),
            body: if identity.is_none() {
                "sin identidad de paquete - ver notifyd.log".into()
            } else if access != Access::Allowed {
                "sin permiso de notificaciones - ver notifyd.log".into()
            } else {
                format!("{n} notificacion(es) visibles")
            },
            icon: if ok { "check" } else { "warn" },
        },
        8.0,
    );
}

// ---------------------------------------------------------------- main

fn main() {
    win::single_instance_or_exit("Global\\rice-notifyd");

    let hold: f32 = args::flag_or("--hold", DEFAULT_HOLD);
    let poll = Duration::from_secs_f32(args::flag_or("--poll", DEFAULT_POLL).clamp(1.0, 60.0));
    let check = args::has_flag("--check");

    // MTA, and this is not a detail. Two things depend on it:
    //  * `IAsyncOperation::get()` blocks the calling thread. On an STA thread
    //    that stalls the apartment's message pump and can deadlock the very call
    //    it is waiting on.
    //  * COM delivers an inbound call to an MTA subscriber on an RPC worker
    //    thread, so there is NOTHING to pump: this daemon needs no GetMessage
    //    loop, unlike taskbar.exe with its WinEvent hook. In an STA the same
    //    callback arrives as a window message instead, and a process that goes
    //    idle without pumping would simply never see NotificationChanged fire.
    //    That is the whole "does the event need a message pump" question: it is
    //    the ordinary COM rule, not anything specific to the listener.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    log(if check { "--check" } else { "starting" });

    let (tx, rx) = std::sync::mpsc::channel::<Toast>();
    let pending = Arc::new(AtomicUsize::new(0));
    {
        let pending = pending.clone();
        std::thread::spawn(move || display_thread(rx, pending, hold));
    }

    if check {
        run_check(&tx, &pending);
        return;
    }

    report_identity();

    let listener = match UserNotificationListener::Current() {
        Ok(l) => l,
        Err(e) => {
            log(&format!("UserNotificationListener::Current failed: {e}"));
            return;
        }
    };
    await_access(&listener);

    let mut seen = Seen::new();

    // Seed: everything already in the Notification Center is backlog.
    let n = scan(&listener, &mut seen, &tx, &pending, true).unwrap_or(0);
    log(&format!(
        "primed with {n} existing notification(s); do-not-disturb {}",
        dnd_state()
    ));

    // The event is the fast path, the poll below is the safety net, and shipping
    // only one of them is a mistake either way.
    //
    // The subscription is the single API in this file that is honest about a
    // missing package identity: it throws ELEMENT_NOT_FOUND (0x80070490) rather
    // than pretending to work. It also throws RPC_S_CALL_FAILED (0x800706BE)
    // when the process is elevated. Neither is fatal here -- the poll still
    // works in both cases -- so the result is logged, not acted on.
    let (wake_tx, wake_rx): (SyncSender<u32>, Receiver<u32>) = std::sync::mpsc::sync_channel(QUEUE_MAX);
    let registered = listener
        .NotificationChanged(&TypedEventHandler::<
            UserNotificationListener,
            UserNotificationChangedEventArgs,
        >::new(move |_, argsv| {
            // Callback contract: this runs on a broker-owned RPC thread. It does
            // no WinRT reads and spawns no processes -- it hands an id to the
            // scan loop and returns immediately. try_send, not send, so a burst
            // can never block the broker.
            if let Some(a) = argsv.as_ref() {
                if a.ChangeKind().unwrap_or(UserNotificationChangedKind::Removed)
                    == UserNotificationChangedKind::Added
                {
                    if let Ok(id) = a.UserNotificationId() {
                        let _ = wake_tx.try_send(id);
                    }
                }
            }
            Ok(())
        }))
        .map_err(|e| log(&format!("NotificationChanged subscribe failed: {e}")))
        .is_ok();
    log(&format!(
        "NotificationChanged registered: {registered}; polling every {:.0}s",
        poll.as_secs_f32()
    ));

    let mut ticks: u64 = 0;
    loop {
        // Woken by the event, or by the poll timeout. Either way the same
        // de-dup decides what is new, so an event that never fires costs only
        // latency and an event that fires twice costs nothing.
        match wake_rx.recv_timeout(poll) {
            Ok(id) => scan_one(&listener, id, &mut seen, &tx, &pending),
            Err(RecvTimeoutError::Timeout) => {}
            // Disconnected must NOT fall through to the scan below. Nothing
            // holds the sender when NotificationChanged fails to register --
            // which is exactly what happens without package identity -- and
            // recv_timeout then returns instantly rather than waiting, so the
            // loop span at full speed. Measured: this process was the single
            // largest CPU consumer on the machine at 4.7%, more than Firefox
            // entire, while a scan itself costs 26ms and the interval is 2s.
            Err(RecvTimeoutError::Disconnected) => {
                std::thread::sleep(poll);
            }
        }
        scan(&listener, &mut seen, &tx, &pending, false);

        ticks += 1;
        // Same cadence and same reasoning as the supervisor's trim: this process
        // is idle almost all the time and should not sit on its pages.
        if ticks % 150 == 0 {
            win::trim_ram();
        }
    }
}
