//! Keeps an adb reverse tunnel set up for every usable connected device,
//! so the bridge app on each device can always reach this daemon
//! regardless of USB replugs or wireless reconnects.

use bridge_adb::{AdbCli, TrackDevicesClient};
use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

const REAL_ADB_SERVER_ADDR: &str = "127.0.0.1:5037";
const USABLE_STATE: &str = "device";
const RETRY_DELAY: Duration = Duration::from_secs(5);

/// Identifies the companion app to keep installed and running on every
/// usable device, RenderDoc-style: connecting implies install-if-missing-
/// or-outdated, then launch-if-not-running. `None` (the default, since
/// there's no companion app yet) skips this step entirely and only
/// manages tunnels, matching prior behavior.
#[derive(Clone, Debug)]
pub struct AppConfig {
    pub package: String,
    /// The activity component to launch, e.g. `".MainActivity"` or a
    /// fully-qualified `"com.example.other/.OtherActivity"` — combined
    /// with `package` only if it doesn't already contain a `/`.
    pub activity: String,
    pub apk_path: PathBuf,
    /// The version code bundled in `apk_path`. A mismatch (including "not
    /// installed") triggers a reinstall.
    pub version_code: i64,
}

/// Installs `config.apk_path` on `serial` if the installed version code
/// doesn't match `config.version_code` (including "not installed" at
/// all), then launches `config.activity` if it isn't already running.
/// Errors from either step are the caller's to decide how to handle —
/// this doesn't retry internally.
pub async fn ensure_app_ready(
    adb: &AdbCli,
    serial: &str,
    config: &AppConfig,
) -> std::io::Result<()> {
    let installed_version = adb.installed_version_code(serial, &config.package).await?;
    if installed_version != Some(config.version_code) {
        adb.install(serial, &config.apk_path).await?;
    }

    if !adb.is_process_running(serial, &config.package).await? {
        let component = if config.activity.contains('/') {
            config.activity.clone()
        } else {
            format!("{}/{}", config.package, config.activity)
        };
        adb.start_activity(serial, &component).await?;
    }

    Ok(())
}

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
///
/// When `app_config` is set, also ensures the companion app is installed
/// (or up to date) and running on each newly-tunneled device — a
/// RenderDoc-style implicit connect. A failure here is logged and does
/// not affect tunnel tracking; it's retried the same way on the next
/// snapshot where the device is still newly-tunneled-looking (i.e. never,
/// today — see the note on `ensure_app_ready` call site below).
pub async fn watch_and_tunnel(
    adb_server_addr: SocketAddr,
    adb: &AdbCli,
    local_port: u16,
    app_config: Option<&AppConfig>,
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

                        if let Some(config) = app_config {
                            if let Err(err) = ensure_app_ready(adb, serial, config).await {
                                tracing::warn!(
                                    serial,
                                    %err,
                                    "failed to install/launch companion app"
                                );
                            }
                        }
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
pub async fn watch_and_tunnel_forever(local_port: u16, app_config: Option<AppConfig>) {
    let adb_server_addr: SocketAddr = REAL_ADB_SERVER_ADDR
        .parse()
        .expect("REAL_ADB_SERVER_ADDR must be a valid socket address");
    let adb = AdbCli::new();

    loop {
        match watch_and_tunnel(adb_server_addr, &adb, local_port, app_config.as_ref()).await {
            Ok(()) => tracing::warn!("adb track-devices connection closed, retrying shortly"),
            Err(err) => tracing::warn!(%err, "adb device watch stopped, retrying shortly"),
        }
        tokio::time::sleep(RETRY_DELAY).await;
    }
}
