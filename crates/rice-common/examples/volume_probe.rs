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
        // `pids`, en plural: una aplicacion como un navegador tiene una sesion
        // de audio por proceso hijo. El campo era `pid` y cambio de forma sin
        // que este ejemplo se enterara, lo que dejaba `cargo check --all-targets`
        // -- y por tanto `cargo test` -- roto en todo el workspace.
        let pids = x.pids.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(",");
        println!("  {:>3}%  mute={:<5} pid={:<6} {}", (x.volume * 100.0).round(), x.muted, pids, x.name);
    }
}
