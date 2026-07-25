// Lists the master volume and every app currently holding an audio session.
fn main() {
    match rice_common::audio::master_volume() {
        Some(v) => println!("MASTER {:>3}%  muted={}", (v * 100.0).round(), rice_common::audio::master_muted()),
        None => println!("MASTER  (no default output)"),
    }
    let s = rice_common::audio::sessions();
    if s.is_empty() {
        println!("(no app sessions)");
    }
    for x in s {
        println!("  {:>3}%  mute={:<5} pid={:<6} {}", (x.volume * 100.0).round(), x.muted, x.pid, x.name);
    }
}
