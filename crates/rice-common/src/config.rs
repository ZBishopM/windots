//! Paths and small persisted values under `~/.config`.

use std::path::PathBuf;

/// `%USERPROFILE%\.config` -- where every rice runtime file lives.
pub fn config_dir() -> PathBuf {
    PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()).join(".config")
}

/// A file inside `~/.config`.
pub fn config_path(name: &str) -> PathBuf {
    config_dir().join(name)
}

/// Live-adjustable translucency, persisted so the value survives a restart and
/// so WezTerm (which watches its file) can hot-reload it.
pub const OPACITY_MIN: f32 = 0.3;
pub const OPACITY_MAX: f32 = 1.0;

/// Map an opacity to 0..1 for a slider, and back. Kept here so the range is
/// defined once instead of being re-derived at each call site.
pub fn opacity_to_slider(v: f32) -> f32 {
    ((v - OPACITY_MIN) / (OPACITY_MAX - OPACITY_MIN)).clamp(0.0, 1.0)
}
pub fn slider_to_opacity(t: f32) -> f32 {
    OPACITY_MIN + t.clamp(0.0, 1.0) * (OPACITY_MAX - OPACITY_MIN)
}

pub fn read_opacity(name: &str, default: f32) -> f32 {
    std::fs::read_to_string(config_path(name))
        .ok()
        .and_then(|s| s.trim().parse::<f32>().ok())
        .map(|v| v.clamp(OPACITY_MIN, OPACITY_MAX))
        .unwrap_or(default)
}

pub fn write_opacity(name: &str, v: f32) {
    let _ = std::fs::write(config_path(name), format!("{v:.3}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slider_roundtrips() {
        for v in [0.3f32, 0.5, 0.78, 1.0] {
            let back = slider_to_opacity(opacity_to_slider(v));
            assert!((back - v).abs() < 1e-5, "{v} -> {back}");
        }
    }

    #[test]
    fn slider_clamps_out_of_range() {
        assert_eq!(opacity_to_slider(0.0), 0.0);
        assert_eq!(opacity_to_slider(2.0), 1.0);
    }
}
