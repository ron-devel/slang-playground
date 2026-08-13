use bridge_cli::adb_watch;
use std::net::SocketAddr;

const DEFAULT_PORT: u16 = 8800;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let addr = SocketAddr::from(([127, 0, 0, 1], DEFAULT_PORT));

    tokio::spawn(adb_watch::watch_and_tunnel_forever(DEFAULT_PORT));

    if let Err(err) = bridge_core::run(addr).await {
        tracing::error!("bridge-core exited with an error: {err}");
        std::process::exit(1);
    }
}
