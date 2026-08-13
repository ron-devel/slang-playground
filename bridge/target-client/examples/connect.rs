//! Manual smoke test against a real running bridge daemon. Not run in
//! CI — start `slang-bridge` first (see `bridge/cli`), then:
//!   cargo run --example connect -p bridge-target-client -- ws://127.0.0.1:8800/ws

use bridge_target_client::TargetClient;

#[tokio::main]
async fn main() {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "ws://127.0.0.1:8800/ws".to_string());

    println!("Connecting to {url} as a target peer...");
    let mut client = TargetClient::connect(&url, "target-client example")
        .await
        .expect("failed to connect");
    println!("Connected. session_id = {}", client.session_id());
    println!("Waiting for shader updates (Ctrl+C to quit)...");

    while let Some(update) = client.recv().await {
        println!(
            "Received shader update: {} bytes vertex, {} bytes fragment",
            update.vertex_spirv.len(),
            update.fragment_spirv.len()
        );
    }
    println!("Connection closed.");
}
