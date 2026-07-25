use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager as Manager;

fn main() {
    // Distinguish "WinRT unreachable" from "nothing is playing".
    match Manager::RequestAsync().and_then(|op| op.get()) {
        Ok(mgr) => {
            println!("SMTC manager: OK");
            match mgr.GetCurrentSession() {
                Ok(s) => println!("current session app: {:?}", s.SourceAppUserModelId().map(|h| h.to_string())),
                Err(_) => println!("current session: none"),
            }
        }
        Err(e) => println!("SMTC manager FAILED: {e}"),
    }
    match rice_common::media::now_playing() {
        Some(n) => println!("now: playing={} '{}' - '{}'", n.playing, n.title, n.artist),
        None => println!("now: nothing"),
    }
}
