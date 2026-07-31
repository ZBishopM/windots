//! What is currently playing, and control over it.
//!
//! Uses GlobalSystemMediaTransportControlsSession -- the same source Windows'
//! own media flyout reads, so anything that publishes SMTC metadata (Spotify,
//! YouTube in a browser, VLC, a video call) shows up without per-app work.
//!
//! Every call here is blocking: the WinRT APIs are async, and `.get()` waits.
//! They take tens of milliseconds, so callers should run them off any render
//! thread.

use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Storage::Streams::{DataReader, IRandomAccessStreamReference};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    /// Source app, e.g. `Spotify.exe` or the browser.
    pub app: String,
    pub playing: bool,
    /// Whether the session says these controls are available right now.
    pub can_next: bool,
    pub can_prev: bool,
}

/// The session manager, built once per thread and kept.
///
/// `Manager::RequestAsync()` is not a cheap accessor: it crosses to NPSMSvc (the
/// Now Playing Session Manager service) and waits. Calling it on every poll made
/// that service the busiest thing on an idle desktop -- 427 s of CPU in 2h20m
/// measured, with two bars asking every 1.5 s each. The manager stays valid for
/// the life of the process and raises its own change events, so rebuilding it was
/// pure waste.
///
/// Thread-local, not a global: the WinRT object is not `Sync` and every caller
/// here already runs on its own polling thread.
fn manager() -> Option<Manager> {
    thread_local! {
        static MGR: std::cell::RefCell<Option<Manager>> = const { std::cell::RefCell::new(None) };
    }
    MGR.with(|m| {
        let mut m = m.borrow_mut();
        if m.is_none() {
            *m = Manager::RequestAsync().ok()?.get().ok();
        }
        m.clone()
    })
}

fn session() -> Option<Session> {
    manager()?.GetCurrentSession().ok()
}

/// The current session's metadata, or None when nothing is playing or paused.
pub fn now_playing() -> Option<NowPlaying> {
    let s = session()?;
    let props = s.TryGetMediaPropertiesAsync().ok()?.get().ok()?;
    let info = s.GetPlaybackInfo().ok()?;
    let status = info.PlaybackStatus().ok()?;
    let controls = info.Controls().ok();
    Some(NowPlaying {
        title: props.Title().map(|h| h.to_string()).unwrap_or_default(),
        artist: props.Artist().map(|h| h.to_string()).unwrap_or_default(),
        app: s
            .SourceAppUserModelId()
            .map(|h| h.to_string())
            .unwrap_or_default(),
        playing: status == Status::Playing,
        can_next: controls
            .as_ref()
            .and_then(|c| c.IsNextEnabled().ok())
            .unwrap_or(false),
        can_prev: controls
            .as_ref()
            .and_then(|c| c.IsPreviousEnabled().ok())
            .unwrap_or(false),
    })
}

/// Raw bytes of the current track's cover art (usually PNG or JPEG).
///
/// Separate from `now_playing` because it is the expensive part: the thumbnail
/// is fetched and copied over a stream, so it should only be re-read when the
/// track actually changes.
pub fn cover_bytes() -> Option<Vec<u8>> {
    let s = session()?;
    let props = s.TryGetMediaPropertiesAsync().ok()?.get().ok()?;
    let thumb: IRandomAccessStreamReference = props.Thumbnail().ok()?;
    let stream = thumb.OpenReadAsync().ok()?.get().ok()?;
    let size = stream.Size().ok()? as u32;
    if size == 0 || size > 8 * 1024 * 1024 {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    reader.LoadAsync(size).ok()?.get().ok()?;
    let mut buf = vec![0u8; size as usize];
    reader.ReadBytes(&mut buf).ok()?;
    Some(buf)
}

fn with_session<F: Fn(&Session) -> bool>(f: F) -> bool {
    match session() {
        Some(s) => f(&s),
        None => false,
    }
}

pub fn toggle_play() -> bool {
    with_session(|s| {
        s.TryTogglePlayPauseAsync()
            .ok()
            .and_then(|op| op.get().ok())
            .unwrap_or(false)
    })
}

pub fn next() -> bool {
    with_session(|s| {
        s.TrySkipNextAsync()
            .ok()
            .and_then(|op| op.get().ok())
            .unwrap_or(false)
    })
}

pub fn previous() -> bool {
    with_session(|s| {
        s.TrySkipPreviousAsync()
            .ok()
            .and_then(|op| op.get().ok())
            .unwrap_or(false)
    })
}
