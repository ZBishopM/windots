//! The rice's warm palette, in one place.
//!
//! This used to live in three files with values that had already diverged --
//! the bar's surface was (26,22,19) while the toast's was (32,26,23), and the
//! text/subtext greys differed too. Only the amber accent matched. Colours are
//! plain `[u8; 3]` so non-egui tools (and future non-Rust generators) can use
//! them; `ui::col` converts when egui is in play.

/// Bar background / deepest surface.
pub const BAR_BG: [u8; 3] = [26, 22, 19];
/// Raised surface: the island pill, the toast body.
pub const SURFACE: [u8; 3] = [40, 33, 28];
/// Top highlight edge of a raised surface.
pub const HIGHLIGHT: [u8; 3] = [60, 50, 43];
/// Primary text.
pub const TEXT: [u8; 3] = [233, 224, 214];
/// Secondary / dimmed text.
pub const SUBTEXT: [u8; 3] = [170, 154, 140];
/// Amber accent (`#e0a35c`) -- the default for notifications and the mic action.
pub const ACCENT: [u8; 3] = [224, 163, 92];
/// Lime accent (`#a9b56a`) -- replay saved / success.
pub const ACCENT_OK: [u8; 3] = [169, 181, 106];
/// Warm terracotta -- warnings and failures.
pub const ACCENT_WARN: [u8; 3] = [208, 135, 112];

/// Parse `#rrggbb` (or `rrggbb`). Returns `None` on anything else.
pub fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let h = s.trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    Some([
        u8::from_str_radix(&h[0..2], 16).ok()?,
        u8::from_str_radix(&h[2..4], 16).ok()?,
        u8::from_str_radix(&h[4..6], 16).ok()?,
    ])
}

/// Sunset gradient stops used by cava's spectrum bars (bottom -> top).
pub const GRADIENT: [[u8; 3]; 4] = [[224, 163, 92], [216, 122, 84], [169, 181, 106], [233, 224, 214]];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_hex_forms() {
        assert_eq!(parse_hex("#e0a35c"), Some(ACCENT));
        assert_eq!(parse_hex("e0a35c"), Some(ACCENT));
        assert_eq!(parse_hex("#a9b56a"), Some(ACCENT_OK));
    }

    #[test]
    fn rejects_malformed() {
        assert_eq!(parse_hex("#fff"), None);
        assert_eq!(parse_hex(""), None);
        assert_eq!(parse_hex("#gggggg"), None);
    }
}
