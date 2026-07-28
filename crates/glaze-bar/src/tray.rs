//! La bandeja del sistema, pintada en la barra.
//!
//! Aquí no se lee nada de Windows. Quien lee la bandeja es `taskbar`, que ya es
//! el dueño de la ventana de explorer y explica en su propio módulo por qué hace
//! falta UI Automation y por qué la barra de tareas tiene que quedarse
//! *realizada pero invisible*. Esto sólo recoge lo que aquel publica.
//!
//! Ese reparto no es casual. Una llamada de UIA cruza al proceso de explorer y
//! puede tardar decenas de milisegundos o quedarse colgada; hacerla desde el
//! bucle de dibujo de la barra sería pagarla en fotogramas. Aquí lo más caro que
//! ocurre es un `stat` por segundo.

use eframe::egui;

/// Cabecera de `tray.bin`: magia, versión, número de iconos y su lado.
const HEADER: usize = 8 + 4 + 4 + 4;

pub struct Tray {
    /// Cuándo cambió el archivo por última vez. Comparar la fecha evita releer y
    /// resubir texturas cada segundo para nada.
    stamp: Option<std::time::SystemTime>,
    icons: Vec<(String, egui::TextureHandle)>,
    /// Cuándo se miró el archivo por última vez.
    checked: std::time::Instant,
}

impl Tray {
    pub fn new() -> Self {
        Self {
            stamp: None,
            icons: Vec::new(),
            checked: std::time::Instant::now() - std::time::Duration::from_secs(10),
        }
    }

    fn path() -> std::path::PathBuf {
        rice_common::config::config_path("tray.bin")
    }

    /// Recarga si el archivo ha cambiado. Barato: un `stat` como mucho una vez
    /// por segundo, y sólo se toca el disco de verdad cuando la fecha cambia.
    pub fn poll(&mut self, ctx: &egui::Context) {
        if self.checked.elapsed() < std::time::Duration::from_secs(1) {
            return;
        }
        self.checked = std::time::Instant::now();
        let p = Self::path();
        let Ok(stamp) = std::fs::metadata(&p).and_then(|m| m.modified()) else { return };
        if Some(stamp) == self.stamp {
            return;
        }
        self.stamp = Some(stamp);
        let Ok(bytes) = std::fs::read(&p) else { return };
        self.icons = decode(&bytes)
            .into_iter()
            .enumerate()
            .map(|(i, (name, side, rgba))| {
                let img = egui::ColorImage::from_rgba_unmultiplied([side, side], &rgba);
                let tex = ctx.load_texture(format!("tray{i}"), img, egui::TextureOptions::LINEAR);
                (name, tex)
            })
            .collect();
    }

    pub fn is_empty(&self) -> bool {
        self.icons.is_empty()
    }

    /// Píntalos. Devuelve el nombre del que se haya pulsado.
    pub fn ui(&self, ui: &mut egui::Ui, side: f32) -> Option<String> {
        let mut clicked = None;
        // Al revés a propósito. `taskbar` los publica en el orden en que están
        // en la bandeja, de izquierda a derecha, y esto se pinta dentro de un
        // layout de derecha a izquierda: sin invertir aquí, salen en espejo
        // respecto a donde el usuario los tiene memorizados.
        for (name, tex) in self.icons.iter().rev() {
            let (rect, resp) =
                ui.allocate_exact_size(egui::vec2(side + 6.0, side), egui::Sense::click());
            let img = egui::Rect::from_center_size(rect.center(), egui::vec2(side, side));
            ui.painter().image(
                tex.id(),
                img,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            // El nombre ES el tooltip que puso la aplicación, así que suele
            // traer varias líneas; sólo la primera dice de qué programa es.
            let tip = name.lines().next().unwrap_or(name);
            let resp = resp.on_hover_text(tip);
            if resp.clicked() {
                clicked = Some(name.clone());
            }
        }
        clicked
    }
}

/// Pulsa un icono, a través del proceso que sí sabe hablar con la bandeja.
///
/// Un proceso nuevo por clic y no una petición al residente: la llamada de UIA
/// tarda lo que tarde, y la barra tiene que seguir pintando mientras.
pub fn click(name: &str) {
    let exe = rice_common::win::sibling_exe("taskbar.exe");
    let _ = std::process::Command::new(exe).arg("--click").arg(name).spawn();
}

/// (nombre, lado, RGBA) por icono.
fn decode(b: &[u8]) -> Vec<(String, usize, Vec<u8>)> {
    let mut out = Vec::new();
    if b.len() < HEADER || &b[..8] != b"RICETRAY" {
        return out;
    }
    let u32at = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
    if u32at(8) != 1 {
        return out; // versión que no conocemos: mejor nada que basura
    }
    let count = u32at(12);
    let side = u32at(16);
    let px = side * side * 4;
    let mut off = HEADER;
    for _ in 0..count {
        if off + 4 > b.len() {
            break;
        }
        let n = u32at(off);
        off += 4;
        if off + n + px > b.len() {
            break;
        }
        let name = String::from_utf8_lossy(&b[off..off + n]).into_owned();
        off += n;
        out.push((name, side, b[off..off + px].to_vec()));
        off += px;
    }
    out
}
