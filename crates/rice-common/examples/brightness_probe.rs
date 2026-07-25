// Read-only probe: current brightness of every DDC/CI display.
fn main() {
    let ds = rice_common::brightness::displays();
    if ds.is_empty() {
        println!("no DDC/CI-capable displays found");
        return;
    }
    for d in &ds {
        println!("x={:<6} {}%", d.x, (d.fraction() * 100.0).round());
    }
}
