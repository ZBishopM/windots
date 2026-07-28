// No console: this is a hotkey-driven overlay.
#![windows_subsystem = "windows"]

//! The Win+Space search box.
//!
//! Replaces PowerToys' Command Palette, which on this machine cost 267 MB
//! resident and 59 seconds of cold start at every login -- measured, that was
//! the single largest item in the whole post-boot phase.
//!
//! It stays resident like the bar does, because a launcher that has to start
//! before it can search is a launcher you stop using. Resident here means the
//! same order as the bar (single-digit MB), not a quarter of a gigabyte.
//!
//!   launcher            run the resident instance
//!   launcher --show     tell the resident instance to open (what the hotkey does)

mod commands;
mod files;
mod icons;
mod index;

use eframe::egui;
use index::{Action, Entry};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher};
use rice_common::theme;
use rice_common::ui::col;

const SHOW_EVENT: &str = "Global\\rice-launcher-show";

/// How long the accepted row stays lit before the box disappears.
///
/// It is an acknowledgement, not an animation. Pressing Enter used to produce
/// nothing at all on screen for as long as the launch took, and the launch takes
/// a while (see `launch_async`), so the box just sat there looking frozen.
const FLASH_MS: u64 = 110;

/// Donde empieza el texto de una fila. Fijo, con icono o sin el.
const TEXT_X: f32 = 40.0;

const MAX_ROWS: usize = 9;
const ROW_H: f32 = 34.0;
const INPUT_H: f32 = 46.0;
const WIDTH: f32 = 620.0;

#[cfg(windows)]
mod win {
    use std::sync::atomic::{AtomicIsize, Ordering};

    #[link(name = "kernel32")]
    extern "system" {
        pub fn CreateEventW(attrs: isize, manual: i32, initial: i32, name: *const u16) -> isize;
        pub fn OpenEventW(access: u32, inherit: i32, name: *const u16) -> isize;
        pub fn SetEvent(h: isize) -> i32;
        pub fn WaitForSingleObject(h: isize, ms: u32) -> u32;
        pub fn CloseHandle(h: isize) -> i32;
    }
    #[link(name = "user32")]
    extern "system" {
        pub fn ShowWindow(h: isize, cmd: i32) -> i32;
        pub fn SetForegroundWindow(h: isize) -> i32;
        pub fn IsWindowVisible(h: isize) -> i32;
        pub fn SetWindowPos(h: isize, after: isize, x: i32, y: i32, cx: i32, cy: i32, f: u32) -> i32;
        pub fn GetSystemMetrics(i: i32) -> i32;
        pub fn AttachThreadInput(from: u32, to: u32, attach: i32) -> i32;
        pub fn GetWindowThreadProcessId(h: isize, pid: *mut u32) -> u32;
        pub fn GetForegroundWindow() -> isize;
        pub fn GetCurrentThreadId() -> u32;
        pub fn GetWindowRect(h: isize, r: *mut core::ffi::c_void) -> i32;
        pub fn IsWindow(h: isize) -> i32;
        pub fn GetWindow(h: isize, cmd: u32) -> isize;
        pub fn GetWindowLongPtrW(h: isize, i: i32) -> isize;
    }

    pub const EVENT_MODIFY_STATE: u32 = 0x0002;
    const SW_HIDE: i32 = 0;
    const SW_SHOW: i32 = 5;

    /// The real HWND, handed over by eframe at creation. Everything else was
    /// guesswork: FindWindowW on the title never located it, and walking this
    /// process's windows did not either -- and both failed SILENTLY, which
    /// turned into a retry loop burning a core.
    pub static HWND: AtomicIsize = AtomicIsize::new(0);

    /// Whoever owned the foreground before we took it, so it can be handed back.
    ///
    /// Parking the window off-screen does NOT take its focus away: the closed
    /// box went on owning the keyboard from -30000,-30000, so every keystroke
    /// afterwards went into a window nobody could see. It also fed the far more
    /// visible symptom -- being foreground already, the next open had no focus
    /// change to make, so nothing told the toolkit it was focused and the box
    /// closed itself a quarter of a second later, every time.
    pub static PREV_FG: AtomicIsize = AtomicIsize::new(0);

    pub fn hwnd() -> isize {
        HWND.load(Ordering::Relaxed)
    }

    /// Does the OS think we have the keyboard?
    ///
    /// The authority on purpose. eframe's `viewport().focused` is bookkeeping
    /// derived from focus messages, and it was observed reading false while
    /// `GetForegroundWindow` returned this very window.
    pub fn has_focus() -> bool {
        let h = hwnd();
        h != 0 && unsafe { GetForegroundWindow() } == h
    }

