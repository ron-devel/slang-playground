//! Keeps an adb reverse tunnel set up for every usable connected device,
//! so the bridge app on each device can always reach this daemon
//! regardless of USB replugs or wireless reconnects.

use bridge_adb::{AdbCli, TrackDevicesClient};
use std::collections::HashSet;
use std::net::SocketAddr;
use std::time::Duration;

const REAL_ADB_SERVER_ADDR: &str = "127.0.0.1:5037";
const USABLE_STATE: &str = "device";
const RETRY_DELAY: Duration = Duration::from_secs(5);

/// Watches `adb_server_addr`'s device list and keeps a reverse tunnel
/// (the device's own `localhost:<local_port>` to this host's
/// `localhost:<local_port>`) set up for every device in a usable
/// (`"device"`) state. Runs until the adb server connection closes or
/// errors.
///
/// A device's reverse tunnel doesn't survive it actually disconnecting —
/// it goes away with the adb transport — so there's no explicit "remove"
/// step here; a device dropping out of the usable set just means we stop
/// treating it as tunneled, so `reverse` runs again if/when it reappears.
pub async fn watch_and_tunnel(
    adb_server_addr: SocketAddr,
    adb: &AdbCli,
    local_port: u16,
) -> std::io::Result<()> {
    let mut track_devices = TrackDevicesClient::connect(adb_server_addr, None).await?;
    let mut tunneled: HashSet<String> = HashSet::new();

    while let Some(devices) = track_devices.next_snapshot().await? {
        let usable: HashSet<&str> = devices
            .iter()
            .filter(|d| d.state == USABLE_STATE)
            .map(|d| d.serial.as_str())
            .collect();

        for &serial in &usable {
            if !tunneled.contains(serial) {
                match adb.reverse(serial, local_port, local_port).await {
                    Ok(()) => {
                        tracing::info!(serial, "adb reverse tunnel set up");
                        tunneled.insert(serial.to_string());
                    }
                    Err(err) => {
                        tracing::warn!(
                            serial,
                            %err,
                            "failed to set up adb reverse tunnel, will retry on next snapshot"
                        );
                    }
                }
            }
        }

        tunneled.retain(|serial| usable.contains(serial.as_str()));
    }

    Ok(())
}

/// Runs `watch_and_tunnel` against the real local adb server forever,
/// retrying after a short delay if the connection fails or drops (e.g.
/// adb isn't installed yet, or its server hasn't started).
pub async fn watch_and_tunnel_forever(local_port: u16) {
    let adb_server_addr: SocketAddr = REAL_ADB_SERVER_ADDR
        .parse()
        .expect("REAL_ADB_SERVER_ADDR must be a valid socket address");
    let adb = AdbCli::new();

    loop {
        match watch_and_tunnel(adb_server_addr, &adb, local_port).await {
            Ok(()) => tracing::warn!("adb track-devices connection closed, retrying shortly"),
            Err(err) => tracing::warn!(%err, "adb device watch stopped, retrying shortly"),
        }
        tokio::time::sleep(RETRY_DELAY).await;
    }
}
