#![windows_subsystem = "windows"] // no console window

use eframe::egui;
use serde::Deserialize;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ---- Release physical RAM while idle (pages -> standby, fault back on demand) ----
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

// Debug log to %TEMP%\glaze-bar.log when GLAZEBAR_LOG is set.
fn dlog(msg: &str) {
    if std::env::var_os("GLAZEBAR_LOG").is_some() {
        if let Ok(dir) = std::env::var("TEMP") {
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(format!("{dir}\\glaze-bar.log"))
            {
                let _ = writeln!(f, "{msg}");
            }
        }
    }
}

// ---- GlazeWM IPC types ----
#[derive(Deserialize, Clone, Default)]
struct Workspace {
    #[serde(default)]
    name: String,
    #[serde(default, rename = "displayName")]
    display_name: Option<String>,
    #[serde(default, rename = "hasFocus")]
    has_focus: bool,
    #[serde(default, rename = "isDisplayed")]
    is_displayed: bool,
}
#[derive(Deserialize, Default)]
struct Monitor {
    #[serde(default)]
    x: i32,
    #[serde(default)]
    children: Vec<Workspace>,
}
#[derive(Deserialize)]
struct MonData {
    monitors: Vec<Monitor>,
}
#[derive(Deserialize)]
struct MonResp {
    data: Option<MonData>,
}
#[derive(Deserialize)]
struct TdData {
    #[serde(rename = "tilingDirection")]
    tiling_direction: Option<String>,
}
#[derive(Deserialize)]
struct TdResp {
    data: Option<TdData>,
}
#[derive(Deserialize, Default)]
struct BMode {
    #[serde(default)]
    name: String,
    #[serde(default, rename = "displayName")]
    display_name: Option<String>,
}
#[derive(Deserialize)]
struct BmData {
    #[serde(rename = "bindingModes")]
    binding_modes: Vec<BMode>,
}
#[derive(Deserialize)]
struct BmResp {
    data: Option<BmData>,
}

// ---- Shared state, written by worker threads, read by the UI ----
#[derive(Default)]
struct Shared {
    workspaces: Vec<Workspace>,
    tiling: String,
    mode: String,
    cpu: f32,
    mem: f32,
    gpu: String, // "44° 11%" (temp + utilization, from nvidia-smi)
    net: String, // throughput "↓1.2M ↑0.3M"
}

// Fire-and-forget IPC command (e.g. clicking a workspace pill to focus it).
fn ipc_command(cmd: String) {
    std::thread::spawn(move || {
        if let Ok((mut sock, _)) = tungstenite::connect("ws://localhost:6123") {
            let _ = sock.send(tungstenite::Message::Text(cmd.into()));
            let _ = sock.read(); // wait for the ack so it's processed
            let _ = sock.close(None);
        }
    });
}

fn human_rate(bytes_per_sec: f64) -> String {
    if bytes_per_sec >= 1_000_000.0 {
        format!("{:.1}M", bytes_per_sec / 1_000_000.0)
    } else if bytes_per_sec >= 1_000.0 {
        format!("{:.0}K", bytes_per_sec / 1_000.0)
    } else {
        format!("{:.0}B", bytes_per_sec)
    }
}

// Send a query, return the first text response (no subscriptions => next text
// message is the response).
fn query<S: Read + Write>(sock: &mut tungstenite::WebSocket<S>, msg: &str) -> Option<String> {
    sock.send(tungstenite::Message::Text(msg.into())).ok()?;
    loop {
        match sock.read().ok()? {
            tungstenite::Message::Text(t) => return Some(t.to_string()),
            tungstenite::Message::Close(_) => return None,
            _ => continue,
        }
    }
}

fn ipc_thread(shared: Arc<Mutex<Shared>>, my_x: i32, ctx: egui::Context) {
    loop {
        match tungstenite::connect("ws://localhost:6123") {
            Ok((mut sock, _)) => loop {
                // Workspaces for the monitor this bar lives on.
                let Some(txt) = query(&mut sock, "query monitors") else { break };
                if let Ok(r) = serde_json::from_str::<MonResp>(&txt) {
                    if let Some(d) = r.data {
                        if let Some(mon) = d
                            .monitors
                            .into_iter()
                            .min_by_key(|m| (m.x - my_x).abs())
                        {
                            shared.lock().unwrap().workspaces = mon.children;
                        }
                    }
                }
                if let Some(txt) = query(&mut sock, "query tiling-direction") {
                    if let Ok(r) = serde_json::from_str::<TdResp>(&txt) {
                        if let Some(d) = r.data {
                            shared.lock().unwrap().tiling = d.tiling_direction.unwrap_or_default();
                        }
                    }
                }
                if let Some(txt) = query(&mut sock, "query binding-modes") {
                    if let Ok(r) = serde_json::from_str::<BmResp>(&txt) {
                        if let Some(d) = r.data {
                            shared.lock().unwrap().mode = d
                                .binding_modes
                                .first()
                                .map(|m| m.display_name.clone().unwrap_or_else(|| m.name.clone()))
                                .unwrap_or_default();
                        }
                    }
                }
                ctx.request_repaint();
                std::thread::sleep(Duration::from_millis(300));
            },
            Err(_) => std::thread::sleep(Duration::from_secs(2)),
        }
    }
}

