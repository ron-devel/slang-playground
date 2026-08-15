use bridge_protocol::{envelope, DeviceInfo, Envelope, Hello, PeerRole, PerfSample};
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

async fn send_envelope(ws: &mut WsStream, envelope: Envelope) {
    let mut buf = Vec::new();
    envelope.encode(&mut buf).unwrap();
    ws.send(Message::Binary(buf)).await.unwrap();
}

async fn recv_envelope(ws: &mut WsStream) -> Envelope {
    let response = ws
        .next()
        .await
        .expect("connection closed before an envelope arrived")
        .expect("websocket error");
    let Message::Binary(bytes) = response else {
        panic!("expected a binary frame, got {response:?}");
    };
    Envelope::decode(&*bytes).unwrap()
}

#[tokio::test]
async fn device_info_and_perf_sample_from_a_target_are_relayed_to_every_ui_peer() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(bridge_core::serve(listener));

    let mut target = connect_and_say_hello(addr, PeerRole::Target, "test-target").await;
    let mut ui_a = connect_and_say_hello(addr, PeerRole::Ui, "ui-a").await;
    let mut ui_b = connect_and_say_hello(addr, PeerRole::Ui, "ui-b").await;

    // Each UI peer's initial PresenceUpdate isn't what this test cares
    // about.
    let _ = recv_envelope(&mut ui_a).await;
    let _ = recv_envelope(&mut ui_b).await;

    send_envelope(
        &mut target,
        Envelope {
            message: Some(envelope::Message::DeviceInfo(DeviceInfo {
                gpu_name: "Adreno (TM) 740".to_string(),
                driver_version: 0x0080_0400,
                vendor_id: 0x5143,
                device_id: 0x1,
                api_version: 0x0040_1000,
                android_model: "Pixel 8".to_string(),
                android_manufacturer: "Google".to_string(),
                android_release: "15".to_string(),
                android_sdk_int: 35,
                android_fingerprint: "google/shiba/shiba:15/...".to_string(),
            })),
        },
    )
    .await;

    for ui in [&mut ui_a, &mut ui_b] {
        match recv_envelope(ui).await.message {
            Some(envelope::Message::DeviceInfo(info)) => {
                assert_eq!(info.gpu_name, "Adreno (TM) 740");
                assert_eq!(info.android_model, "Pixel 8");
            }
            other => panic!("expected DeviceInfo, got {other:?}"),
        }
    }

    send_envelope(
        &mut target,
        Envelope {
            message: Some(envelope::Message::PerfSample(PerfSample {
                frame_id: 42,
                gpu_time_ms: 4.2,
            })),
        },
    )
    .await;

    for ui in [&mut ui_a, &mut ui_b] {
        match recv_envelope(ui).await.message {
            Some(envelope::Message::PerfSample(sample)) => {
                assert_eq!(sample.frame_id, 42);
                assert!((sample.gpu_time_ms - 4.2).abs() < f32::EPSILON);
            }
            other => panic!("expected PerfSample, got {other:?}"),
        }
    }
}
