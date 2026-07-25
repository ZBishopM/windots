//! Volume control: master, per application, and the default output device.
//!
//! Replaces what EarTrumpet was doing from the system tray -- which matters now
//! that the tray is hidden.
//!
//!   appvol                      list master + every app holding audio
//!   appvol 40                   set master to 40%
//!   appvol discord 20           set Discord to 20% (all of its processes)
//!   appvol discord mute|unmute
//!   appvol --mute | --unmute    master

use rice_common::audio;

fn pct(v: f32) -> String {
    format!("{}%", (v * 100.0).round() as i32)
}

fn list() {
    match audio::master_volume() {
        Some(v) => println!(
            "{} master{}",
            pct(v),
            if audio::master_muted() { "  (muted)" } else { "" }
        ),
        None => println!("  (no default output)"),
    }
    for s in audio::sessions() {
        let label = s.name.trim_end_matches(".exe");
        println!(
            "{:>5} {}{}",
            pct(s.volume),
            label,
            if s.muted { "  (muted)" } else { "" }
        );
    }
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    match a.len() {
        0 => list(),
        1 => match a[0].as_str() {
            "--mute" => {
                audio::set_master_mute(true);
                list();
            }
            "--unmute" => {
                audio::set_master_mute(false);
                list();
            }
            // A bare number sets the master volume.
            n => match n.trim_end_matches('%').parse::<f32>() {
                Ok(p) => {
                    audio::set_master_volume(p / 100.0);
                    list();
                }
                Err(_) => eprintln!("usage: appvol [<percent> | <app> <percent|mute|unmute>]"),
            },
        },
        _ => {
            let app = &a[0];
            match a[1].as_str() {
                "mute" => {
                    if !audio::set_app_mute(app, true) {
                        eprintln!("no audio session for '{app}'");
                    }
                }
                "unmute" => {
                    if !audio::set_app_mute(app, false) {
                        eprintln!("no audio session for '{app}'");
                    }
                }
                p => match p.trim_end_matches('%').parse::<f32>() {
                    Ok(v) => {
                        if !audio::set_app_volume(app, v / 100.0) {
                            eprintln!("no audio session for '{app}'");
                        }
                    }
                    Err(_) => eprintln!("expected a percentage, 'mute' or 'unmute'"),
                },
            }
            list();
        }
    }
}
