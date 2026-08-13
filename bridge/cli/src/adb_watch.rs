//! Keeps an adb reverse tunnel set up for every usable connected device,
//! so the bridge app on each device can always reach this daemon
//! regardless of USB replugs or wireless reconnects.

use bridge_adb::{AdbCli, TrackDevicesClient};
use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const REAL_ADB_SERVER_ADDR: &str = "127.0.0.1:5037";
const USABLE_STATE: &str = "device";
const RETRY_DELAY: Duration = Duration::from_secs(5);

/// The set of device serials currently tunneled, shared with whoever
/// wants to know it from outside the watch loop (e.g. `main` removing
/// every tunnel on shutdown — see `remove_all_reverse_tunnels`).
pub type TunneledDevices = Arc<Mutex<HashSet<String>>>;

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
/// (`"device"`) state, recording each in `tunneled`. Runs until the adb
/// server connection closes or errors.
///
/// A device's reverse tunnel doesn't survive it actually disconnecting —
/// it goes away with the adb transport — so there's no explicit "remove"
/// step here for that case; a device dropping out of the usable set just
/// means we stop treating it as tunneled (and drop it from `tunneled`),
/// so `reverse` runs again if/when it reappears. A device that's *still*
/// connected when this process exits needs an explicit removal instead —
/// see `remove_all_reverse_tunnels`, which `tunneled` exists for.
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
    tunneled: &TunneledDevices,
) -> std::io::Result<()> {
    let mut track_devices = TrackDevicesClient::connect(adb_server_addr, None).await?;

    while let Some(devices) = track_devices.next_snapshot().await? {
        let usable: HashSet<&str> = devices
            .iter()
            .filter(|d| d.state == USABLE_STATE)
            .map(|d| d.serial.as_str())
            .collect();

        // Snapshotted (and released) before the `.await`s below — a
        // std::sync::MutexGuard can't be held across an await point.
        let not_yet_tunneled: Vec<String> = {
            let tunneled = tunneled.lock().unwrap();
            usable
                .iter()
                .filter(|&&serial| !tunneled.contains(serial))
                .map(|&serial| serial.to_string())
                .collect()
        };

        for serial in not_yet_tunneled {
            match adb.reverse(&serial, local_port, local_port).await {
                Ok(()) => {
                    tracing::info!(serial, "adb reverse tunnel set up");
                    tunneled.lock().unwrap().insert(serial.clone());

                    if let Some(config) = app_config {
                        if let Err(err) = ensure_app_ready(adb, &serial, config).await {
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

        tunneled
            .lock()
            .unwrap()
            .retain(|serial| usable.contains(serial.as_str()));
    }

    Ok(())
}

/// Runs `watch_and_tunnel` against the real local adb server forever,
/// retrying after a short delay if the connection fails or drops (e.g.
/// adb isn't installed yet, or its server hasn't started). `tunneled`
/// persists across those retries (an adb-server-connection drop doesn't
/// tear down tunnels already set up at the OS/adb level), so a device
/// tunneled before a retry isn't redundantly re-tunneled after one.
pub async fn watch_and_tunnel_forever(
    local_port: u16,
    app_config: Option<AppConfig>,
    tunneled: TunneledDevices,
) {
    let adb_server_addr: SocketAddr = REAL_ADB_SERVER_ADDR
        .parse()
        .expect("REAL_ADB_SERVER_ADDR must be a valid socket address");
    let adb = AdbCli::new();

    loop {
        match watch_and_tunnel(
            adb_server_addr,
            &adb,
            local_port,
            app_config.as_ref(),
            &tunneled,
        )
        .await
        {
            Ok(()) => tracing::warn!("adb track-devices connection closed, retrying shortly"),
            Err(err) => tracing::warn!(%err, "adb device watch stopped, retrying shortly"),
        }
        tokio::time::sleep(RETRY_DELAY).await;
    }
}

/// Removes the adb reverse tunnel for every device currently in
/// `tunneled`, best-effort — a failure for one device is logged and
/// doesn't stop the rest from being attempted. Meant for daemon shutdown:
/// a tunnel doesn't tear itself down just because this process exits, so
/// without this, a device that's still connected afterward is left with
/// a stale mapping pointing at a port nothing is listening on anymore.
pub async fn remove_all_reverse_tunnels(adb: &AdbCli, tunneled: &TunneledDevices, local_port: u16) {
    let serials: Vec<String> = tunneled.lock().unwrap().iter().cloned().collect();
    for serial in serials {
        match adb.reverse_remove(&serial, local_port).await {
            Ok(()) => tracing::info!(serial, "removed adb reverse tunnel"),
            Err(err) => tracing::warn!(serial, %err, "failed to remove adb reverse tunnel"),
        }
    }
}
