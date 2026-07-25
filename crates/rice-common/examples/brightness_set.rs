// Set brightness: cargo run --example brightness_set -- <x> <percent>
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (x, pct) = match (a.get(1).and_then(|s| s.parse::<i32>().ok()),
                          a.get(2).and_then(|s| s.parse::<f32>().ok())) {
        (Some(x), Some(p)) => (x, p),
        _ => { eprintln!("usage: brightness_set <monitor-x> <percent>"); return; }
    };
    for d in rice_common::brightness::displays() {
        if d.x == x {
            rice_common::brightness::set(&d, pct / 100.0);
            println!("x={} -> {}%", x, pct);
        }
    }
}
