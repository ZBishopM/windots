//! One GlazeWM IPC client.
//!
//! There were two independent implementations with incompatible serde models for
//! the same wire format (one used per-field renames, the other `rename_all`, and
//! they disagreed on which monitor fields even existed), plus five copies of the
//! connect/query/command dance across Rust and PowerShell.
//!
//! Two rules encoded here, both learned the hard way:
//!   * connect to `127.0.0.1`, never `localhost` -- localhost resolves `::1`
//!     first, which GlazeWM doesn't listen on, so every connect burned a ~2.1s
//!     IPv6 timeout. That once made a 2000ms health probe report a perfectly
//!     healthy WM as dead and restart it every 60 seconds.
//!   * always set socket timeouts -- an untimed read against a wedged GlazeWM
//!     blocks the calling thread forever.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::Message;

pub const URL: &str = "ws://127.0.0.1:6123";
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(1200);

pub type Socket = tungstenite::WebSocket<MaybeTlsStream<TcpStream>>;

// ---- wire model ----------------------------------------------------------

#[derive(Deserialize)]
pub struct Envelope<T> {
    pub data: Option<T>,
}

#[derive(Deserialize)]
pub struct Monitors {
    pub monitors: Vec<Monitor>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Monitor {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    #[serde(default)]
    pub children: Vec<Workspace>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub name: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub has_focus: bool,
    #[serde(default)]
    pub is_displayed: bool,
}

impl Workspace {
    /// The label to show: `display_name` when set and non-empty, else `name`.
    pub fn label(&self) -> &str {
        self.display_name
            .as_deref()
            .filter(|t| !t.is_empty())
            .unwrap_or(&self.name)
    }
    /// Workspace number, when the name is numeric.
    pub fn index(&self) -> Option<i32> {
        self.name.parse().ok()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TilingDirection {
    pub tiling_direction: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindingModes {
    pub binding_modes: Vec<BindingMode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindingMode {
    pub name: String,
    #[serde(default)]
    pub display_name: Option<String>,
}

impl BindingMode {
    pub fn label(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.name)
    }
}

// ---- client --------------------------------------------------------------

/// A lazily-(re)connected GlazeWM IPC connection.
///
/// Reusing one socket matters on latency-critical paths: a fresh
/// `tungstenite::connect` costs ~30ms of TCP+WS handshake, which was most of the
/// delay before ws-slide could cover the screen for its animation.
#[derive(Default)]
pub struct Client {
    sock: Option<Socket>,
    timeout: Option<Duration>,
}

impl Client {
    pub fn new() -> Self {
        Self { sock: None, timeout: Some(DEFAULT_TIMEOUT) }
    }

    /// Blocking client with no socket timeout, for a thread whose only job is to
    /// wait on subscription events.
    pub fn blocking() -> Self {
        Self { sock: None, timeout: None }
    }

    fn ensure(&mut self) -> Option<&mut Socket> {
        if self.sock.is_none() {
            self.sock = connect(self.timeout);
        }
        self.sock.as_mut()
    }

    /// Drop the connection so the next call reconnects.
    pub fn reset(&mut self) {
        self.sock = None;
    }

    pub fn is_connected(&self) -> bool {
        self.sock.is_some()
    }

    /// Send a message and return the first response that deserializes to `T`.
    pub fn query<T: DeserializeOwned>(&mut self, msg: &str) -> Option<T> {
        let out = self.ensure().and_then(|s| query_on(s, msg));
        if out.is_none() {
            self.reset();
        }
        out
    }

    /// Fire a command and wait for its acknowledgement. Retries once on a fresh
    /// socket, so a stale connection never swallows the command.
    pub fn command(&mut self, cmd: &str) -> bool {
        if self.ensure().map(|s| send_ack(s, cmd)) == Some(true) {
            return true;
        }
        self.reset();
        match self.ensure() {
            Some(s) => send_ack(s, cmd),
            None => false,
        }
    }

    /// Subscribe to an event stream (`sub -e <events>`), leaving the socket open
    /// for `read`.
    pub fn subscribe(&mut self, events: &str) -> bool {
        let msg = format!("sub -e {events}");
        self.ensure().map(|s| s.send(Message::Text(msg)).is_ok()) == Some(true)
    }

    /// Next text frame, or `None` on timeout/close.
    pub fn read(&mut self) -> Option<String> {
        match self.ensure()?.read() {
            Ok(Message::Text(t)) => Some(t),
            Ok(_) => Some(String::new()),
            Err(_) => {
                self.reset();
                None
            }
        }
    }

    /// All monitors with their workspaces.
    pub fn monitors(&mut self) -> Option<Vec<Monitor>> {
        self.query::<Envelope<Monitors>>("query monitors")?.data.map(|d| d.monitors)
    }

    /// Focus a workspace by name/number.
    pub fn focus_workspace(&mut self, name: &str) -> bool {
        self.command(&format!("command focus --workspace {name}"))
    }
}

/// Open a socket, optionally with read/write timeouts.
pub fn connect(timeout: Option<Duration>) -> Option<Socket> {
    let (sock, _) = tungstenite::connect(URL).ok()?;
    if let (Some(t), MaybeTlsStream::Plain(s)) = (timeout, sock.get_ref()) {
        let _ = s.set_read_timeout(Some(t));
        let _ = s.set_write_timeout(Some(t));
    }
    Some(sock)
}

fn query_on<S: Read + Write, T: DeserializeOwned>(sock: &mut tungstenite::WebSocket<S>, msg: &str) -> Option<T> {
    sock.send(Message::Text(msg.to_string())).ok()?;
    // GlazeWM interleaves subscription events with responses, so skip frames
    // until one parses as the expected shape.
    for _ in 0..8 {
        match sock.read() {
            Ok(Message::Text(t)) => {
                if let Ok(v) = serde_json::from_str::<T>(&t) {
                    return Some(v);
                }
            }
            Ok(_) => {}
            Err(_) => return None,
        }
    }
    None
}

fn send_ack<S: Read + Write>(sock: &mut tungstenite::WebSocket<S>, cmd: &str) -> bool {
    sock.send(Message::Text(cmd.to_string())).is_ok() && sock.read().is_ok()
}

/// One-shot command on a throwaway connection, for callers with no client.
pub fn send_once(cmd: &str) {
    if let Some(mut s) = connect(Some(DEFAULT_TIMEOUT)) {
        let _ = send_ack(&mut s, cmd);
    }
}
