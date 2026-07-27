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
    #[link(name = "kernel32")]
    extern "system" {
        pub fn CreateEventW(attrs: isize, manual: i32, initial: i32, name: *const u16) -> isize;
        pub fn OpenEventW(access: u32, inherit: i32, name: *const u16) -> isize;
        pub fn SetEvent(h: isize) -> i32;
        pub fn WaitForSingleObject(h: isize, ms: u32) -> u32;
        pub fn CloseHandle(h: isize) -> i32;
    }
    pub const EVENT_MODIFY_STATE: u32 = 0x0002;

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
    show_event: isize,
    /// Set the frame after becoming visible, so focus lands in the box without
    /// being stolen back every frame afterwards.
    focus_next: bool,
    idle_frames: u32,
}

impl App {
    fn new(entries: Vec<Entry>, show_event: isize) -> Self {
        let mut s = Self {
            entries,
            matcher: Matcher::new(Config::DEFAULT),
            query: String::new(),
            hits: Vec::new(),
            selected: 0,
            visible: false,
            show_event,
            focus_next: false,
            idle_frames: 0,
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

    fn launch(&mut self) {
        let Some(&(i, _)) = self.hits.get(self.selected) else { return };
        match &self.entries[i].action {
            Action::Shortcut(p) => open_via_shell(&p.to_string_lossy()),
            // A packaged app is started through the shell, not CreateProcess:
            // an AUMID is not a path and there is no exe to run.
            Action::Aumid(id) => open_via_shell(&format!("shell:AppsFolder\\{id}")),
        }
        self.hide();
    }

    fn hide(&mut self) {
        self.visible = false;
        self.query.clear();
        self.refilter();
    }
}

/// Width and height of the primary monitor.
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

fn open_via_shell(target: &str) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // `explorer.exe <target>` resolves shortcuts and AppsFolder ids alike,
        // and detaches, so the launcher never becomes the parent of what it
        // starts -- closing the launcher must not take the app with it.
        let _ = std::process::Command::new("explorer.exe")
            .arg(target)
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .spawn();
    }
    #[cfg(not(windows))]
    let _ = target;
}

impl eframe::App for App {
    fn clear_color(&self, _v: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, _f: &mut eframe::Frame) {
        // The hotkey sets a named event rather than synthesising input. On this
        // machine synthetic Win presses desynchronise AltSnap and it starts
        // eating the spacebar, so nothing here may depend on sending keys.
        #[cfg(windows)]
        if !self.visible && unsafe { win::WaitForSingleObject(self.show_event, 0) } == 0 {
            self.visible = true;
            self.focus_next = true;
            // Position it explicitly: with none set, Windows put it on whichever
            // monitor it liked -- it opened off on the second screen.
            let (mw, _mh) = primary_size();
            let rows = self.hits.len().max(1) as f32;
            let h = INPUT_H + rows * ROW_H + 16.0;
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
                (mw - WIDTH) / 2.0,
                220.0,
            )));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(WIDTH, h)));
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }

        if !self.visible {
            // Hand the working set back while hidden. An egui/glow process sits
            // near 150 MB otherwise -- the bar reads as 7 MB for exactly this
            // reason. This is hidden almost all the time, so the pages cost
            // nothing to fault back in on the rare open.
            self.idle_frames = self.idle_frames.wrapping_add(1);
            if self.idle_frames % 200 == 0 {
                rice_common::win::trim_ram();
            }
            // Often enough that opening feels instant, rarely enough to be free.
            ctx.request_repaint_after(std::time::Duration::from_millis(60));
            return;
        }

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.hide();
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            return;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
            self.selected = (self.selected + 1).min(self.hits.len().saturating_sub(1));
        }
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
            self.selected = self.selected.saturating_sub(1);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
            self.launch();
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            return;
        }

        let rows = self.hits.len().max(1);
        let want_h = INPUT_H + rows as f32 * ROW_H + 16.0;
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(WIDTH, want_h)));

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
                    .hint_text("buscar...");
                let r = ui.add(te);
                if self.focus_next {
                    r.request_focus();
                    self.focus_next = false;
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
                            egui::Color32::from_rgba_unmultiplied(
                                accent.r(),
                                accent.g(),
                                accent.b(),
                                40,
                            ),
                        );
                    }
                    ui.painter().text(
                        egui::pos2(rect.left() + 10.0, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        &e.name,
                        egui::FontId::proportional(15.0),
                        if row == self.selected { col(theme::TEXT) } else { col(theme::SUBTEXT) },
                    );
                    if matches!(e.action, Action::Aumid(_)) {
                        ui.painter().text(
                            egui::pos2(rect.right() - 10.0, rect.center().y),
                            egui::Align2::RIGHT_CENTER,
                            "store",
                            egui::FontId::proportional(11.0),
                            col(theme::SUBTEXT),
                        );
                    }
                }
            });

        ctx.request_repaint();
    }
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    #[cfg(windows)]
    if args.iter().any(|a| a == "--show") {
        // Signal the resident instance and exit. If there is none, fall through
        // and become it, so the very first hotkey press still opens something.
        if signal_show() {
            return Ok(());
        }
    }
    let _ = args;

    #[cfg(windows)]
    let show_event = unsafe {
        let name = win::wide(SHOW_EVENT);
        // Auto-reset: the wait consumes it, so one signal is exactly one open.
        win::CreateEventW(0, 0, 0, name.as_ptr())
    };
    #[cfg(not(windows))]
    let show_event = 0isize;

    // Index before the window exists. It takes about a second for 519 entries
    // here, and doing it lazily would mean the first open shows an empty list.
    let entries = index::build();

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
            Ok(Box::new(App::new(entries, show_event)))
        }),
    )
}