fn sys_thread(shared: Arc<Mutex<Shared>>, ctx: egui::Context) {
    let mut sys = sysinfo::System::new();
    let mut nets = sysinfo::Networks::new_with_refreshed_list();
    let mut last = Instant::now();
    loop {
        sys.refresh_cpu_usage();
        std::thread::sleep(Duration::from_millis(500));
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        let cpu = sys.global_cpu_usage();
        let total = sys.total_memory();
        let mem = if total > 0 {
            sys.used_memory() as f32 / total as f32 * 100.0
        } else {
            0.0
        };

        // Network throughput: bytes since the last refresh / elapsed time.
        nets.refresh();
        let now = Instant::now();
        let secs = now.duration_since(last).as_secs_f64().max(0.001);
        last = now;
        let (mut rx, mut tx) = (0u64, 0u64);
        for (_iface, data) in &nets {
            rx += data.received();
            tx += data.transmitted();
        }
        let net = format!(
            "↓{} ↑{}",
            human_rate(rx as f64 / secs),
            human_rate(tx as f64 / secs)
        );

        {
            let mut s = shared.lock().unwrap();
            s.cpu = cpu;
            s.mem = mem;
            s.net = net;
        }
        ctx.request_repaint();
        std::thread::sleep(Duration::from_millis(1500));
    }
}

// GPU temperature + utilization via nvidia-smi (no admin needed).
fn gpu_thread(shared: Arc<Mutex<Shared>>, ctx: egui::Context) {
    loop {
        if let Some(g) = fetch_gpu() {
            shared.lock().unwrap().gpu = g;
            ctx.request_repaint();
        }
        std::thread::sleep(Duration::from_secs(3));
    }
}
fn fetch_gpu() -> Option<String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let out = std::process::Command::new("nvidia-smi")
            .args([
                "--query-gpu=temperature.gpu,utilization.gpu",
                "--format=csv,noheader,nounits",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()?;
        let s = String::from_utf8_lossy(&out.stdout);
        let line = s.lines().next()?;
        let mut parts = line.split(',').map(|x| x.trim());
        let temp = parts.next()?;
        let util = parts.next()?;
        let g = format!("{temp}° {util}%");
        dlog(&format!("gpu = {g}"));
        return Some(g);
    }
    #[allow(unreachable_code)]
    None
}

// Load JetBrainsMono Nerd Font (already installed for the terminal) as the
// primary font so glyphs like the ↓↑ arrows render; egui's bundled fonts stay
// as fallback. No-op if the font isn't found (arrows would then show as tofu).
fn load_font(ctx: &egui::Context) {
    let mut candidates = vec![
        "C:\\Windows\\Fonts\\JetBrainsMonoNerdFont-Regular.ttf".to_string(),
    ];
    if let Ok(d) = std::env::var("LOCALAPPDATA") {
        candidates.insert(
            0,
            format!("{d}\\Microsoft\\Windows\\Fonts\\JetBrainsMonoNerdFont-Regular.ttf"),
        );
    }
    for path in candidates {
        if let Ok(bytes) = std::fs::read(&path) {
            let mut fonts = egui::FontDefinitions::default();
            fonts
                .font_data
                .insert("jbm".to_owned(), egui::FontData::from_owned(bytes));
            for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts.families.entry(fam).or_default().insert(0, "jbm".to_owned());
            }
            ctx.set_fonts(fonts);
            return;
        }
    }
}

struct BarApp {
    shared: Arc<Mutex<Shared>>,
    width: f32,
    sized: bool,
    frame: u32,
}

