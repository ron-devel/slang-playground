mod ws;

use axum::{routing::get, Router};
use std::net::SocketAddr;
use tokio::net::TcpListener;

/// Builds the bridge's axum app. Exposed separately from `run` so embedders
/// (tests, or a host process that wants its own listener/lifecycle control)
/// can compose or serve it themselves.
pub fn app() -> Router {
    Router::new().route("/ws", get(ws::handler))
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
