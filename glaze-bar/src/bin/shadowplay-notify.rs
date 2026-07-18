#![windows_subsystem = "windows"] // no console

// Animated native toast for ShadowPlay saves. Materializes (faint -> solid,
// sliding in), holds, then disintegrates (fades + drifts) on timeout or click.
// Rendered continuously so it's smooth at the monitor's 165 Hz. Click opens the
// clip's folder in Explorer. Same egui engine as the status bar.
// Usage: shadowplay-notify.exe "C:\path\to\replay.mp4"

use eframe::egui;
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

const IN_DUR: f32 = 0.32; // materialize
const HOLD: f32 = 5.0; // visible
const OUT_DUR: f32 = 0.40; // disintegrate

fn ease_out_cubic(t: f32) -> f32 {
    let u = 1.0 - t;
    1.0 - u * u * u
}
fn ease_in_cubic(t: f32) -> f32 {
    t * t * t
}

struct Notify {
    path: String,
    opened: bool,
    start: Instant,
    closing_at: Option<Instant>,
    frame: u32,
}

impl Notify {
    fn file_name(&self) -> &str {
        self.path.rsplit(['\\', '/']).next().unwrap_or(&self.path)
    }
    fn open_folder(&self) {
        let _ = std::process::Command::new("explorer")
            .arg(format!("/select,{}", self.path))
            .spawn();
    }
}

impl eframe::App for Notify {
    fn clear_color(&self, _v: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, _f: &mut eframe::Frame) {
        let now = Instant::now();
        let t = self.start.elapsed().as_secs_f32();

        // Begin the exit after the hold window.
        if self.closing_at.is_none() && t > HOLD {
            self.closing_at = Some(now);
        }

        // Click anywhere -> open folder, then disintegrate out.
        let clicked = ctx.input(|i| i.pointer.any_click());
        if clicked && !self.opened {
            self.opened = true;
            self.open_folder();
            if self.closing_at.is_none() {
                self.closing_at = Some(now);
            }
        }

        // Animation curve: alpha 0..1 and a slide/drift offset.
        let (alpha, off_x, off_y) = if let Some(c) = self.closing_at {
            let p = (c.elapsed().as_secs_f32() / OUT_DUR).clamp(0.0, 1.0);
            let e = ease_in_cubic(p);
            if p >= 1.0 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            (1.0 - e, e * 26.0, -e * 16.0) // fade out, drift right + up
        } else {
            let p = (t / IN_DUR).clamp(0.0, 1.0);
            let e = ease_out_cubic(p);
            ((e * 0.15 + e * e * 0.85), (1.0 - e) * 44.0, 0.0) // faint->solid, slide from right
        };

        // Apply the animation alpha to a colour.
        let fade = |c: egui::Color32| {
            egui::Color32::from_rgba_unmultiplied(
                c.r(),
                c.g(),
                c.b(),
                (c.a() as f32 * alpha).round() as u8,
            )
        };

        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                let card = egui::Frame::none()
                    .fill(fade(egui::Color32::from_rgb(26, 26, 36)))
                    .rounding(11.0)
                    .inner_margin(egui::Margin::symmetric(16.0, 12.0))
                    .outer_margin(egui::Margin {
                        left: 18.0 + off_x,
                        top: 16.0 + off_y,
                        right: 6.0,
                        bottom: 6.0,
                    })
                    .stroke(egui::Stroke::new(1.5, fade(egui::Color32::from_rgb(90, 140, 255))));
                card.show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.colored_label(
                            fade(egui::Color32::from_rgb(120, 220, 150)),
                            egui::RichText::new("Replay guardado").size(16.0).strong(),
                        );
                        ui.add_space(5.0);
                        ui.colored_label(fade(egui::Color32::from_rgb(215, 215, 225)), self.file_name());
                        ui.add_space(4.0);
                        ui.colored_label(
                            fade(egui::Color32::from_rgb(140, 160, 215)),
                            "Click para abrir la carpeta",
                        );
                    });
                });
            });

        self.frame = self.frame.wrapping_add(1);
        if self.frame == 30 {
            trim_ram();
        }
        // Render continuously -> smooth at 165 Hz (vsync caps to the refresh).
        ctx.request_repaint();
    }
}

fn main() -> eframe::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_default();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_resizable(false)
            .with_transparent(true)
            .with_inner_size([420.0, 130.0])
            .with_position([1490.0, 50.0])
            .with_title("shadowplay-notify"),
        ..Default::default()
    };
    eframe::run_native(
        "shadowplay-notify",
        options,
        Box::new(move |_cc| {
            Ok(Box::new(Notify {
                path,
                opened: false,
                start: Instant::now(),
                closing_at: None,
                frame: 0,
            }))
        }),
    )
}
