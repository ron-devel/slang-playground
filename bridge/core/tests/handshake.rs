use bridge_protocol::{envelope, Envelope, Hello, PeerRole};
use futures_util::{SinkExt, StreamExt};
use prost::Message as _;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn hello_handshake_round_trips_a_session_id() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(bridge_core::serve(listener));

    let (mut ws, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .expect("failed to connect to bridge-core");

    let hello = Envelope {
        message: Some(envelope::Message::Hello(Hello {
            role: PeerRole::Ui as i32,
            display_name: "test-ui".to_string(),
        })),
    };
    let mut buf = Vec::new();
    hello.encode(&mut buf).unwrap();
    ws.send(Message::Binary(buf)).await.unwrap();

    let response = ws
        .next()
        .await
        .expect("connection closed before a response arrived")
        .expect("websocket error");
    let Message::Binary(bytes) = response else {
        panic!("expected a binary frame, got {response:?}");
    };
    let envelope = Envelope::decode(&*bytes).unwrap();

    match envelope.message {
        Some(envelope::Message::HelloAck(ack)) => {
            assert!(!ack.session_id.is_empty(), "session_id should be non-empty");
        }
        other => panic!("expected HelloAck, got {other:?}"),
    }
}
