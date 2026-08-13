//! Target-side client for the bridge WebSocket protocol: connects to a
//! bridge daemon and identifies as `PeerRole::Target`, becoming the
//! device the web UI sees. Reuses `bridge-protocol`'s generated types
//! directly rather than reimplementing protobuf codegen in each target
//! platform's own language (e.g. Kotlin) — this crate is meant to be
//! embedded via a thin per-platform shim (e.g. `renderer-android`'s JNI
//! layer), the same way `renderer-core` is.

pub use bridge_protocol::ShaderUpdate;
use bridge_protocol::{envelope, Envelope, Hello, PeerRole};
use futures_util::{SinkExt, StreamExt};
use prost::Message as _;
use std::fmt;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

type WsStream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Debug)]
pub enum Error {
    Connect(tokio_tungstenite::tungstenite::Error),
    /// The connection closed, or sent something other than a well-formed
    /// `Envelope` binary frame, before/instead of a `HelloAck`.
    HandshakeFailed,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Connect(err) => write!(f, "failed to connect: {err}"),
            Error::HandshakeFailed => {
                write!(
                    f,
                    "connection closed or sent an unexpected response before HelloAck"
                )
            }
        }
    }
}

impl std::error::Error for Error {}

impl From<tokio_tungstenite::tungstenite::Error> for Error {
    fn from(err: tokio_tungstenite::tungstenite::Error) -> Self {
        Error::Connect(err)
    }
}

/// A live connection to a bridge daemon, identified as `PeerRole::Target`.
/// The daemon treats this as "the" currently connected device for as
/// long as the connection stays open.
pub struct TargetClient {
    socket: WsStream,
    session_id: String,
}

impl TargetClient {
    /// Connects to the bridge daemon at `url` (e.g.
    /// `"ws://127.0.0.1:8800/ws"`) and performs the Hello/HelloAck
    /// handshake, identifying as `PeerRole::Target` with `display_name`.
    pub async fn connect(url: &str, display_name: &str) -> Result<Self, Error> {
        let (mut socket, _response) = connect_async(url).await?;

        let hello = Envelope {
            message: Some(envelope::Message::Hello(Hello {
                role: PeerRole::Target as i32,
                display_name: display_name.to_string(),
            })),
        };
        let mut buf = Vec::new();
        hello
            .encode(&mut buf)
            .expect("Envelope encoding is infallible");
        socket.send(Message::Binary(buf)).await?;

        let response = socket
            .next()
            .await
            .ok_or(Error::HandshakeFailed)?
            .map_err(Error::Connect)?;
        let Message::Binary(bytes) = response else {
            return Err(Error::HandshakeFailed);
        };
        let envelope = Envelope::decode(&*bytes).map_err(|_| Error::HandshakeFailed)?;
        let session_id = match envelope.message {
            Some(envelope::Message::HelloAck(ack)) => ack.session_id,
            _ => return Err(Error::HandshakeFailed),
        };

        Ok(Self { socket, session_id })
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Waits for the next `ShaderUpdate` relayed from a UI peer, skipping
    /// anything else (an envelope variant a target peer has no use for,
    /// or a frame that isn't a well-formed `Envelope` at all) rather than
    /// erroring on it — this stays forward-compatible with new envelope
    /// variants without every caller needing its own fallback case for
    /// them. Returns `None` once the connection closes: the daemon
    /// shutting down, a network error, or (not yet possible, since
    /// nothing initiates it today) the daemon deliberately dropping this
    /// target. Doubles as this client's only disconnection signal, same
    /// as the `wait_until_closed` this replaced.
    pub async fn recv(&mut self) -> Option<ShaderUpdate> {
        loop {
            let message = self.socket.next().await?.ok()?;
            let Message::Binary(bytes) = message else {
                continue;
            };
            let Ok(envelope) = Envelope::decode(&*bytes) else {
                continue;
            };
            if let Some(envelope::Message::ShaderUpdate(update)) = envelope.message {
                return Some(update);
            }
        }
    }
}
