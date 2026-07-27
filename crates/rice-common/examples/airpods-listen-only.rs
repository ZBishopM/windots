//! Point playback at a Bluetooth headset while leaving the voice path on a
//! wired microphone, so the headset never gets dragged into hands-free mode.
//!
//!   cargo run --release -p rice-common --features bluetooth \
//!     --example airpods-listen-only -- "airpods" "hyperx"
//!
//! First argument matches the device to listen on, second the device to keep the
//! communications role. Both are case-insensitive substrings of the endpoint
//! name.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let listen = args.first().map(|s| s.to_lowercase()).unwrap_or_else(|| "airpods".into());
    let talk = args.get(1).map(|s| s.to_lowercase()).unwrap_or_else(|| "hyperx".into());

    let outs = rice_common::audio::outputs(true);
    let pick = |want: &str| {
        outs.iter()
            .filter(|e| e.name.to_lowercase().contains(want))
            .find(|e| e.active)
            .or_else(|| outs.iter().find(|e| e.name.to_lowercase().contains(want)))
    };

    match pick(&listen) {
        Some(e) => {
            let ok = rice_common::audio::set_default_output_roles(
                &e.id,
                &[rice_common::audio::ROLE_CONSOLE, rice_common::audio::ROLE_MULTIMEDIA],
            );
            println!("listen  -> {} ({})", e.name, if ok { "set" } else { "FAILED" });
        }
        None => println!("listen  -> no endpoint matching {listen:?}"),
    }
    match pick(&talk) {
        Some(e) => {
            let ok = rice_common::audio::set_default_output_roles(
                &e.id,
                &[rice_common::audio::ROLE_COMMUNICATIONS],
            );
            println!("talk    -> {} ({})", e.name, if ok { "set" } else { "FAILED" });
        }
        None => println!("talk    -> no endpoint matching {talk:?}"),
    }

    // Report each role separately: the whole point is that they now differ.
    println!();
    for (label, role) in [
        ("console (sistema)", rice_common::audio::ROLE_CONSOLE),
        ("multimedia (musica)", rice_common::audio::ROLE_MULTIMEDIA),
        ("communications (voz)", rice_common::audio::ROLE_COMMUNICATIONS),
    ] {
        println!("  {label:<22} {:?}", rice_common::audio::output_name_for_role(role));
    }
}