    /// Ask for the keyboard.
    ///
    /// `SetForegroundWindow` alone is not enough: Windows refuses it for a
    /// process that does not own the foreground, and the box would then appear
    /// without a caret, so typing went to whatever was behind it. Attaching to
    /// the current foreground thread first is what makes the grant legal.
    pub fn take_foreground() {
        let h = hwnd();
        if h == 0 {
            return;
        }
        unsafe {
            let fg = GetForegroundWindow();
            if fg == h {
                return;
            }
            if fg != 0 {
                PREV_FG.store(fg, Ordering::Relaxed);
            }
            let mut other_pid = 0u32;
            let other = GetWindowThreadProcessId(fg, &mut other_pid);
            let me = GetCurrentThreadId();
            if other != 0 && other != me {
                AttachThreadInput(other, me, 1);
                SetForegroundWindow(h);
                AttachThreadInput(other, me, 0);
            } else {
                SetForegroundWindow(h);
            }
        }
    }

    pub fn is_visible() -> bool {
        let h = hwnd();
        h != 0 && unsafe { IsWindowVisible(h) != 0 }
    }

    /// Show centred on the primary monitor and take focus.
    pub fn show(width: f32, height: f32) {
        let h = hwnd();
        if h == 0 {
            return;
        }
        unsafe {
            let sw = GetSystemMetrics(0) as f32;
            let x = ((sw - width) / 2.0) as i32;
            const SWP_NOZORDER: u32 = 0x0004;
            SetWindowPos(h, 0, x, 220, width as i32, height as i32, SWP_NOZORDER);
            ShowWindow(h, SW_SHOW);
        }
        take_foreground();
    }

