//! Manual smoke test: connects as a UI peer and sends a `ShaderUpdate`,
//! for exercising the relay-to-target path without a real web frontend
//! yet (that's future work). Not run in CI.
//!
//!   cargo run --example send_shader -p bridge-core -- \
//!     ws://127.0.0.1:8800/ws vertex.spv fragment.spv

use bridge_protocol::{envelope, Envelope, Hello, PeerRole, ShaderUpdate};
use futures_util::{SinkExt, StreamExt};
use prost::Message as _;
use tokio_tungstenite::tungstenite::Message;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let url = args
        .next()
        .unwrap_or_else(|| "ws://127.0.0.1:8800/ws".to_string());
    let vertex_path = args
        .next()
        .expect("usage: send_shader <url> <vertex.spv> <fragment.spv>");
    let fragment_path = args
        .next()
        .expect("usage: send_shader <url> <vertex.spv> <fragment.spv>");

    println!("Connecting to {url} as a UI peer...");
    let (mut ws, _response) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("failed to connect");

    let hello = Envelope {
        message: Some(envelope::Message::Hello(Hello {
            role: PeerRole::Ui as i32,
            display_name: "send_shader example".to_string(),
        })),
    };
    let mut buf = Vec::new();
    hello.encode(&mut buf).unwrap();
    ws.send(Message::Binary(buf))
        .await
        .expect("failed to send Hello");
    let _ = ws.next().await.expect("connection closed before HelloAck");

    let vertex_spirv = std::fs::read(&vertex_path).expect("failed to read vertex shader");
    let fragment_spirv = std::fs::read(&fragment_path).expect("failed to read fragment shader");
    println!(
        "Sending shader update ({} bytes vertex, {} bytes fragment)...",
        vertex_spirv.len(),
        fragment_spirv.len()
    );

    let update = Envelope {
        message: Some(envelope::Message::ShaderUpdate(ShaderUpdate {
            vertex_spirv,
            fragment_spirv,
        })),
    };
    let mut buf = Vec::new();
    update.encode(&mut buf).unwrap();
    ws.send(Message::Binary(buf))
        .await
        .expect("failed to send ShaderUpdate");

    println!("Sent.");
}
