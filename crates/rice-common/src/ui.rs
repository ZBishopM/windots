//! egui bits shared by the bar and the toast: the Nerd Font, the icon table and
//! optically-centred icon drawing.
//!
//! The icon table had drifted: the toast's copy was missing `term` and `rec`, so
//! `shadowplay-notify --icon term` rendered nothing while the bar drew a
//! terminal glyph for the same keyword.

use eframe::egui;

/// A palette entry as an egui colour.
pub const fn col(c: [u8; 3]) -> egui::Color32 {
    egui::Color32::from_rgb(c[0], c[1], c[2])
}

/// A palette entry with explicit alpha.
pub fn col_a(c: [u8; 3], alpha: f32) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(c[0], c[1], c[2], (alpha.clamp(0.0, 1.0) * 255.0) as u8)
}

/// Nerd Font glyph for an icon keyword.
pub fn icon_glyph(name: &str) -> &'static str {
    match name {
        "mic" => "\u{f130}",
        "mic-off" => "\u{f131}",
        "replay" | "save" => "\u{f03d}",
        "check" => "\u{f00c}",
        "rec" => "\u{f111}", // filled circle
        "info" => "\u{f05a}",
        "warn" => "\u{f071}",
        "term" => "\u{f120}",    // fa-terminal
        "desktop" => "\u{f108}", // fa-desktop
        "sliders" => "\u{f1de}",
        _ => "",
    }
}

/// Per-glyph nudge so the glyph's ink *centroid* (visual mass), not just its
/// bounding box, lands on the target centre. Measured with the GLAZEBAR_ICONTEST
/// rig by rendering each glyph 10x and taking the pixel centroid, so these are
/// exact at size 14 and scale with the font size.
pub fn icon_correction(glyph: &str) -> egui::Vec2 {
    match glyph {
        "\u{f03d}" => egui::vec2(0.72, 0.10),  // fa-video-camera (body-heavy left)
        "\u{f120}" => egui::vec2(1.76, -1.76), // fa-terminal (>_ mass low-left)
        "\u{f130}" => egui::vec2(0.00, 0.06),  // fa-microphone (symmetric)
        _ => egui::vec2(0.0, 0.0),
    }
}

/// Draw an icon glyph optically centred at `center`.
///
/// egui's CENTER_CENTER only centres the layout box; icon glyphs sit off-centre
/// inside that box because Nerd Font metrics differ per glyph, so we centre the
/// real painted ink (`mesh_bounds`) and then apply the centroid correction.
pub fn draw_icon(painter: &egui::Painter, center: egui::Pos2, glyph: &str, size: f32, color: egui::Color32) {
    let galley = painter.layout_no_wrap(glyph.to_string(), egui::FontId::proportional(size), color);
    let pos = center - galley.mesh_bounds.center().to_vec2() + icon_correction(glyph) * (size / 14.0);
    painter.galley(pos, galley, color);
}

/// Install JetBrains Mono Nerd Font as the default family, so Nerd Font icons
/// render instead of tofu. No-op if the font isn't installed.
pub fn load_nerd_font(ctx: &egui::Context) {
    let candidates = [
        "C:\\Windows\\Fonts\\JetBrainsMonoNerdFont-Regular.ttf".to_string(),
        format!(
            "{}\\Microsoft\\Windows\\Fonts\\JetBrainsMonoNerdFont-Regular.ttf",
            std::env::var("LOCALAPPDATA").unwrap_or_default()
        ),
    ];
    let Some(bytes) = candidates.iter().find_map(|p| std::fs::read(p).ok()) else {
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts
        .font_data
        .insert("jbm".to_owned(), egui::FontData::from_owned(bytes));
    for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(fam).or_default().insert(0, "jbm".to_owned());
    }
    ctx.set_fonts(fonts);
}

/// Viewport settings common to every rice overlay: undecorated, always on top,
/// no taskbar button, transparent, non-resizable.
pub fn overlay_viewport(title: &str, size: [f32; 2], pos: [f32; 2]) -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_decorations(false)
        .with_always_on_top()
        .with_taskbar(false)
        .with_resizable(false)
        .with_transparent(true)
        .with_inner_size(size)
        .with_position(pos)
        .with_title(title)
}