    /// Resize in place: same position, same z-order, same focus.
    ///
    /// File results land after the box is already open and being typed into, so
    /// it has to grow. Going through `show()` for that would re-centre it and
    /// re-take the foreground on every batch.
    pub fn resize(width: f32, height: f32) {
        let h = hwnd();
        if h == 0 {
            return;
        }
        unsafe {
            const SWP_NOMOVE: u32 = 0x0002;
            const SWP_NOZORDER: u32 = 0x0004;
            const SWP_NOACTIVATE: u32 = 0x0010;
            SetWindowPos(h, 0, 0, 0, width as i32, height as i32,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }

    /// "Hidden" means parked off-screen, NOT ShowWindow(SW_HIDE).
    ///
    /// Hiding it outright cost a full core: with no visible surface there is
    /// nothing for SwapBuffers to sync to, so the GL loop free-runs. Measured --
    /// one winit thread Running continuously while update() ran once in twelve
    /// seconds, so the spin was below our code entirely. glaze-bar, same stack,
    /// idles at 0.15% because its window is always visible and vsync paces it.
    ///
    /// Off-screen keeps the window real and paced, and the user cannot see it.
    pub fn hide() {
        let h = hwnd();
        if h == 0 {
            return;
        }
        unsafe {
            const SWP_NOZORDER: u32 = 0x0004;
            const SWP_NOACTIVATE: u32 = 0x0010;
            const SWP_NOSIZE: u32 = 0x0001;
            SetWindowPos(h, 0, -30000, -30000, 0, 0, SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOSIZE);

        }
        release_foreground();
    }

    /// Hand the keyboard to somebody else.
    ///
    /// Moving a window off-screen does not take its focus away, so without this
    /// the closed box went on owning every keystroke from -30000,-30000. Windows
    /// normally does this for us when a window is destroyed or minimised, and we
    /// can do neither: the window has to stay alive AND visible, because a
    /// window with no visible surface has nothing to vsync against and its GL
    /// loop free-runs a whole core.
    ///
    /// Giving the foreground away needs no attach dance -- the process that owns
    /// it is allowed to pass it on.
    pub fn release_foreground() {
        let h = hwnd();
        if h == 0 {
            return;
        }
        unsafe {
            if GetForegroundWindow() != h {
                return;
            }
            // Whoever had it before us, if that is still a real window.
            let prev = PREV_FG.load(Ordering::Relaxed);
            PREV_FG.store(0, Ordering::Relaxed);
            if prev != 0 && prev != h && IsWindow(prev) != 0 && activatable(prev) {
                SetForegroundWindow(prev);
                if GetForegroundWindow() != h {
                    return;
                }
            }
            // Nothing recorded, or it refused: walk down the z-order and give it
            // to the first window that can actually take it. Reached whenever
            // the box is closed twice in a row -- the second open never took the
            // foreground from anyone, so there was nobody recorded to give it
            // back to, and it stayed ours forever.
            const GW_HWNDNEXT: u32 = 2;
            let mut w = GetWindow(h, GW_HWNDNEXT);
            let mut hops = 0;
            while w != 0 && hops < 200 {
                hops += 1;
                if activatable(w) {
                    SetForegroundWindow(w);
                    if GetForegroundWindow() != h {
                        return;
                    }
                }
                w = GetWindow(w, GW_HWNDNEXT);
            }
        }
    }

    /// Can this window sensibly be handed the keyboard?
    ///
    /// Excludes our own overlays and everything else that is on screen without
    /// wanting focus: the bars are click-through tool windows, and handing the
    /// foreground to one is how the first attempt at this silently did nothing.
    unsafe fn activatable(w: isize) -> bool {
        const GW_OWNER: u32 = 4;
        const GWL_STYLE: i32 = -16;
        const GWL_EXSTYLE: i32 = -20;
        const WS_DISABLED: isize = 0x0800_0000;
        const WS_MINIMIZE: isize = 0x2000_0000;
        const WS_EX_TOOLWINDOW: isize = 0x0000_0080;
        const WS_EX_TRANSPARENT: isize = 0x0000_0020;
        const WS_EX_NOACTIVATE: isize = 0x0800_0000;

        if IsWindowVisible(w) == 0 || GetWindow(w, GW_OWNER) != 0 {
            return false;
        }
        let style = GetWindowLongPtrW(w, GWL_STYLE);
        if style & (WS_DISABLED | WS_MINIMIZE) != 0 {
            return false;
        }
        let ex = GetWindowLongPtrW(w, GWL_EXSTYLE);
        if ex & (WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE) != 0 {
            return false;
        }
        // Something real, not a 1x1 helper parked somewhere.
        let mut r = [0i32; 4];
        if GetWindowRect(w, r.as_mut_ptr() as *mut _) == 0 {
            return false;
        }
        r[0] > -10000 && (r[2] - r[0]) > 200 && (r[3] - r[1]) > 100
    }

    /// Off-screen windows still receive frames, so the waiter thread only has to
    /// nudge egui -- no ShowWindow needed. Kept as a named no-op so the call
    /// site keeps reading as "wake the loop".
    pub fn wake() {}

    /// Is the window somewhere the user can actually see?
    pub fn on_screen() -> bool {
        let h = hwnd();
        if h == 0 {
            return false;
        }
        let mut r = [0i32; 4];
        unsafe { GetWindowRect(h, r.as_mut_ptr() as *mut _) };
        r[0] > -10000
    }

    pub fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

/// `--show` from a second process: poke the event the resident one waits on.
/// Returns true if a resident instance was there to poke.
#[cfg(windows)]
fn signal_show() -> bool {
    unsafe {
        let name = win::wide(SHOW_EVENT);
        let h = win::OpenEventW(win::EVENT_MODIFY_STATE, 0, name.as_ptr());
        if h == 0 {
            return false;
        }
        win::SetEvent(h);
        win::CloseHandle(h);
        true
    }
}

/// Que icono pedir para un resultado de archivo: (clave, ruta, por-extension).
///
/// Casi todo va por extension, porque el icono de un `.rs` no depende de que
/// `.rs` sea y asi dos millones de archivos son un punado de consultas. Las
/// excepciones son los tipos donde el icono ES la informacion -- un ejecutable
/// o un acceso directo se reconocen por su icono, no por su extension -- y esos
/// se piden por ruta.
fn icon_request(h: &files::FileHit) -> (String, String, u32) {
    if h.is_dir {
        return ("dir".into(), "carpeta".into(), icons::DIR);
    }
    let ext = h
        .name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    if matches!(ext.as_str(), "exe" | "lnk" | "msi" | "ico" | "cpl" | "scr") {
        (h.path.clone(), h.path.clone(), icons::REAL)
    } else if ext.is_empty() || ext.len() > 8 {
        ("sin-extension".into(), "archivo".into(), icons::FILE)
    } else {
        (format!(".{ext}"), format!("archivo.{ext}"), icons::FILE)
    }
}

/// Do we have the keyboard?
///
/// On Windows this asks the OS rather than the toolkit. eframe's
/// `viewport().focused` is bookkeeping derived from focus messages, and it was
/// measured reading false while `GetForegroundWindow` returned this very window
/// -- which closed the box a quarter of a second after every open.
fn window_focused(ctx: &egui::Context) -> bool {
    #[cfg(windows)]
    {
        let _ = ctx;
        win::has_focus()
    }
    #[cfg(not(windows))]
    ctx.input(|i| i.viewport().focused.unwrap_or(true))
}

/// How many application rows to leave room for once files are also showing.
///
/// Applications go first because they are what the box is for most of the time,
/// but letting a query like "s" fill all nine rows with applications would hide
/// the file results entirely.
const APP_ROWS: usize = 4;

struct App {
    entries: Vec<Entry>,
    matcher: Matcher,
    query: String,
    hits: Vec<(usize, u32)>,
    selected: usize,
    visible: bool,
    /// Set by the waiter thread when the hotkey fires.
    pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// When it last opened, for the focus grace period.
    opened_at: Option<std::time::Instant>,
    idle: u32,
    started: bool,
    /// `None` when `launcher.index_files` is off, in which case nothing is
    /// walked and nothing is held in memory.
    files: Option<files::FileIndex>,
    file_hits: Vec<files::FileHit>,
    icons: icons::Icons,
    /// Todo lo ejecutable del PATH, recorrido una vez.
    runner: commands::Runner,
    /// La fila de "ejecutar esto", si lo escrito da para una. Va la primera
    /// porque es la más literal de todas: el usuario ha escrito un comando, no
    /// ha pedido que se adivine nada.
    run_row: Option<commands::Run>,
    /// Set when Enter was accepted. The chosen row stays lit for [`FLASH_MS`],
    /// then the box closes. Without it the only sign anything happened was the
    /// application's own window, seconds later.
    flash: Option<std::time::Instant>,
    /// Rows the window was last sized for. File results arrive after the window
    /// is already open, so it has to grow to fit them.
    sized_rows: usize,
}

impl App {
    fn new(
        entries: Vec<Entry>,
        pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
        ctx: &egui::Context,
    ) -> Self {
        let files = rice_common::settings::Settings::live().launcher.index_files.then(|| {
            let ctx = ctx.clone();
            files::FileIndex::new(std::sync::Arc::new(move || ctx.request_repaint()))
        });
        let icons = {
            let ctx = ctx.clone();
            icons::Icons::new(std::sync::Arc::new(move || ctx.request_repaint()))
        };
        let mut s = Self {
            entries,
            matcher: Matcher::new(Config::DEFAULT),
            query: String::new(),
            hits: Vec::new(),
            selected: 0,
            visible: false,
            pending,
            opened_at: None,
            idle: 0,
            started: false,
            files,
            icons,
            runner: commands::Runner::new(),
            run_row: None,
            file_hits: Vec::new(),
            flash: None,
            sized_rows: 0,
        };
        s.refilter();
        s
    }

    /// The run row: zero or one.
    fn run_n(&self) -> usize {
        usize::from(self.run_row.is_some())
    }

    /// Application rows on screen.
    fn app_shown(&self) -> usize {
        let cap = if self.file_hits.is_empty() { MAX_ROWS } else { APP_ROWS };
        self.hits.len().min(cap.saturating_sub(self.run_n()))
    }

    /// File rows on screen: whatever the rest left over.
    fn file_shown(&self) -> usize {
        self.file_hits.len().min(MAX_ROWS - self.run_n() - self.app_shown())
    }

    fn rows(&self) -> usize {
        self.run_n() + self.app_shown() + self.file_shown()
    }

    fn refilter(&mut self) {
        self.selected = 0;
        if self.query.trim().is_empty() {
            // Empty query shows the first entries rather than nothing, so the
            // window is never a blank box.
            self.hits = (0..self.entries.len().min(MAX_ROWS)).map(|i| (i, 0)).collect();
            return;
        }
        let pat = Pattern::parse(&self.query, CaseMatching::Ignore, Normalization::Smart);
        let mut scored: Vec<(usize, u32)> = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            // Curated commands edge out applications on an equal match. There
            // are two dozen of them and every one was written on purpose, while
            // the Start Menu is full of near-misses that happen to share a word:
            // typing "desinstalar" matched six vendor "Desinstalar <producto>"
            // stubs before it reached our own uninstaller. Deliberately smaller
            // than the name-hit bonus below, so an application matched by NAME
            // still beats a command matched only by keyword.
            let curated = if matches!(e.action, Action::Command { .. }) { 40 } else { 0 };
            let mut buf = Vec::new();
            let hay = nucleo_matcher::Utf32Str::new(&e.name, &mut buf);
            if let Some(s) = pat.score(hay, &mut self.matcher) {
                // A hit on the name always outranks one on the folder, or
                // typing "mozilla" would rank a stray helper above Firefox.
                scored.push((i, s + 100 + curated));
                continue;
            }
            if e.keywords.is_empty() {
                continue;
            }
            let mut b2 = Vec::new();
            let hay = nucleo_matcher::Utf32Str::new(&e.keywords, &mut b2);
            if let Some(s) = pat.score(hay, &mut self.matcher) {
                scored.push((i, s + curated));
            }
        }
        scored.sort_by(|a, b| b.1.cmp(&a.1));
        scored.truncate(MAX_ROWS);
        self.hits = scored;
    }

    /// Everything that has to happen when the typed text changes.
    ///
    /// The file search is fired and forgotten: it runs on its own thread and
    /// asks for a repaint when it has an answer. Waiting for it here would mean
    /// dropping a frame on every keystroke, since a full sweep of two million
    /// entries is tens of milliseconds.
    fn on_query_changed(&mut self) {
        self.run_row = self.runner.offer(&self.query);
        self.refilter();
        if let Some(f) = &self.files {
            f.search(&self.query, MAX_ROWS);
        }
        self.file_hits.clear();
    }

    fn launch(&mut self, elevated: bool) {
        let run = self.run_n();
        let apps = self.app_shown();
        let target = if run == 1 && self.selected == 0 {
            self.run_row.as_ref().map(|r| (r.target.clone(), r.args.clone(), elevated))
        } else if self.selected - run < apps {
            self.hits.get(self.selected - run).map(|&(i, _)| match &self.entries[i].action {
                Action::Shortcut(p) => (p.to_string_lossy().into_owned(), String::new(), elevated),
                // A packaged app cannot be elevated -- Windows has no mechanism
                // for it -- so the flag is ignored rather than quietly starting
                // it unelevated as though it had worked.
                Action::Aumid(id) => (format!("shell:AppsFolder\\{id}"), String::new(), false),
                Action::Command { target, args } => (target.clone(), args.clone(), elevated),
            })
        } else {
            // A folder opens in Explorer and a file in whatever owns it: the
            // same call either way, since the shell decides from the target.
            self.file_hits
                .get(self.selected - run - apps)
                .map(|h| (h.path.clone(), String::new(), elevated))
        };
        let Some((target, args, elevated)) = target else {
            self.close();
            return;
        };
        launch_async(target, args, elevated);
        // Deliberately NOT closing yet. The lit row is the only acknowledgement
        // the keypress gets, and the application's own window is seconds away.
        self.flash = Some(std::time::Instant::now());
    }

    fn open(&mut self, ctx: &egui::Context) {
        // Always a blank box. Reopening onto the last query is never what you
        // want: you press the hotkey to search for something new.
        self.query.clear();
        self.on_query_changed();
        self.flash = None;
        self.visible = true;
        self.opened_at = Some(std::time::Instant::now());
        self.sized_rows = self.rows().max(1);
        #[cfg(windows)]
        win::show(WIDTH, Self::height_for(self.sized_rows));
        ctx.request_repaint();
    }

    fn close(&mut self) {
        self.visible = false;
        self.flash = None;
        // Discard what was typed here rather than in open(), so nothing is left
        // sitting in memory while hidden.
        self.query.clear();
        self.on_query_changed();
        #[cfg(windows)]
        {
            win::hide();
        }
    }

    fn height_for(rows: usize) -> f32 {
        INPUT_H + rows.max(1) as f32 * ROW_H + 16.0
    }

    /// Fit the window to the rows it currently has, without touching where it is
    /// or who has focus.
    ///
    /// Growing must NOT go through `show()`: that re-centres and re-takes the
    /// foreground, so every batch of file results arriving would yank focus
    /// again mid-typing.
    fn refit(&mut self) {
        let rows = self.rows().max(1);
        if rows == self.sized_rows {
            return;
        }
        self.sized_rows = rows;
        #[cfg(windows)]
        win::resize(WIDTH, Self::height_for(rows));
    }
}

fn primary_size() -> (f32, f32) {
    #[cfg(windows)]
    unsafe {
        #[link(name = "user32")]
        extern "system" {
            fn GetSystemMetrics(i: i32) -> i32;
        }
        (GetSystemMetrics(0) as f32, GetSystemMetrics(1) as f32)
    }
    #[cfg(not(windows))]
    (1920.0, 1080.0)
}

/// Start something without blocking the interface.
///
/// Measured on this machine, `ShellExecuteW` takes **440-930 ms** on a warm
/// cache and **over 5 seconds** cold. It ran on the UI thread, before the window
/// was hidden, so the search box stayed on screen frozen for exactly that long
/// after Enter -- which is the whole of "no feedback, and it feels slow".
///
/// A thread per launch rather than a worker queue: `ShellExecuteW` goes through
/// arbitrary shell extensions, and one that decides to block would put every
/// later launch behind it. Spawning costs microseconds next to half a second.
fn launch_async(target: String, args: String, elevated: bool) {
    std::thread::Builder::new()
        .name("shell-exec".into())
        .spawn(move || open_via_shell(&target, &args, elevated))
        .ok();
}

/// Start something, optionally elevated.
///
/// ShellExecuteW rather than CreateProcess: it resolves .lnk files with their
/// arguments and working directory intact, and its "runas" verb is the only way
/// to ask for elevation -- that is what raises the UAC prompt. It also does not
/// make the launcher the parent of what it starts, so closing this window can
/// never take the launched application with it.
fn open_via_shell(target: &str, args: &str, elevated: bool) {
    #[cfg(windows)]
    unsafe {
        #[link(name = "shell32")]
        extern "system" {
            fn ShellExecuteW(
                hwnd: isize,
                verb: *const u16,
                file: *const u16,
                params: *const u16,
                dir: *const u16,
                show: i32,
            ) -> isize;
        }
        let verb: Vec<u16> = if elevated { "runas\0".encode_utf16().collect() } else { "open\0".encode_utf16().collect() };
        let file: Vec<u16> = target.encode_utf16().chain(Some(0)).collect();
        let params: Vec<u16> = args.encode_utf16().chain(Some(0)).collect();
        ShellExecuteW(
            0,
            verb.as_ptr(),
            file.as_ptr(),
            if args.is_empty() { std::ptr::null() } else { params.as_ptr() },
            std::ptr::null(),
            1, // SW_SHOWNORMAL
        );
    }
    #[cfg(not(windows))]
    let _ = (target, args, elevated);
}

impl eframe::App for App {
    fn clear_color(&self, _v: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, _f: &mut eframe::Frame) {
        // Hide here rather than at creation: eframe presents a frame after the
        // window is built, and that present is what makes it visible, so a hide
        // in the constructor is undone immediately.

        // One signal = one toggle. The hotkey sets a named event; a thread
        // blocked on it flips this and wakes us. Nothing polls, and nothing
        // synthesises keystrokes -- synthetic Win presses desynchronise AltSnap
        // on this machine and it starts swallowing the spacebar.
        if self.pending.swap(false, std::sync::atomic::Ordering::Relaxed) {
            if self.visible {
                self.close();
            } else {
                self.open(ctx);
            }
        }

        if !self.visible {
            self.idle = self.idle.wrapping_add(1);
            if self.idle % 40 == 0 {
                rice_common::win::trim_ram();
            }
            // Self-correcting rather than a one-shot hide at startup: eframe
            // presents a frame after building the window, and that present is
            // what puts it on screen -- after any hide we do in the first
            // update. Parking it whenever it is on screen while we consider
            // ourselves closed settles it without guessing at frame counts.
            #[cfg(windows)]
            if win::on_screen() {
                win::hide();
            }
            // Explicitly ask for nothing for a long time. Returning with NO
            // repaint request at all leaves eframe's control flow on Poll, and
            // winit then spins its event loop -- measured, one thread Running
            // with a full core burned while the window was hidden and update()
            // ran once in twelve seconds. A far-future wake puts it on Wait.
            ctx.request_repaint_after(std::time::Duration::from_secs(3600));
            return;
        }

        // A launch was accepted: hold the lit row, then go. Input is ignored
        // meanwhile -- further keys would act on a choice already committed.
        let flashing = match self.flash {
            Some(t) if t.elapsed() >= std::time::Duration::from_millis(FLASH_MS) => {
                self.close();
                return;
            }
            Some(_) => {
                ctx.request_repaint();
                true
            }
            None => false,
        };

        // Closing on focus loss is what makes it feel like a launcher rather
        // than a window. The grace period matters: focus does not arrive on the
        // frame the window appears, so checking immediately closed it before it
        // was ever seen.
        let settled = self
            .opened_at
            .map(|t| t.elapsed() > std::time::Duration::from_millis(250))
            .unwrap_or(true);
        // ...but not while flashing: the application being started takes the
        // foreground, and closing on that would swallow the acknowledgement.
        let focused = window_focused(ctx);
        if settled && !flashing && !focused {
            // Not ours yet is not the same as taken from us. Asking again for a
            // few hundred milliseconds covers the case where the grant was
            // refused on the first try; only after that does giving up mean the
            // user really has clicked somewhere else.
            let gave_up = self
                .opened_at
                .map(|t| t.elapsed() > std::time::Duration::from_millis(600))
                .unwrap_or(true);
            if gave_up {
                self.close();
                return;
            }
            #[cfg(windows)]
            win::take_foreground();
            ctx.request_repaint();
        }

        // The file search publishes from its own thread and asks for a repaint;
        // this is where those results are picked up. Cheap: at most nine rows.
        if let Some(f) = &self.files {
            if !self.query.trim().is_empty() {
                self.file_hits = f.results();
            }
        }
        self.refit();

        if !flashing {
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.close();
                return;
            }
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                self.selected = (self.selected + 1).min(self.rows().saturating_sub(1));
            }
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                self.selected = self.selected.saturating_sub(1);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                let admin = ctx.input(|i| i.modifiers.ctrl);
                self.launch(admin);
            }
        }

