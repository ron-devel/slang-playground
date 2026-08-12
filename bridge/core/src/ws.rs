use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use bridge_protocol::{envelope, Envelope, HelloAck};
use prost::Message as _;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

pub async fn handler(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(handle_socket)
}

async fn handle_socket(mut socket: WebSocket) {
    let Some(Ok(Message::Binary(bytes))) = socket.recv().await else {
        tracing::warn!("peer disconnected before sending Hello");
        return;
    };

    let envelope = match Envelope::decode(&*bytes) {
        Ok(envelope) => envelope,
        Err(err) => {
            tracing::warn!("failed to decode Envelope: {err}");
            return;
        }
    };

    let hello = match envelope.message {
        Some(envelope::Message::Hello(hello)) => hello,
        _ => {
            tracing::warn!("first message on a connection must be Hello");
            return;
        }
    };

    let session_id = NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed).to_string();
    tracing::info!(
        role = ?hello.role(),
        name = %hello.display_name,
        session_id = %session_id,
        "peer connected"
    );

    let ack = Envelope {
        message: Some(envelope::Message::HelloAck(HelloAck { session_id })),
    };

    let mut buf = Vec::new();
    if let Err(err) = ack.encode(&mut buf) {
        tracing::error!("failed to encode HelloAck: {err}");
        return;
    }

    let _ = socket.send(Message::Binary(buf)).await;

    // Keep the connection open; further message handling (device registry,
    // presence, shader/uniform relay) lands in later steps.
    while let Some(Ok(_)) = socket.recv().await {}
}
