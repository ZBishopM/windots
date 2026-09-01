//! Shared building blocks for the rice's Rust tools.
//!
//! Each module was previously copy-pasted across two or more binaries, and
//! several copies had already drifted apart: the warm palette existed in three
//! versions with different greys, and the icon table in the toast was missing
//! the `term` and `rec` glyphs the bar had, so `--icon term` silently rendered
//! nothing. Everything here is feature-gated -- see Cargo.toml.

pub mod args;
pub mod config;
pub mod theme;
pub mod win;

#[cfg(feature = "ui")]
pub mod ui;

#[cfg(feature = "ipc")]
pub mod ipc;

#[cfg(feature = "event")]
pub mod event;

#[cfg(feature = "settings")]
pub mod settings;

pub mod brightness;

#[cfg(feature = "audio")]
pub mod audio;

#[cfg(feature = "bluetooth")]
pub mod bluetooth;

#[cfg(feature = "battery")]
pub mod battery;

#[cfg(feature = "media")]
pub mod media;

#[cfg(feature = "spectrum")]
pub mod spectrum;

/// Anillo de audio en memoria compartida: un capturador, varios lectores.
/// Sin dependencias -- Win32 a pelo, como `win` -- asi que va sin feature.
#[cfg(windows)]
pub mod audioshare;
