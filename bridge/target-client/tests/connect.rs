//! Runs against a real bridge-core server (via bridge-core::serve, a dev
//! dependency), not a fake — bridge-core is trivial to spin up on a real
//! port and gives a genuine black-box integration test of the handshake.

use bridge_protocol::{envelope, Envelope, Hello, HelloAck, PeerRole, ShaderUpdate};
use bridge_target_client::TargetClient;
use futures_util::{SinkExt, StreamExt};
use prost::Message as _;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn connects_and_completes_the_handshake() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(bridge_core::serve(listener));

    let client = TargetClient::connect(&format!("ws://{addr}/ws"), "test-target")
        .await
        .expect("handshake should succeed");

    assert!(
        !client.session_id().is_empty(),
        "expected a non-empty session_id from HelloAck"
    );
}

/// Uses a raw fake WS peer rather than `bridge_core::serve` here: axum
/// spawns each accepted connection as its own independent tokio task, so
/// aborting the task running the accept loop (simulating "the server
/// process went away") doesn't actually close already-established
/// connections — this needs a peer that genuinely closes the socket to
/// test disconnection detection deterministically.
#[tokio::test]
async fn detects_disconnection_when_the_peer_closes_the_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();

        // Consume the Hello, reply HelloAck, then drop the connection —
        // simulating the daemon going away after a successful handshake.
        let _ = ws.next().await;
        let ack = Envelope {
            message: Some(envelope::Message::HelloAck(HelloAck {
                session_id: "test-session".to_string(),
            })),
        };
        let mut buf = Vec::new();
        ack.encode(&mut buf).unwrap();
        ws.send(Message::Binary(buf)).await.unwrap();
    });

    let mut client = TargetClient::connect(&format!("ws://{addr}/ws"), "test-target")
        .await
        .expect("handshake should succeed");

    let update = tokio::time::timeout(Duration::from_secs(2), client.recv())
        .await
        .expect("recv should return once the peer closes the connection");
    assert!(
        update.is_none(),
        "expected recv to return None once the connection closed"
    );
}

/// Full-stack: a real `bridge-core` server relaying a real `ShaderUpdate`
/// from a fake UI peer, decoded by `TargetClient::recv` on the other end
/// — proves this crate's decoding actually matches what the daemon sends,
/// not just that it can decode something it encoded itself.
#[tokio::test]
async fn receives_a_shader_update_relayed_from_a_ui_peer() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(bridge_core::serve(listener));

    let mut client = TargetClient::connect(&format!("ws://{addr}/ws"), "test-target")
        .await
        .expect("handshake should succeed");

    let (mut ui, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let hello = Envelope {
        message: Some(envelope::Message::Hello(Hello {
            role: PeerRole::Ui as i32,
            display_name: "test-ui".to_string(),
        })),
    };
    let mut buf = Vec::new();
    hello.encode(&mut buf).unwrap();
    ui.send(Message::Binary(buf)).await.unwrap();
    let _ = ui.next().await; // HelloAck
    let _ = ui.next().await; // initial PresenceUpdate (test-target is already connected)

    let update = Envelope {
        message: Some(envelope::Message::ShaderUpdate(ShaderUpdate {
            compute_spirv: vec![9, 9],
            entry_point: "imageMain".to_string(),
            thread_group_size_x: 16,
            thread_group_size_y: 16,
            thread_group_size_z: 1,
            output_texture_binding: 0,
        })),
    };
    let mut buf = Vec::new();
    update.encode(&mut buf).unwrap();
    ui.send(Message::Binary(buf)).await.unwrap();

    let received = tokio::time::timeout(Duration::from_secs(2), client.recv())
        .await
        .expect("recv should return once the shader update arrives")
        .expect("expected Some(update), got None (connection closed)");
    assert_eq!(received.compute_spirv, vec![9, 9]);
    assert_eq!(received.entry_point, "imageMain");
}

#[tokio::test]
async fn fails_to_connect_when_nothing_is_listening() {
    // A port nothing is bound to, rather than picking an arbitrary
    // constant, to avoid ever colliding with something else running on
    // the test machine.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let result = TargetClient::connect(&format!("ws://{addr}/ws"), "test-target").await;
    assert!(
        result.is_err(),
        "expected connecting to a closed port to fail"
    );
}
