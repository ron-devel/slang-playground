//! Manual smoke test: connects as a UI peer and sends a `ShaderUpdate`,
//! for exercising the relay-to-target path without a real web frontend
//! yet (that's future work). Not run in CI.
//!
//!   cargo run --example send_shader -p bridge-core -- \
//!     ws://127.0.0.1:8800/ws compute.spv imageMain 16 16 1 0 \
//!     [uniform_buffer_size] [time_offset] [frame_id_offset] [mouse_position_offset]
//!
//! The thread group size (x y z), the descriptor binding the compute
//! shader's output storage image is expected at, and (if the last four,
//! optional arguments are given) its packed uniform buffer's size and
//! the byte offsets within it of the TIME/FRAME_ID/MOUSE_POSITION values
//! — see `ShaderUpdate` in `bridge/protocol/proto/bridge/v1.proto` for
//! why these travel alongside the SPIR-V bytes rather than being assumed
//! constant.

use bridge_protocol::{envelope, Envelope, Hello, PeerRole, ShaderUpdate};
use futures_util::{SinkExt, StreamExt};
use prost::Message as _;
use tokio_tungstenite::tungstenite::Message;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let usage = "usage: send_shader <url> <compute.spv> <entry_point> <tgx> <tgy> <tgz> <output_texture_binding>";
    let url = args
        .next()
        .unwrap_or_else(|| "ws://127.0.0.1:8800/ws".to_string());
    let spirv_path = args.next().expect(usage);
    let entry_point = args.next().expect(usage);
    let thread_group_size_x: u32 = args
        .next()
        .expect(usage)
        .parse()
        .expect("tgx must be a u32");
    let thread_group_size_y: u32 = args
        .next()
        .expect(usage)
        .parse()
        .expect("tgy must be a u32");
    let thread_group_size_z: u32 = args
        .next()
        .expect(usage)
        .parse()
        .expect("tgz must be a u32");
    let output_texture_binding: u32 = args
        .next()
        .expect(usage)
        .parse()
        .expect("binding must be a u32");
    let uniform_buffer_size: u32 = args
        .next()
        .map(|s| s.parse().expect("uniform_buffer_size must be a u32"))
        .unwrap_or(0);
    let time_offset: Option<u32> = args
        .next()
        .map(|s| s.parse().expect("time_offset must be a u32"));
    let frame_id_offset: Option<u32> = args
        .next()
        .map(|s| s.parse().expect("frame_id_offset must be a u32"));
    let mouse_position_offset: Option<u32> = args
        .next()
        .map(|s| s.parse().expect("mouse_position_offset must be a u32"));

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

    let compute_spirv = std::fs::read(&spirv_path).expect("failed to read compute shader");
    println!(
        "Sending shader update ({} bytes, entry point \"{entry_point}\", thread group size {thread_group_size_x}x{thread_group_size_y}x{thread_group_size_z}, output texture binding {output_texture_binding})...",
        compute_spirv.len()
    );

    let update = Envelope {
        message: Some(envelope::Message::ShaderUpdate(ShaderUpdate {
            compute_spirv,
            entry_point,
            thread_group_size_x,
            thread_group_size_y,
            thread_group_size_z,
            output_texture_binding,
            uniform_buffer_size,
            time_offset,
            frame_id_offset,
            mouse_position_offset,
        })),
    };
    let mut buf = Vec::new();
    update.encode(&mut buf).unwrap();
    ws.send(Message::Binary(buf))
        .await
        .expect("failed to send ShaderUpdate");

    println!("Sent.");
}