        // Los iconos se resuelven ANTES de dibujar. Dentro del cierre no cabe:
        // pedirlos toma `&mut self.icons` mientras el cierre ya tiene prestado
        // `self` para leer la fila seleccionada.
        let run_n = self.run_n();
        let apps_n = self.app_shown();
        let files_n = self.file_shown();
        let mut row_icons: Vec<Option<egui::TextureId>> =
            Vec::with_capacity(run_n + apps_n + files_n);
        if let Some(r) = self.run_row.clone() {
            // El icono del propio ejecutable, cuando el destino es una ruta.
            // Para `ms-settings:` o una URL no hay nada que sacar.
            let id = if r.target.contains('\\') {
                self.icons.get(ctx, &r.target, &r.target, icons::REAL)
            } else {
                None
            };
            row_icons.push(id);
        }
        for &(i, _) in self.hits.iter().take(apps_n) {
            let id = match &self.entries[i].action {
                Action::Shortcut(p) => {
                    let s = p.to_string_lossy().into_owned();
                    self.icons.get(ctx, &s, &s, icons::REAL)
                }
                // Un AUMID no es una ruta y el shell no le saca icono por aqui.
                Action::Aumid(_) => None,
                // `rundll32.exe` o `shell:RecycleBinFolder`: el shell resuelve
                // los dos, pero solo el primero es un archivo con icono. Y hay
                // que darle la ruta entera -- un nombre suelto lo busca contra
                // el directorio actual, donde no está.
                Action::Command { target, .. } => {
                    let t = if target.contains('\\') {
                        Some(target.clone())
                    } else {
                        self.runner.resolve(target).map(|p| p.to_string_lossy().into_owned())
                    };
                    match t {
                        Some(t) => self.icons.get(ctx, &t, &t, icons::REAL),
                        None => None,
                    }
                }
            };
            row_icons.push(id);
        }
        for k in 0..files_n {
            let (key, path, attrs) = icon_request(&self.file_hits[k]);
            row_icons.push(self.icons.get(ctx, &key, &path, attrs));
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(col(theme::SURFACE))
                    .rounding(egui::Rounding::same(14.0))
                    .inner_margin(egui::Margin::symmetric(14.0, 8.0)),
            )
            .show(ctx, |ui| {
                let te = egui::TextEdit::singleline(&mut self.query)
                    .desired_width(f32::INFINITY)
                    .frame(false)
                    .font(egui::FontId::proportional(20.0))
                    .hint_text("buscar aplicaciones...");
                let r = ui.add(te);
                // Keep the caret until focus has actually settled, or the first
                // characters typed go nowhere.
                if !settled {
                    r.request_focus();
                }
                if r.changed() {
                    self.on_query_changed();
                }
                ui.add_space(6.0);

                let accent = col(theme::ACCENT);
                let run = self.run_n();
                let apps = self.app_shown();
                let files_n = self.file_shown();

                // One closure for both kinds of row so the highlight, the
                // padding and the trailing hint cannot drift apart -- two copies
                // of this in an earlier screen is exactly how the island's close
                // logic ended up resetting three of seven flags.
                let mut row = |ui: &mut egui::Ui,
                               n: usize,
                               name: &str,
                               trail: &str,
                               dim: bool,
                               icon: Option<egui::TextureId>| {
                    let rect = ui.allocate_space(egui::vec2(ui.available_width(), ROW_H - 4.0)).1;
                    let on = n == self.selected;
                    if on {
                        // Same colour, four times the presence: the accepted row
                        // has to read as a different thing from the merely
                        // selected one at a glance, in about a tenth of a second.
                        let a = if flashing { 170 } else { 40 };
                        ui.painter().rect_filled(
                            rect,
                            egui::Rounding::same(8.0),
                            egui::Color32::from_rgba_unmultiplied(
                                accent.r(), accent.g(), accent.b(), a,
                            ),
                        );
                    }
                    let name_col = if on {
                        col(theme::TEXT)
                    } else if dim {
                        col(theme::SUBTEXT)
                    } else {
                        col(theme::TEXT)
                    };
                    if let Some(id) = icon {
                        ui.painter().image(
                            id,
                            egui::Rect::from_center_size(
                                egui::pos2(rect.left() + 20.0, rect.center().y),
                                egui::vec2(20.0, 20.0),
                            ),
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    }
                    // El texto arranca siempre en el mismo sitio, haya icono o
                    // no: si se moviera, las filas bailarian mientras los iconos
                    // van llegando desde su hilo.
                    let w = ui
                        .painter()
                        .text(
                            egui::pos2(rect.left() + TEXT_X, rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            name,
                            egui::FontId::proportional(15.0),
                            name_col,
                        )
                        .width();
                    if !trail.is_empty() {
                        // The folder goes after the name, greyed. It is what
                        // tells four files called main.rs apart, so it is not
                        // decoration -- but it must never outshout the name.
                        ui.painter().text(
                            egui::pos2(rect.left() + TEXT_X + 10.0 + w, rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            trail,
                            egui::FontId::proportional(12.0),
                            col(theme::SUBTEXT),
                        );
                    }
                };

                if let Some(r) = &self.run_row {
                    row(ui, 0, &r.label, &r.hint, false, row_icons.first().copied().flatten());
                }
                for (n, &(i, _)) in self.hits.iter().take(apps).enumerate() {
                    let e = &self.entries[i];
                    let hint = match &e.action {
                        // Se marca lo que NO es una aplicación, para que una
                        // fila que apaga el equipo no se confunda con una que
                        // abre un programa.
                        Action::Command { .. } => "comando",
                        _ => "",
                    };
                    row(ui, run + n, &e.name, hint, false, row_icons.get(run + n).copied().flatten());
                }
                for (k, h) in self.file_hits.iter().take(files_n).enumerate() {
                    // The name is already the tail of the path; showing the
                    // folder alone keeps the row from repeating itself.
                    let folder = h.path.strip_suffix(&h.name).unwrap_or(&h.path);
                    let folder = folder.trim_end_matches('\\');
                    row(
                        ui,
                        run + apps + k,
                        &h.name,
                        folder,
                        true,
                        row_icons.get(run + apps + k).copied().flatten(),
                    );
                }
            });

        // Only repaint while open, and only while focus is still settling or
        // something is animating -- an idle open window does not need frames.
        if !settled {
            ctx.request_repaint();
        }
    }
}

/// `--bench-index`: walk, then write timings to a file and exit.
///
/// A file rather than stdout because this is a `windows_subsystem = "windows"`
/// binary -- it has no console to print to, and giving it one just for a
/// measurement would change what is being measured.
fn bench_index(out: &str) {
    let t0 = std::time::Instant::now();
    let idx = files::FileIndex::start_now(std::sync::Arc::new(|| {}));
    let mut last = 0usize;
    let mut log = String::new();
    while idx.scanning() {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let n = idx.len();
        log.push_str(&format!(
            "{:6.2}s  {:>9} entradas  (+{})\n",
            t0.elapsed().as_secs_f32(),
            n,
            n - last
        ));
        last = n;
    }
    log.push_str(&format!(
        "\nrecorrido completo: {:.2}s, {} entradas\n",
        t0.elapsed().as_secs_f32(),
        idx.len()
    ));

    // Dos vueltas: la primera con las cachés frías, la segunda es lo que siente
    // el usuario al teclear la segunda letra.
    for q in ["s", "sy", "main.rs", "mnrs", "rice.json", "glazewm.exe", "carpeta"] {
        let mut best = f32::MAX;
        let mut hits = Vec::new();
        for _ in 0..3 {
            let t = std::time::Instant::now();
            idx.search(q, 9);
            // La búsqueda va en su propio hilo; aquí se espera a que publique
            // los de ESTA consulta. Esperar a "hay algo" devolvía los de la
            // anterior al instante y daba 0.0 ms en todas.
            while !idx.settled() && t.elapsed().as_millis() < 5000 {
                std::thread::yield_now();
            }
            hits = idx.results();
            best = best.min(t.elapsed().as_secs_f32() * 1000.0);
        }
        log.push_str(&format!("\nbuscar {q:?}: {best:.1} ms, {} mostrados\n", hits.len()));
        for h in hits.iter().take(4) {
            log.push_str(&format!("    {}\n", h.path));
        }
    }
    log.push_str(&format!("\nRSS: {} MB\n", rss_mb()));
    let _ = std::fs::write(out, log);
}

fn rss_mb() -> u64 {
    #[cfg(windows)]
    unsafe {
        #[link(name = "psapi")]
        extern "system" {
            fn GetProcessMemoryInfo(p: isize, c: *mut u8, cb: u32) -> i32;
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentProcess() -> isize;
        }
        // PROCESS_MEMORY_COUNTERS: cb, PageFaultCount, then five SIZE_T of which
        // WorkingSetSize is the second.
        let mut buf = [0u8; 80];
        if GetProcessMemoryInfo(GetCurrentProcess(), buf.as_mut_ptr(), 80) != 0 {
            let ws = usize::from_ne_bytes(buf[16..24].try_into().unwrap());
            return (ws / 1024 / 1024) as u64;
        }
        0
    }
    #[cfg(not(windows))]
    0
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--bench-floor") {
        let (n, s) = files::bench_floor();
        let _ = std::fs::write(
            args.get(i + 1).map(|s| s.as_str()).unwrap_or("floor.txt"),
            format!("suelo del recorrido: {n} entradas en {s:.2}s\nRSS: {} MB\n", rss_mb()),
        );
        return Ok(());
    }
    if let Some(i) = args.iter().position(|a| a == "--bench-index") {
        bench_index(args.get(i + 1).map(|s| s.as_str()).unwrap_or("bench.txt"));
        return Ok(());
    }
    #[cfg(windows)]
    if args.iter().any(|a| a == "--show") {
        // Signal the resident instance and exit. If there is none, fall through
        // and become it, so the first hotkey press still opens something.
        if signal_show() {
            return Ok(());
        }
    }
    let _ = args;

    #[cfg(windows)]
    let show_event = unsafe {
        let name = win::wide(SHOW_EVENT);
        // Auto-reset: the wait consumes it, so one signal is exactly one toggle.
        win::CreateEventW(0, 0, 0, name.as_ptr())
    };
    #[cfg(not(windows))]
    let show_event = 0isize;

    // Index before the window exists: about a second for 500-odd entries here,
    // and doing it lazily would mean the first open shows an empty list.
    let entries = index::build();
    let pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_resizable(false)
            .with_transparent(true)
            .with_visible(false)
            .with_inner_size([WIDTH, INPUT_H + 9.0 * ROW_H])
            .with_title("rice-launcher"),
        ..Default::default()
    };
    eframe::run_native(
        "rice-launcher",
        options,
        Box::new(move |cc| {
            rice_common::ui::load_nerd_font(&cc.egui_ctx);
            #[cfg(windows)]
            {
                // The HWND, straight from eframe. Guessing at it cost most of a
                // debugging session: by title and by process walk, both silent
                // failures that became retry loops.
                use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                if let Ok(h) = cc.window_handle() {
                    if let RawWindowHandle::Win32(w) = h.as_raw() {
                        win::HWND.store(
                            isize::from(w.hwnd),
                            std::sync::atomic::Ordering::Relaxed,
                        );
                    }
                }
                let p = pending.clone();
                let ctx = cc.egui_ctx.clone();
                std::thread::spawn(move || loop {
                    // INFINITE: this thread costs nothing until the hotkey fires.
                    let r = unsafe { win::WaitForSingleObject(show_event, u32::MAX) };
                    if r != 0 {
                        // A bad handle would return instantly forever; sleeping
                        // keeps that from becoming a spin loop.
                        std::thread::sleep(std::time::Duration::from_millis(500));
                        continue;
                    }
                    p.store(true, std::sync::atomic::Ordering::Relaxed);
                    win::wake();
                    ctx.request_repaint();
                });
            }
            Ok(Box::new(App::new(entries, pending, &cc.egui_ctx)))
        }),
    )
}
