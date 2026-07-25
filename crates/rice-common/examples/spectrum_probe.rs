fn main() {
    let s = rice_common::spectrum::Spectrum::start(8);
    println!("capturando 6s (pon algo de audio para ver movimiento)...");
    for i in 0..6 {
        std::thread::sleep(std::time::Duration::from_secs(1));
        let l = s.levels();
        let bars: String = l.iter().map(|v| {
            let n = (v * 8.0).round().clamp(0.0, 8.0) as usize;
            [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'][n]
        }).collect();
        println!("{}s [{}]  active={}", i + 1, bars, s.active());
    }
}
