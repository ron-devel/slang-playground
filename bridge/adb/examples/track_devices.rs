//! Manual smoke test against a real local adb server. Not run in CI — this
//! sandbox has no adb server or device, so it needs to run on a machine
//! with the Android platform-tools installed and a device already visible
//! to `adb devices` (USB, or paired via `adb connect host:port` for
//! wireless). Plug/unplug the device — or run `adb connect`/`adb
//! disconnect` for a wireless one — to watch snapshots stream in live.
//!
//! Usage: cargo run --example track_devices

use bridge_adb::TrackDevicesClient;
use std::time::Duration;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let addr = "127.0.0.1:5037".parse().expect("valid socket address");
    // A bounded connect timeout so this fails fast with a clear error if
    // no adb server is running, rather than hanging indefinitely.
    let mut client = TrackDevicesClient::connect(addr, Some(Duration::from_secs(5))).await?;
    println!("Connected to adb server, watching for device list changes (Ctrl+C to quit)...");

    while let Some(devices) = client.next_snapshot().await? {
        println!("--- device list update ---");
        if devices.is_empty() {
            println!("  (no devices connected)");
        }
        for device in &devices {
            println!("  {}\t{}", device.serial, device.state);
        }
    }

    println!("adb server closed the connection");
    Ok(())
}
