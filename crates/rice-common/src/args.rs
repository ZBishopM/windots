//! Tiny `--flag value` lookup.
//!
//! The two hand-rolled copies this replaces each re-collected the whole of
//! `std::env::args()` on every lookup; the toast did that seven times per run.
//! Here argv is collected once.

use std::sync::OnceLock;

fn argv() -> &'static [String] {
    static ARGV: OnceLock<Vec<String>> = OnceLock::new();
    ARGV.get_or_init(|| std::env::args().collect())
}

/// Value following `flag`, if present.
pub fn flag(name: &str) -> Option<&'static str> {
    let a = argv();
    let i = a.iter().position(|x| x == name)?;
    a.get(i + 1).map(|s| s.as_str())
}

/// Value following `flag`, parsed, or `default`.
pub fn flag_or<T: std::str::FromStr>(name: &str, default: T) -> T {
    flag(name).and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// Whether a bare `flag` is present (no value).
pub fn has_flag(name: &str) -> bool {
    argv().iter().any(|x| x == name)
}
