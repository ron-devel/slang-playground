//! A minimal client for the local adb server's `host:track-devices`
//! service: a long-lived connection that pushes a full device-list
//! snapshot every time the set of connected devices changes, so callers
//! never need to poll `adb devices`.

use std::io;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

mod cli;
pub mod ffi;

pub use cli::AdbCli;

/// One entry in an adb `track-devices` snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub serial: String,
    /// Raw adb device state (e.g. "device", "offline", "unauthorized",
    /// "no permissions"). Only "device" means it's actually usable; the
    /// rest are left unparsed here since interpreting them is a policy
    /// decision for the caller, not this client.
    pub state: String,
}

/// A connection to the local adb server's `host:track-devices` service.
#[derive(Debug)]
pub struct TrackDevicesClient {
    stream: TcpStream,
}

impl TrackDevicesClient {
    /// Connects to an adb server at `addr` (typically `127.0.0.1:5037`)
    /// and starts tracking devices. `timeout` bounds how long this waits
    /// for the TCP connection and initial handshake; `None` waits
    /// indefinitely. Returns an `io::ErrorKind::TimedOut` error if it
    /// expires.
    pub async fn connect(addr: SocketAddr, timeout: Option<Duration>) -> io::Result<Self> {
        let connect = Self::connect_inner(addr);
        match timeout {
            Some(timeout) => {
                tokio::time::timeout(timeout, connect)
                    .await
                    .unwrap_or_else(|_elapsed| {
                        Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "timed out connecting to adb server",
                        ))
                    })
            }
            None => connect.await,
        }
    }

    async fn connect_inner(addr: SocketAddr) -> io::Result<Self> {
        let mut stream = TcpStream::connect(addr).await?;
        send_request(&mut stream, "host:track-devices").await?;
        read_okay(&mut stream).await?;
        tracing::debug!(%addr, "connected to adb host:track-devices");
        Ok(Self { stream })
    }

    /// Waits for the next device-list snapshot. Returns `Ok(None)` once
    /// the adb server closes the connection.
    pub async fn next_snapshot(&mut self) -> io::Result<Option<Vec<Device>>> {
        let Some(payload) = read_length_prefixed(&mut self.stream).await? else {
            return Ok(None);
        };
        let devices = parse_device_list(&payload);
        tracing::debug!(count = devices.len(), "adb device list snapshot");
        Ok(Some(devices))
    }
}

async fn send_request(stream: &mut TcpStream, service: &str) -> io::Result<()> {
    let header = format!("{:04x}", service.len());
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(service.as_bytes()).await?;
    Ok(())
}

async fn read_okay(stream: &mut TcpStream) -> io::Result<()> {
    let mut status = [0u8; 4];
    stream.read_exact(&mut status).await?;
    match &status {
        b"OKAY" => Ok(()),
        b"FAIL" => {
            let message = read_length_prefixed(stream).await?.unwrap_or_default();
            Err(io::Error::other(format!(
                "adb server returned FAIL: {}",
                String::from_utf8_lossy(&message)
            )))
        }
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected adb server status: {other:?}"),
        )),
    }
}

/// Reads a `<4 hex-digit length><payload>` frame, adb's basic wire framing
/// for host-server requests/responses. Returns `Ok(None)` if the
/// connection closed cleanly before any bytes of a new frame arrived.
async fn read_length_prefixed(stream: &mut TcpStream) -> io::Result<Option<Vec<u8>>> {
    let mut length_hex = [0u8; 4];
    match stream.read_exact(&mut length_hex).await {
        Ok(_) => {}
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err),
    }
    let length_str = std::str::from_utf8(&length_hex)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "length prefix was not ASCII"))?;
    let length = u32::from_str_radix(length_str, 16)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "length prefix was not hex"))?;
    let mut payload = vec![0u8; length as usize];
    stream.read_exact(&mut payload).await?;
    Ok(Some(payload))
}

fn parse_device_list(payload: &[u8]) -> Vec<Device> {
    String::from_utf8_lossy(payload)
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(2, '\t');
            let serial = parts.next()?.trim();
            let state = parts.next()?.trim();
            if serial.is_empty() {
                return None;
            }
            Some(Device {
                serial: serial.to_string(),
                state: state.to_string(),
            })
        })
        .collect()
}
