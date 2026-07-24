#![windows_subsystem = "windows"] // no console

// Generic caelestia/quickshell-style toast (Catppuccin Mocha + Nerd Font icon).
// Reused by the ShadowPlay save step, the mic switcher, and a test command.
//
// Usage:
//   shadowplay-notify --title T --body B [--icon mic|replay|check|info|warn]
//                     [--accent #89b4fa] [--open PATH] [--x N] [--y N] [--hold S]
// --open PATH: clicking the toast opens that file's folder in Explorer.

use eframe::egui;
use egui::{Align2, Color32, FontId, Margin, Rounding, Sense, Stroke, Vec2};
use std::time::Instant;

#[cfg(windows)]
extern "system" {
    fn GetCurrentProcess() -> isize;
    fn K32EmptyWorkingSet(process: isize) -> i32;
}
fn trim_ram() {
    #[cfg(windows)]
    unsafe {
        K32EmptyWorkingSet(GetCurrentProcess());
    }
}

#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn FindWindowW(class_name: *const u16, window_name: *const u16) -> isize;
    fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
    fn SetWindowLongPtrW(hwnd: isize, index: i32, new_long: isize) -> isize;
    fn SetWindowPos(hwnd: isize, after: isize, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32;
}

// True non-activating tool overlay: a topmost window popping over an exclusive/
// borderless-fullscreen game must not yank it out of fullscreen or steal focus.
#[cfg(windows)]
fn harden_overlay() {
    const GWL_EXSTYLE: i32 = -20;
    const WS_EX_NOACTIVATE: isize = 0x0800_0000;
    const WS_EX_TOOLWINDOW: isize = 0x0000_0080;
    const HWND_TOPMOST: isize = -1;
    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOACTIVATE: u32 = 0x0010;
    let title: Vec<u16> = "shadowplay-notify\0".encode_utf16().collect();
    unsafe {
        let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
        if hwnd != 0 {
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW);
            SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
}

const OUT_DUR: f32 = 0.40; // fade-out seconds

// --- warm palette (matches the sunset system theme) ---
const BASE: Color32 = Color32::from_rgb(32, 26, 23); // #201a17 warm dark card
const BORDER: Color32 = Color32::from_rgb(58, 47, 40); // #3a2f28 warm surface
const TEXT: Color32 = Color32::from_rgb(239, 228, 216); // #efe4d8 warm white
const SUBTEXT: Color32 = Color32::from_rgb(179, 162, 147); // #b3a293 warm grey
const DEF_ACCENT: Color32 = Color32::from_rgb(224, 163, 92); // #e0a35c amber

struct Notify {
    title: String,
    body: String,
    icon: String,
    accent: Color32,
    open: Option<String>,
    hold: f32,
    opened: bool,
    start: Instant,
    closing_at: Option<Instant>,
    frame: u32,
}

impl Notify {
    fn open_folder(&self) {
        if let Some(p) = &self.open {
            let _ = std::process::Command::new("explorer").arg(format!("/select,{p}")).spawn();
        }
    }
}

impl eframe::App for Notify {
    fn clear_color(&self, _v: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, _f: &mut eframe::Frame) {
        #[cfg(windows)]
        if self.frame < 3 {
            harden_overlay();
        }

        let now = Instant::now();
        let t = self.start.elapsed().as_secs_f32();
        if self.closing_at.is_none() && t > self.hold {
            self.closing_at = Some(now);
        }
        if ctx.input(|i| i.pointer.any_click()) && !self.opened {
            self.opened = true;
            self.open_folder();
            if self.closing_at.is_none() {
                self.closing_at = Some(now);
            }
        }

        // Appear instantly; cubic ease-out fade.
        let alpha = if let Some(c) = self.closing_at {
            let p = (c.elapsed().as_secs_f32() / OUT_DUR).clamp(0.0, 1.0);
            if p >= 1.0 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            let a = 1.0 - p;
            a * a * a
        } else {
            1.0
        };
        let fade = |c: Color32| {
            Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (c.a() as f32 * alpha).round() as u8)
        };
        let accent = self.accent;
        let accent_soft = Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 38);

        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(fade(BASE))
                    .rounding(Rounding::same(16.0))
                    .inner_margin(Margin::symmetric(14.0, 13.0))
                    .outer_margin(Margin::same(10.0))
                    .stroke(Stroke::new(1.0, fade(BORDER)))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            // accent icon chip
                            let chip = 46.0;
                            let (rect, _) = ui.allocate_exact_size(Vec2::splat(chip), Sense::hover());
                            ui.painter().rect_filled(rect, Rounding::same(13.0), fade(accent_soft));
                            if !self.icon.is_empty() {
                                ui.painter().text(
                                    rect.center(),
                                    Align2::CENTER_CENTER,
                                    &self.icon,
                                    FontId::proportional(23.0),
                                    fade(accent),
                                );
                            }
                            ui.add_space(13.0);
                            ui.vertical(|ui| {
                                ui.add_space(3.0);
                                ui.label(
                                    egui::RichText::new(&self.title).size(15.0).strong().color(fade(TEXT)),
                                );
                                ui.add_space(3.0);
                                ui.label(egui::RichText::new(&self.body).size(12.5).color(fade(SUBTEXT)));
                            });
                        });
                    });
            });

        self.frame = self.frame.wrapping_add(1);
        if self.frame == 30 {
            trim_ram();
        }
        ctx.request_repaint();
    }
}

