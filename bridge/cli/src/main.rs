use std::net::SocketAddr;

const DEFAULT_ADDR: &str = "127.0.0.1:8800";

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let addr: SocketAddr = DEFAULT_ADDR
        .parse()
        .expect("DEFAULT_ADDR must be a valid socket address");

    if let Err(err) = bridge_core::run(addr).await {
        tracing::error!("bridge-core exited with an error: {err}");
        std::process::exit(1);
    }
}
