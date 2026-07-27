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

mod index;

use eframe::egui;
use index::{Action, Entry};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher};
use rice_common::theme;
use rice_common::ui::col;

const SHOW_EVENT: &str = "Global\\rice-launcher-show";
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
    }

    pub const EVENT_MODIFY_STATE: u32 = 0x0002;
    const SW_HIDE: i32 = 0;
    const SW_SHOW: i32 = 5;

    /// The real HWND, handed over by eframe at creation. Everything else was
    /// guesswork: FindWindowW on the title never located it, and walking this
    /// process's windows did not either -- and both failed SILENTLY, which
    /// turned into a retry loop burning a core.
    pub static HWND: AtomicIsize = AtomicIsize::new(0);

    pub fn hwnd() -> isize {
        HWND.load(Ordering::Relaxed)
    }

    pub fn is_visible() -> bool {
        let h = hwnd();
        h != 0 && unsafe { IsWindowVisible(h) != 0 }
    }

    /// Show centred on the primary monitor and take focus.
    ///
    /// SetForegroundWindow alone is not enough: Windows refuses it for a process
    /// that does not own the foreground, and the box would appear without the
    /// caret, so typing went to whatever was behind it. Attaching to the current
    /// foreground thread first is what makes the grant legal.
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

            let fg = GetForegroundWindow();
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
}

impl App {
    fn new(entries: Vec<Entry>, pending: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Self {
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
        };
        s.refilter();
        s
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
            let mut buf = Vec::new();
            let hay = nucleo_matcher::Utf32Str::new(&e.name, &mut buf);
            if let Some(s) = pat.score(hay, &mut self.matcher) {
                // A hit on the name always outranks one on the folder, or
                // typing "mozilla" would rank a stray helper above Firefox.
                scored.push((i, s + 100));
                continue;
            }
            if e.keywords.is_empty() {
                continue;
            }
            let mut b2 = Vec::new();
            let hay = nucleo_matcher::Utf32Str::new(&e.keywords, &mut b2);
            if let Some(s) = pat.score(hay, &mut self.matcher) {
                scored.push((i, s));
            }
        }
        scored.sort_by(|a, b| b.1.cmp(&a.1));
        scored.truncate(MAX_ROWS);
        self.hits = scored;
    }

    fn launch(&mut self, elevated: bool) {
        if let Some(&(i, _)) = self.hits.get(self.selected) {
            match &self.entries[i].action {
                Action::Shortcut(p) => open_via_shell(&p.to_string_lossy(), elevated),
                // A packaged app cannot be elevated -- Windows has no mechanism
                // for it -- so the flag is ignored rather than quietly starting
                // it unelevated as though it had worked.
                Action::Aumid(id) => open_via_shell(&format!("shell:AppsFolder\\{id}"), false),
            }
        }
        self.close();
    }

    fn open(&mut self, ctx: &egui::Context) {
        // Always a blank box. Reopening onto the last query is never what you
        // want: you press the hotkey to search for something new.
        self.query.clear();
        self.refilter();
        self.visible = true;
        self.opened_at = Some(std::time::Instant::now());
        let rows = self.hits.len().max(1) as f32;
        #[cfg(windows)]
        win::show(WIDTH, INPUT_H + rows * ROW_H + 16.0);
        ctx.request_repaint();
    }

    fn close(&mut self) {
        self.visible = false;
        // Discard what was typed here rather than in open(), so nothing is left
        // sitting in memory while hidden.
        self.query.clear();
        self.refilter();
        #[cfg(windows)]
        {
            win::hide();
        }
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

/// Start something, optionally elevated.
///
/// ShellExecuteW rather than CreateProcess: it resolves .lnk files with their
/// arguments and working directory intact, and its "runas" verb is the only way
/// to ask for elevation -- that is what raises the UAC prompt. It also does not
/// make the launcher the parent of what it starts, so closing this window can
/// never take the launched application with it.
fn open_via_shell(target: &str, elevated: bool) {
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
        ShellExecuteW(
            0,
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1, // SW_SHOWNORMAL
        );
    }
    #[cfg(not(windows))]
    let _ = (target, elevated);
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

        // Closing on focus loss is what makes it feel like a launcher rather
        // than a window. The grace period matters: focus does not arrive on the
        // frame the window appears, so checking immediately closed it before it
        // was ever seen.
        let settled = self
            .opened_at
            .map(|t| t.elapsed() > std::time::Duration::from_millis(250))
            .unwrap_or(true);
        if settled && !ctx.input(|i| i.viewport().focused.unwrap_or(true)) {
            self.close();
            return;
        }

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.close();
            return;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
            self.selected = (self.selected + 1).min(self.hits.len().saturating_sub(1));
        }
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
            self.selected = self.selected.saturating_sub(1);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
            let admin = ctx.input(|i| i.modifiers.ctrl);
            self.launch(admin);
            return;
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
                    self.refilter();
                }
                ui.add_space(6.0);

                let accent = col(theme::ACCENT);
                for (row, &(i, _)) in self.hits.iter().enumerate() {
                    let e = &self.entries[i];
                    let rect = ui.allocate_space(egui::vec2(ui.available_width(), ROW_H - 4.0)).1;
                    if row == self.selected {
                        ui.painter().rect_filled(
                            rect,
                            egui::Rounding::same(8.0),
                            egui::Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 40),
                        );
                    }
                    ui.painter().text(
                        egui::pos2(rect.left() + 10.0, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        &e.name,
                        egui::FontId::proportional(15.0),
                        if row == self.selected { col(theme::TEXT) } else { col(theme::SUBTEXT) },
                    );
                    if row == self.selected {
                        ui.painter().text(
                            egui::pos2(rect.right() - 10.0, rect.center().y),
                            egui::Align2::RIGHT_CENTER,
                            "ctrl+enter = admin",
                            egui::FontId::proportional(11.0),
                            col(theme::SUBTEXT),
                        );
                    }
                }
            });

        // Only repaint while open, and only while focus is still settling or
        // something is animating -- an idle open window does not need frames.
        if !settled {
            ctx.request_repaint();
        }
    }
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
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
            Ok(Box::new(App::new(entries, pending)))
        }),
    )
}
