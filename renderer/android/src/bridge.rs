//! JNI glue connecting the app to the bridge daemon as a live target
//! peer, via `bridge-target-client`. Kept separate from the render path
//! (`lib.rs`) — the two are independent: rendering runs regardless of
//! whether a bridge connection exists, and vice versa.
//!
//! Each call to `nativeConnectAndWait` owns its connection attempt
//! start-to-finish on the calling (Kotlin-owned) thread: build a fresh
//! single-threaded Tokio runtime, connect, then block until the
//! connection closes or `nativeRequestShutdown` cancels it. Kotlin drives
//! any reconnect-on-disconnect policy by simply calling this again — see
//! `BridgeClient.kt`'s loop — so there's no persistent state here beyond
//! the one in-flight shutdown signal below.

use bridge_target_client::TargetClient;
use jni::objects::{JClass, JString};
use jni::sys::{jboolean, JNI_FALSE, JNI_TRUE};
use jni::JNIEnv;
use std::sync::Mutex;
use tokio::sync::oneshot;

/// Set while a `nativeConnectAndWait` call is in flight, so
/// `nativeRequestShutdown` has something to cancel. Only one bridge
/// connection is ever attempted at a time (matching the daemon's own
/// single-target model), so a single slot is enough — no connection
/// handle/registry needed.
static SHUTDOWN_TX: Mutex<Option<oneshot::Sender<()>>> = Mutex::new(None);

fn jstring_to_string(env: &mut JNIEnv, value: &JString) -> Option<String> {
    env.get_string(value).ok().map(|s| s.into())
}

/// Connects to the bridge daemon at `url` and identifies as `display_name`,
/// then blocks the calling thread until the connection closes (server
/// shutdown, network error, or `nativeRequestShutdown` being called).
/// Returns `true` if the connection was established at all (even if it
/// later closed), `false` if the initial connect attempt itself failed
/// (bad URL, daemon unreachable, handshake rejected, ...) — Kotlin uses
/// this only to decide how noisily to log before retrying, not as a hard
/// error signal.
///
/// # Safety
/// `env`/`url`/`display_name` come directly from a JNI call.
#[no_mangle]
pub extern "system" fn Java_dev_slangplayground_app_bridge_BridgeClient_nativeConnectAndWait(
    mut env: JNIEnv,
    _class: JClass,
    url: JString,
    display_name: JString,
) -> jboolean {
    let Some(url) = jstring_to_string(&mut env, &url) else {
        return JNI_FALSE;
    };
    let Some(display_name) = jstring_to_string(&mut env, &display_name) else {
        return JNI_FALSE;
    };

    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return JNI_FALSE;
    };

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    *SHUTDOWN_TX.lock().unwrap() = Some(shutdown_tx);

    let connected = runtime.block_on(async {
        let Ok(mut client) = TargetClient::connect(&url, &display_name).await else {
            return false;
        };
        tokio::select! {
            () = client.wait_until_closed() => {}
            _ = shutdown_rx => {}
        }
        true
    });

    // Already consumed if `nativeRequestShutdown` fired; otherwise this
    // connection ended on its own (server/network side), so the sender
    // here is now stale — clear it either way rather than leaving a
    // sender for a connection that no longer exists.
    *SHUTDOWN_TX.lock().unwrap() = None;

    if connected {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

/// Cancels an in-flight `nativeConnectAndWait` call, if any, causing it
/// to return promptly instead of waiting for the daemon or network to
/// close the connection on their own. A no-op if no call is currently in
/// flight.
#[no_mangle]
pub extern "system" fn Java_dev_slangplayground_app_bridge_BridgeClient_nativeRequestShutdown(
    _env: JNIEnv,
    _class: JClass,
) {
    if let Some(tx) = SHUTDOWN_TX.lock().unwrap().take() {
        let _ = tx.send(());
    }
}
