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

#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn FindWindowW(class_name: *const u16, window_name: *const u16) -> isize;
    fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
    fn SetWindowLongPtrW(hwnd: isize, index: i32, new_long: isize) -> isize;
    fn SetWindowPos(hwnd: isize, after: isize, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32;
}

// Turn the toast into a true non-activating tool overlay. Without WS_EX_NOACTIVATE
// a topmost window popping over an exclusive/borderless-fullscreen game yanks it
// out of fullscreen (minimizes it). This keeps foreground on the game.
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

const HOLD: f32 = 5.0; // visible
const OUT_DUR: f32 = 0.45; // fade out

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
        // Re-assert the non-activating overlay styles for the first few frames (the
        // window/title may not exist on frame 0).
        #[cfg(windows)]
        if self.frame < 3 {
            harden_overlay();
        }

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

        // Appear instantly at full opacity; fade out cleanly (cubic ease-out).
        let alpha = if let Some(c) = self.closing_at {
            let p = (c.elapsed().as_secs_f32() / OUT_DUR).clamp(0.0, 1.0);
            if p >= 1.0 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            let a = 1.0 - p;
            a * a * a // cubic fade out
        } else {
            1.0 // visible immediately
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
                        left: 18.0,
                        top: 16.0,
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
    // Args: 1 = clip path, 2 = x, 3 = y (top-left of the toast, virtual-desktop
    // pixels). x/y let the caller place it on the focused monitor; without them
    // it falls back to the primary monitor's top-right (all monitors are 100%).
    let path = std::env::args().nth(1).unwrap_or_default();
    let x = std::env::args().nth(2).and_then(|s| s.parse::<f32>().ok()).unwrap_or(1490.0);
    let y = std::env::args().nth(3).and_then(|s| s.parse::<f32>().ok()).unwrap_or(50.0);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_resizable(false)
            .with_transparent(true)
            .with_active(false) // don't steal focus from the game/app when it pops
            .with_inner_size([420.0, 130.0])
            .with_position([x, y])
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
