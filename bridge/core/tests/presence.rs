use bridge_protocol::{envelope, Envelope, Hello, PeerRole};
use futures_util::{SinkExt, StreamExt};
use prost::Message as _;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

type WsStream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect_and_say_hello(addr: std::net::SocketAddr, role: PeerRole, name: &str) -> WsStream {
    let (mut ws, _response) = connect_async(format!("ws://{addr}/ws")).await.unwrap();
    let hello = Envelope {
        message: Some(envelope::Message::Hello(Hello {
            role: role as i32,
            display_name: name.to_string(),
        })),
    };
    let mut buf = Vec::new();
    hello.encode(&mut buf).unwrap();
    ws.send(Message::Binary(buf)).await.unwrap();

    // Consume the HelloAck.
    let response = ws.next().await.unwrap().unwrap();
    let Message::Binary(bytes) = response else {
        panic!("expected a binary frame, got {response:?}");
    };
    let envelope = Envelope::decode(&*bytes).unwrap();
    assert!(matches!(
        envelope.message,
        Some(envelope::Message::HelloAck(_))
    ));

    ws
}

async fn recv_presence_update(ws: &mut WsStream) -> Option<String> {
    let response = ws
        .next()
        .await
        .expect("connection closed before a presence update arrived")
        .expect("websocket error");
    let Message::Binary(bytes) = response else {
        panic!("expected a binary frame, got {response:?}");
    };
    let envelope = Envelope::decode(&*bytes).unwrap();
    match envelope.message {
        Some(envelope::Message::PresenceUpdate(update)) => update.target.map(|t| t.display_name),
        other => panic!("expected PresenceUpdate, got {other:?}"),
    }
}

#[tokio::test]
async fn ui_peer_sees_target_connect_and_disconnect() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(bridge_core::serve(listener));

    let mut ui = connect_and_say_hello(addr, PeerRole::Ui, "test-ui").await;

    // No target connected yet.
    assert_eq!(recv_presence_update(&mut ui).await, None);

    // A target connects; the UI peer should be told about it.
    let target = connect_and_say_hello(addr, PeerRole::Target, "pixel-test-device").await;
    assert_eq!(
        recv_presence_update(&mut ui).await,
        Some("pixel-test-device".to_string())
    );

    // The target disconnects; the UI peer should be told it's gone.
    drop(target);
    assert_eq!(recv_presence_update(&mut ui).await, None);
}