fn arg(flag: &str) -> Option<String> {
    let a: Vec<String> = std::env::args().collect();
    a.iter().position(|x| x == flag).and_then(|i| a.get(i + 1).cloned())
}

fn parse_accent(s: &str) -> Color32 {
    let h = s.trim_start_matches('#');
    if h.len() == 6 {
        if let (Ok(r), Ok(g), Ok(b)) = (
            u8::from_str_radix(&h[0..2], 16),
            u8::from_str_radix(&h[2..4], 16),
            u8::from_str_radix(&h[4..6], 16),
        ) {
            return Color32::from_rgb(r, g, b);
        }
    }
    DEF_ACCENT
}

// Map an icon keyword to a JetBrainsMono Nerd Font glyph (Font Awesome range).
fn icon_glyph(name: &str) -> String {
    match name {
        "mic" => "\u{f130}",       // fa-microphone
        "mic-off" => "\u{f131}",   // fa-microphone-slash
        "replay" | "save" => "\u{f03d}", // fa-video-camera
        "check" => "\u{f00c}",     // fa-check
        "info" => "\u{f05a}",      // fa-info-circle
        "warn" => "\u{f071}",      // fa-exclamation-triangle
        _ => "",
    }
    .to_string()
}

// JetBrainsMono Nerd Font (installed for the terminal) so the icon glyphs render.
fn load_font(ctx: &egui::Context) {
    let mut candidates = vec!["C:\\Windows\\Fonts\\JetBrainsMonoNerdFont-Regular.ttf".to_string()];
    if let Ok(d) = std::env::var("LOCALAPPDATA") {
        candidates.insert(0, format!("{d}\\Microsoft\\Windows\\Fonts\\JetBrainsMonoNerdFont-Regular.ttf"));
    }
    for path in candidates {
        if let Ok(bytes) = std::fs::read(&path) {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert("jbm".to_owned(), egui::FontData::from_owned(bytes));
            for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts.families.entry(fam).or_default().insert(0, "jbm".to_owned());
            }
            ctx.set_fonts(fonts);
            return;
        }
    }
}

fn main() -> eframe::Result<()> {
    let title = arg("--title").unwrap_or_else(|| "Notificación".into());
    let body = arg("--body").unwrap_or_default();
    let accent = parse_accent(&arg("--accent").unwrap_or_default());
    let icon = icon_glyph(&arg("--icon").unwrap_or_default());
    let open = arg("--open");
    let hold: f32 = arg("--hold").and_then(|s| s.parse().ok()).unwrap_or(5.0);
    let x = arg("--x").and_then(|s| s.parse().ok()).unwrap_or(1490.0);
    let y = arg("--y").and_then(|s| s.parse().ok()).unwrap_or(50.0);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_resizable(false)
            .with_transparent(true)
            .with_active(false)
            .with_inner_size([400.0, 108.0])
            .with_position([x, y])
            .with_title("shadowplay-notify"),
        ..Default::default()
    };
    eframe::run_native(
        "shadowplay-notify",
        options,
        Box::new(move |cc| {
            load_font(&cc.egui_ctx);
            Ok(Box::new(Notify {
                title,
                body,
                icon,
                accent,
                open,
                hold,
                opened: false,
                start: Instant::now(),
                closing_at: None,
                frame: 0,
            }))
        }),
    )
}
