#![windows_subsystem = "windows"] // no console

// Generic caelestia/quickshell-style toast, warm palette + Nerd Font icon.
// Reused by the ShadowPlay save step, the mic switcher, and a test command.
//
// Usage:
//   shadowplay-notify --title T --body B [--icon mic|replay|check|rec|info|warn|term]
//                     [--accent #e0a35c] [--open PATH] [--x N] [--y N] [--hold S]
// --open PATH: clicking the toast opens that file's folder in Explorer.
//
// Exit code says how the toast ended: 0 = faded out on its own, 10 = the user
// clicked it. Firefox's AutoConfig (dotfiles/firefox/config.js) drives the whole
// notification system through this binary and turns a 10 into the alerts
// service's "alertclickcallback" observer topic, which is what focuses the tab
// that posted the notification.

use eframe::egui;
use egui::{Align2, Color32, FontId, Margin, Rounding, Sense, Stroke, Vec2};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use rice_common::ui::col;
use rice_common::{args, theme, win};

#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn FindWindowW(class_name: *const u16, window_name: *const u16) -> isize;
}

// True non-activating tool overlay: a topmost window popping over an exclusive/
// borderless-fullscreen game must not yank it out of fullscreen or steal focus.
#[cfg(windows)]
fn harden_overlay() {
    let title = win::wide("shadowplay-notify");
    unsafe {
        let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
        if hwnd != 0 {
            win::harden_overlay(hwnd);
        }
    }
}

const OUT_DUR: f32 = 0.40; // fade-out seconds
const EXIT_CLICKED: i32 = 10;

/// Set once, read after the event loop returns. A static rather than a field on
/// `Notify` because the app is moved into eframe's closure and never handed back.
static CLICKED: AtomicBool = AtomicBool::new(false);

// Palette comes from rice_common::theme so the toast and the bar can't drift
// apart again (they had: different card, text and subtext greys).
const BASE: Color32 = col(theme::SURFACE);
const BORDER: Color32 = col(theme::HIGHLIGHT);
const TEXT: Color32 = col(theme::TEXT);
const SUBTEXT: Color32 = col(theme::SUBTEXT);
const DEF_ACCENT: Color32 = col(theme::ACCENT);

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
            CLICKED.store(true, Ordering::Relaxed);
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
            win::trim_ram();
        }
        ctx.request_repaint();
    }
}

fn main() -> eframe::Result<()> {
    let title = args::flag("--title").unwrap_or("Notificación").to_string();
    let body = args::flag("--body").unwrap_or_default().to_string();
    // Accent and the icon table now come from the shared theme/ui modules; the
    // local icon table was missing `term` and `rec`, so `--icon term` rendered
    // nothing while the bar drew a terminal glyph for the same keyword.
    let accent = args::flag("--accent")
        .and_then(theme::parse_hex)
        .map(col)
        .unwrap_or(DEF_ACCENT);
    let icon = rice_common::ui::icon_glyph(args::flag("--icon").unwrap_or_default()).to_string();
    let open = args::flag("--open").map(str::to_string);
    let hold: f32 = args::flag_or("--hold", 5.0);
    let x: f32 = args::flag_or("--x", 1490.0);
    let y: f32 = args::flag_or("--y", 50.0);

    let options = eframe::NativeOptions {
        viewport: rice_common::ui::overlay_viewport("shadowplay-notify", [400.0, 108.0], [x, y])
            .with_active(false), // never steal focus from a fullscreen game
        ..Default::default()
    };
    eframe::run_native(
        "shadowplay-notify",
        options,
        Box::new(move |cc| {
            rice_common::ui::load_nerd_font(&cc.egui_ctx);
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
    )?;

    if CLICKED.load(Ordering::Relaxed) {
        std::process::exit(EXIT_CLICKED);
    }
    Ok(())
}
