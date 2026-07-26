//! Prints what the audio and bluetooth modules actually see on this machine.
//!
//!   cargo run --release -p rice-common --features bluetooth --example audio-probe

fn main() {
    println!("== playback endpoints (active only) ==");
    for e in rice_common::audio::outputs(false) {
        println!("  [{}] {}", if e.active { "on " } else { "off" }, e.name);
        println!("       id={}", e.id);
        println!("       container={:?}", e.container);
    }

    println!("\n== playback endpoints (including disconnected) ==");
    for e in rice_common::audio::outputs(true) {
        println!("  [{}] {}", if e.active { "on " } else { "off" }, e.name);
    }

    println!("\n== current default ==");
    println!("  name = {:?}", rice_common::audio::current_output_name());
    println!("  id   = {:?}", rice_common::audio::current_output_id());

    println!("\n== bluetooth audio devices ==");
    let bt = rice_common::bluetooth::devices();
    if bt.is_empty() {
        println!("  (none -- nothing paired, or no Bluetooth audio endpoint exists)");
    }
    for d in &bt {
        println!(
            "  [{}] {}  output_id={:?}",
            if d.connected { "connected" } else { "offline  " },
            d.name,
            d.output_id
        );
    }
}