impl eframe::App for BarApp {
    fn clear_color(&self, _v: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.sized {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(self.width, 34.0)));
            self.sized = true;
        }

        let s = self.shared.lock().unwrap();
        let bar_bg = egui::Color32::from_rgb(22, 22, 30);
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(bar_bg).inner_margin(egui::Margin::symmetric(10.0, 5.0)))
            .show(ctx, |ui| {
                let full = ui.max_rect();

                ui.horizontal_centered(|ui| {
                    // ---- left: workspaces (clickable -> focus that workspace) ----
                    for ws in &s.workspaces {
                        let label = ws
                            .display_name
                            .as_deref()
                            .filter(|t| !t.is_empty())
                            .unwrap_or(&ws.name);
                        let (bg, fg) = if ws.has_focus {
                            (egui::Color32::from_rgb(90, 140, 255), egui::Color32::WHITE)
                        } else if ws.is_displayed {
                            (egui::Color32::from_rgb(45, 45, 58), egui::Color32::from_rgb(220, 220, 230))
                        } else {
                            (egui::Color32::TRANSPARENT, egui::Color32::from_rgb(120, 120, 135))
                        };
                        let resp = egui::Frame::none()
                            .fill(bg)
                            .rounding(5.0)
                            .inner_margin(egui::Margin::symmetric(9.0, 2.0))
                            .show(ui, |ui| {
                                ui.colored_label(fg, label);
                            })
                            .response
                            .interact(egui::Sense::click())
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        if resp.clicked() {
                            ipc_command(format!("command focus --workspace {}", ws.name));
                        }
                        ui.add_space(5.0);
                    }

                    // ---- right: metrics ----
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(4.0);
                        let dim = egui::Color32::from_rgb(180, 180, 195);
                        if !s.gpu.is_empty() {
                            ui.colored_label(egui::Color32::from_rgb(255, 205, 120), format!("GPU {}", s.gpu));
                            ui.add_space(12.0);
                        }
                        let cpu_col = if s.cpu > 85.0 {
                            egui::Color32::from_rgb(255, 120, 120)
                        } else {
                            dim
                        };
                        ui.colored_label(cpu_col, format!("CPU {:>2.0}%", s.cpu));
                        ui.add_space(12.0);
                        ui.colored_label(dim, format!("RAM {:>2.0}%", s.mem));
                        ui.add_space(12.0);
                        if !s.net.is_empty() {
                            ui.colored_label(egui::Color32::from_rgb(130, 200, 150), &s.net);
                            ui.add_space(12.0);
                        }
                        let dir = if s.tiling == "vertical" { "|" } else { "—" };
                        ui.colored_label(egui::Color32::from_rgb(140, 160, 210), dir);
                        if !s.mode.is_empty() {
                            ui.add_space(12.0);
                            egui::Frame::none()
                                .fill(egui::Color32::from_rgb(200, 130, 60))
                                .rounding(5.0)
                                .inner_margin(egui::Margin::symmetric(8.0, 2.0))
                                .show(ui, |ui| {
                                    ui.colored_label(egui::Color32::WHITE, &s.mode);
                                });
                        }
                    });
                });

                // ---- center: date/time (painted centered, independent of the row) ----
                let now = chrono::Local::now().format("%a %d %b  %H:%M").to_string();
                let font = egui::FontId::proportional(14.0);
                let galley = ui.painter().layout_no_wrap(
                    now,
                    font,
                    egui::Color32::from_rgb(210, 210, 222),
                );
                let pos = egui::pos2(
                    full.center().x - galley.size().x / 2.0,
                    full.center().y - galley.size().y / 2.0,
                );
                ui.painter().galley(pos, galley, egui::Color32::WHITE);
            });
        drop(s);

        self.frame = self.frame.wrapping_add(1);
        if self.frame % 15 == 5 {
            trim_ram();
        }
        ctx.request_repaint_after(Duration::from_millis(1000));
    }
}

fn arg_val(flag: &str, default: f32) -> f32 {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn main() -> eframe::Result<()> {
    let x = arg_val("--x", 0.0);
    let width = arg_val("--width", 1920.0);

    let shared = Arc::new(Mutex::new(Shared::default()));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_resizable(false)
            .with_inner_size([width, 34.0])
            .with_position([x, 0.0])
            .with_title("glaze-bar"),
        ..Default::default()
    };
    eframe::run_native(
        "glaze-bar",
        options,
        Box::new(move |cc| {
            load_font(&cc.egui_ctx);
            let ctx = cc.egui_ctx.clone();
            let s1 = shared.clone();
            std::thread::spawn(move || ipc_thread(s1, x as i32, ctx.clone()));
            let s2 = shared.clone();
            let ctx2 = cc.egui_ctx.clone();
            std::thread::spawn(move || sys_thread(s2, ctx2));
            let s3 = shared.clone();
            let ctx3 = cc.egui_ctx.clone();
            std::thread::spawn(move || gpu_thread(s3, ctx3));
            Ok(Box::new(BarApp {
                shared,
                width,
                sized: false,
                frame: 0,
            }))
        }),
    )
}
