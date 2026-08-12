use bridge_adb::{Device, TrackDevicesClient};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

async fn write_frame(stream: &mut TcpStream, payload: &str) {
    let header = format!("{:04x}", payload.len());
    stream.write_all(header.as_bytes()).await.unwrap();
    stream.write_all(payload.as_bytes()).await.unwrap();
}

/// Speaks just enough of the adb host-server protocol to drive
/// TrackDevicesClient through a realistic sequence of snapshots,
/// including a wireless (IP:port) serial alongside a USB one.
#[tokio::test]
async fn parses_device_list_snapshots_as_they_stream_in() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();

        let mut length_hex = [0u8; 4];
        socket.read_exact(&mut length_hex).await.unwrap();
        let length = u32::from_str_radix(std::str::from_utf8(&length_hex).unwrap(), 16).unwrap();
        let mut service = vec![0u8; length as usize];
        socket.read_exact(&mut service).await.unwrap();
        assert_eq!(service, b"host:track-devices");

        socket.write_all(b"OKAY").await.unwrap();

        write_frame(&mut socket, "emulator-5554\tdevice\n").await;
        write_frame(
            &mut socket,
            "emulator-5554\tdevice\n192.168.1.50:5555\toffline\n",
        )
        .await;
        write_frame(&mut socket, "").await;
        // Connection closes here when `socket` is dropped.
    });

    let mut client = TrackDevicesClient::connect(addr).await.unwrap();

    let first = client.next_snapshot().await.unwrap().unwrap();
    assert_eq!(
        first,
        vec![Device {
            serial: "emulator-5554".into(),
            state: "device".into(),
        }]
    );

    let second = client.next_snapshot().await.unwrap().unwrap();
    assert_eq!(
        second,
        vec![
            Device {
                serial: "emulator-5554".into(),
                state: "device".into(),
            },
            Device {
                serial: "192.168.1.50:5555".into(),
                state: "offline".into(),
            },
        ]
    );

    let third = client.next_snapshot().await.unwrap().unwrap();
    assert_eq!(third, Vec::<Device>::new());

    let fourth = client.next_snapshot().await.unwrap();
    assert_eq!(fourth, None);

    server.await.unwrap();
}

#[tokio::test]
async fn surfaces_fail_responses_as_errors() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut length_hex = [0u8; 4];
        socket.read_exact(&mut length_hex).await.unwrap();
        let length = u32::from_str_radix(std::str::from_utf8(&length_hex).unwrap(), 16).unwrap();
        let mut service = vec![0u8; length as usize];
        socket.read_exact(&mut service).await.unwrap();

        socket.write_all(b"FAIL").await.unwrap();
        write_frame(&mut socket, "no such server").await;
    });

    let result = TrackDevicesClient::connect(addr).await;
    assert!(result.is_err());

    server.await.unwrap();
}
