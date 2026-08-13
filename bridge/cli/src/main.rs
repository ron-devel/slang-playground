use bridge_cli::adb_watch::{self, AppConfig};
use clap::Parser;
use std::net::SocketAddr;

const DEFAULT_PORT: u16 = 8800;

/// Bridge daemon: relays the web Slang playground to connected devices
/// over WebSocket, and keeps an adb reverse tunnel set up so each
/// device's own localhost:<port> reaches this daemon.
#[derive(Parser)]
struct Args {
    /// Port to listen on for WebSocket connections (also the port
    /// tunneled to each device via `adb reverse`, so both ends stay in
    /// sync automatically).
    #[arg(long, default_value_t = DEFAULT_PORT)]
    port: u16,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let args = Args::parse();
    let addr = SocketAddr::from(([127, 0, 0, 1], args.port));

    tokio::spawn(adb_watch::watch_and_tunnel_forever(
        args.port,
        app_config_from_env(),
    ));

    if let Err(err) = bridge_core::run(addr).await {
        tracing::error!("bridge-core exited with an error: {err}");
        std::process::exit(1);
    }
}

/// Builds an `AppConfig` from environment variables, or `None` if any are
/// unset — there's no companion app to manage yet, so this is opt-in and
/// off by default rather than pointing at a package that doesn't exist.
fn app_config_from_env() -> Option<AppConfig> {
    let package = std::env::var("BRIDGE_APP_PACKAGE").ok()?;
    let activity = std::env::var("BRIDGE_APP_ACTIVITY").ok()?;
    let apk_path = std::env::var("BRIDGE_APP_APK_PATH").ok()?;
    let version_code = std::env::var("BRIDGE_APP_VERSION_CODE")
        .ok()?
        .parse()
        .expect("BRIDGE_APP_VERSION_CODE must be an integer");

    Some(AppConfig {
        package,
        activity,
        apk_path: apk_path.into(),
        version_code,
    })
}
