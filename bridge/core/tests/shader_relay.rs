use bridge_protocol::{envelope, Envelope, Hello, PeerRole, ShaderUpdate};
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

#[tokio::test]
async fn shader_update_from_ui_is_relayed_to_the_connected_target() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(bridge_core::serve(listener));

    let mut target = connect_and_say_hello(addr, PeerRole::Target, "test-target").await;
    let mut ui = connect_and_say_hello(addr, PeerRole::Ui, "test-ui").await;

    // The UI peer's initial PresenceUpdate (reflecting the target that's
    // already connected) isn't what this test cares about.
    let _ = ui.next().await.unwrap().unwrap();

    let update = Envelope {
        message: Some(envelope::Message::ShaderUpdate(ShaderUpdate {
            compute_spirv: vec![1, 2, 3],
            entry_point: "imageMain".to_string(),
            thread_group_size_x: 16,
            thread_group_size_y: 16,
            thread_group_size_z: 1,
            output_texture_binding: 0,
            uniform_buffer_size: 0,
            time_offset: None,
            frame_id_offset: None,
        })),
    };
    let mut buf = Vec::new();
    update.encode(&mut buf).unwrap();
    ui.send(Message::Binary(buf)).await.unwrap();

    let response = target
        .next()
        .await
        .expect("connection closed before the shader update arrived")
        .expect("websocket error");
    let Message::Binary(bytes) = response else {
        panic!("expected a binary frame, got {response:?}");
    };
    let received = Envelope::decode(&*bytes).unwrap();
    match received.message {
        Some(envelope::Message::ShaderUpdate(update)) => {
            assert_eq!(update.compute_spirv, vec![1, 2, 3]);
            assert_eq!(update.entry_point, "imageMain");
            assert_eq!(update.thread_group_size_x, 16);
            assert_eq!(update.thread_group_size_y, 16);
            assert_eq!(update.thread_group_size_z, 1);
            assert_eq!(update.output_texture_binding, 0);
        }
        other => panic!("expected ShaderUpdate, got {other:?}"),
    }
}
