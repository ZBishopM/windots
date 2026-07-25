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

fn session() -> Option<Session> {
    let mgr = Manager::RequestAsync().ok()?.get().ok()?;
    mgr.GetCurrentSession().ok()
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
