use crate::AppState;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use bridge_protocol::{envelope, Envelope, HelloAck, PeerRole, PresenceUpdate, TargetInfo};
use prost::Message as _;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

pub async fn handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
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
    let role = hello.role();
    tracing::info!(
        ?role,
        name = %hello.display_name,
        session_id = %session_id,
        "peer connected"
    );

    if !send_envelope(&mut socket, hello_ack(session_id.clone())).await {
        return;
    }

    match role {
        PeerRole::Ui => run_ui_peer(socket, state).await,
        PeerRole::Target => run_target_peer(socket, state, session_id, hello.display_name).await,
        PeerRole::Unspecified => {
            tracing::warn!("Hello.role must be UI or TARGET, got UNSPECIFIED");
        }
    }
}

fn hello_ack(session_id: String) -> Envelope {
    Envelope {
        message: Some(envelope::Message::HelloAck(HelloAck { session_id })),
    }
}

fn presence_update(target: Option<TargetInfo>) -> Envelope {
    Envelope {
        message: Some(envelope::Message::PresenceUpdate(PresenceUpdate { target })),
    }
}

async fn send_envelope(socket: &mut WebSocket, envelope: Envelope) -> bool {
    let mut buf = Vec::new();
    if let Err(err) = envelope.encode(&mut buf) {
        tracing::error!("failed to encode envelope: {err}");
        return false;
    }
    socket.send(Message::Binary(buf)).await.is_ok()
}

/// A UI peer receives the current presence state immediately on connecting,
/// then every subsequent update as targets connect/disconnect, until it
/// closes its end of the connection. Any `ShaderUpdate` it sends is relayed
/// to whichever target is currently connected (see `run_target_peer`);
/// anything else it sends (an unparseable frame, some other envelope
/// variant) is ignored rather than treated as a protocol error, so this
/// stays forward-compatible without every new message type needing a
/// connection-closing fallback case here.
async fn run_ui_peer(mut socket: WebSocket, state: AppState) {
    // Subscribe before reading current state: a target that connects
    // between the two can only produce a harmless duplicate update, never
    // a missed one.
    let mut presence_rx = state.presence_tx.subscribe();

    let current = state.current_target.lock().unwrap().clone();
    if !send_envelope(&mut socket, presence_update(current)).await {
        return;
    }

    loop {
        tokio::select! {
            update = presence_rx.recv() => {
                let Ok(envelope) = update else { return };
                if !send_envelope(&mut socket, envelope).await {
                    return;
                }
            }
            msg = socket.recv() => {
                let Some(Ok(message)) = msg else { return };
                if let Message::Binary(bytes) = &message {
                    if let Ok(envelope) = Envelope::decode(&**bytes) {
                        if matches!(envelope.message, Some(envelope::Message::ShaderUpdate(_))) {
                            let _ = state.shader_tx.send(envelope);
                        }
                    }
                }
            }
        }
    }
}

/// A target peer becomes the registry's current target for as long as its
/// connection stays open, broadcasting a presence update on both arrival
/// and departure, and receiving any `ShaderUpdate` a UI peer sends in the
/// meantime (see `run_ui_peer`).
async fn run_target_peer(
    mut socket: WebSocket,
    state: AppState,
    session_id: String,
    display_name: String,
) {
    let info = TargetInfo {
        session_id: session_id.clone(),
        display_name,
    };
    *state.current_target.lock().unwrap() = Some(info.clone());
    let _ = state.presence_tx.send(presence_update(Some(info)));

    let mut shader_rx = state.shader_tx.subscribe();

    loop {
        tokio::select! {
            update = shader_rx.recv() => {
                let Ok(envelope) = update else { break };
                if !send_envelope(&mut socket, envelope).await {
                    break;
                }
            }
            msg = socket.recv() => {
                if !matches!(msg, Some(Ok(_))) {
                    break;
                }
            }
        }
    }

    let was_current = {
        let mut current = state.current_target.lock().unwrap();
        if current.as_ref().is_some_and(|t| t.session_id == session_id) {
            *current = None;
            true
        } else {
            false
        }
    };
    if was_current {
        let _ = state.presence_tx.send(presence_update(None));
    }
}
