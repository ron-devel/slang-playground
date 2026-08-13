use bridge_cli::adb_watch::{self, AppConfig};
use std::net::SocketAddr;

const DEFAULT_PORT: u16 = 8800;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let addr = SocketAddr::from(([127, 0, 0, 1], DEFAULT_PORT));

    tokio::spawn(adb_watch::watch_and_tunnel_forever(
        DEFAULT_PORT,
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
