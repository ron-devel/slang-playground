mod ws;

use axum::{routing::get, Router};
use bridge_protocol::{Envelope, TargetInfo};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::sync::broadcast;

/// Shared server state: the single currently-connected target (if any), a
/// broadcast channel used to fan `PresenceUpdate`s out to every connected
/// UI peer, one used to relay `ShaderUpdate`s from whichever UI peer sent
/// one to whichever target is currently connected, and one used for the
/// reverse direction — `DeviceInfo`/`PerfSample` from whichever target is
/// connected, fanned out to every UI peer the same way `PresenceUpdate`
/// is (there's no single "the" UI peer to address, unlike `shader_tx`'s
/// single current target).
#[derive(Clone)]
struct AppState {
    current_target: Arc<Mutex<Option<TargetInfo>>>,
    presence_tx: broadcast::Sender<Envelope>,
    shader_tx: broadcast::Sender<Envelope>,
    perf_tx: broadcast::Sender<Envelope>,
}

impl AppState {
    fn new() -> Self {
        let (presence_tx, _receiver) = broadcast::channel(16);
        let (shader_tx, _receiver) = broadcast::channel(16);
        let (perf_tx, _receiver) = broadcast::channel(16);
        Self {
            current_target: Arc::new(Mutex::new(None)),
            presence_tx,
            shader_tx,
            perf_tx,
        }
    }
}

/// Builds the bridge's axum app. Exposed separately from `run` so embedders
/// (tests, or a host process that wants its own listener/lifecycle control)
/// can compose or serve it themselves.
pub fn app() -> Router {
    Router::new()
        .route("/ws", get(ws::handler))
        .with_state(AppState::new())
}

/// Serves the bridge app on an already-bound listener.
pub async fn serve(listener: TcpListener) -> std::io::Result<()> {
    axum::serve(listener, app()).await
}

/// Binds `addr` and serves the bridge app. Convenience entry point for the
/// CLI binary; embedders that need the bound port before it's listening
/// (e.g. port 0) should use `serve` with their own `TcpListener` instead.
pub async fn run(addr: SocketAddr) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    tracing::info!("bridge-core listening on {}", listener.local_addr()?);
    serve(listener).await
}
