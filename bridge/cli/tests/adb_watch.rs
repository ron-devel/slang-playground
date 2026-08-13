//! Verified against a fake adb server (for host:track-devices) and a
//! fake, logging `adb` script (for AdbCli's `reverse` calls), so this
//! runs without any real adb installation or device present.
#![cfg(unix)]

use bridge_adb::AdbCli;
use bridge_cli::adb_watch::{
    remove_all_reverse_tunnels, watch_and_tunnel, AppConfig, TunneledDevices,
};
use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn empty_tunneled() -> TunneledDevices {
    Arc::new(Mutex::new(HashSet::new()))
}

fn write_frame(stream: &mut impl Write, payload: &str) {
    let header = format!("{:04x}", payload.len());
    stream.write_all(header.as_bytes()).unwrap();
    stream.write_all(payload.as_bytes()).unwrap();
}

/// Spawns a fake `host:track-devices` server that sends each of
/// `snapshots` in order, then keeps the connection open without closing
/// it (callers race the client against a timeout instead).
fn spawn_track_devices_server(snapshots: Vec<&'static str>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();

        let mut length_hex = [0u8; 4];
        socket.read_exact(&mut length_hex).unwrap();
        let length = u32::from_str_radix(std::str::from_utf8(&length_hex).unwrap(), 16).unwrap();
        let mut service = vec![0u8; length as usize];
        socket.read_exact(&mut service).unwrap();
        assert_eq!(service, b"host:track-devices");
        socket.write_all(b"OKAY").unwrap();

        for snapshot in snapshots {
            write_frame(&mut socket, snapshot);
        }

        std::thread::sleep(Duration::from_secs(2));
    });

    addr
}

/// A fake `adb` that appends every invocation's arguments to `log_path`
/// and always succeeds.
fn write_logging_fake_adb_script(dir: &Path, log_path: &Path) -> std::path::PathBuf {
    let script_path = dir.join("fake-adb.sh");
    let script = format!(
        "#!/bin/sh\necho \"$*\" >> '{}'\nexit 0\n",
        log_path.display()
    );
    std::fs::write(&script_path, script).unwrap();
    let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms).unwrap();
    script_path
}

#[tokio::test]
async fn tunnels_usable_devices_and_retries_on_reappearance() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("adb.log");
    let adb = AdbCli::with_program(write_logging_fake_adb_script(dir.path(), &log_path));

    let addr = spawn_track_devices_server(vec![
        // A usable device alongside one that should never be tunneled.
        "good-serial\tdevice\nbad-serial\tunauthorized\n",
        // good-serial disappears...
        "",
        // ...then reappears, which should trigger `reverse` again.
        "good-serial\tdevice\n",
    ]);

    // watch_and_tunnel only returns once the connection closes, which
    // this test doesn't do — race it against a generous timeout instead,
    // long enough for the three snapshots above to be fully processed.
    let tunneled = empty_tunneled();
    let _ = tokio::time::timeout(
        Duration::from_secs(1),
        watch_and_tunnel(addr, &adb, 8800, None, &tunneled),
    )
    .await;

    let log = std::fs::read_to_string(&log_path).unwrap_or_default();
    let reverse_calls: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("good-serial"))
        .collect();

    assert_eq!(
        reverse_calls,
        vec![
            "-s good-serial reverse tcp:8800 tcp:8800",
            "-s good-serial reverse tcp:8800 tcp:8800",
        ],
        "expected reverse for good-serial once, then again after it reappeared; full log: {log}"
    );
    assert!(
        !log.contains("bad-serial"),
        "an unauthorized device should never be tunneled; full log: {log}"
    );
    assert_eq!(
        *tunneled.lock().unwrap(),
        HashSet::from(["good-serial".to_string()]),
        "expected only good-serial to remain in the shared tunneled set"
    );
}

/// A fake `adb` that logs every invocation like
/// `write_logging_fake_adb_script`, but also plays along with
/// `ensure_app_ready`'s probes: `dumpsys package` reports the app as not
/// installed (empty stdout), and `pidof` reports it as not running (exit
/// 1) — so both `install` and `am start` should get invoked.
fn write_app_lifecycle_fake_adb_script(dir: &Path, log_path: &Path) -> std::path::PathBuf {
    let script_path = dir.join("fake-adb.sh");
    let script = format!(
        r#"#!/bin/sh
echo "$*" >> '{log}'
case "$*" in
  *"shell dumpsys package"*) exit 0 ;;
  *"shell pidof"*) exit 1 ;;
  *) exit 0 ;;
esac
"#,
        log = log_path.display()
    );
    std::fs::write(&script_path, script).unwrap();
    let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms).unwrap();
    script_path
}

#[tokio::test]
async fn installs_and_launches_the_companion_app_on_a_newly_tunneled_device() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("adb.log");
    let adb = AdbCli::with_program(write_app_lifecycle_fake_adb_script(dir.path(), &log_path));

    let addr = spawn_track_devices_server(vec!["good-serial\tdevice\n"]);

    let app_config = AppConfig {
        package: "com.example.app".to_string(),
        activity: ".MainActivity".to_string(),
        apk_path: "/tmp/app.apk".into(),
        version_code: 1,
    };

    let _ = tokio::time::timeout(
        Duration::from_secs(1),
        watch_and_tunnel(addr, &adb, 8800, Some(&app_config), &empty_tunneled()),
    )
    .await;

    let log = std::fs::read_to_string(&log_path).unwrap_or_default();
    assert!(
        log.contains("-s good-serial shell dumpsys package com.example.app"),
        "expected a version check; full log: {log}"
    );
    assert!(
        log.contains("-s good-serial install -r /tmp/app.apk"),
        "expected an install since dumpsys reported the app as not installed; full log: {log}"
    );
    assert!(
        log.contains("-s good-serial shell pidof com.example.app"),
        "expected a running check; full log: {log}"
    );
    assert!(
        log.contains(
            "-s good-serial shell am start --activity-single-top -n com.example.app/.MainActivity"
        ),
        "expected a launch since pidof reported the app as not running; full log: {log}"
    );
}

#[tokio::test]
async fn removes_the_reverse_tunnel_for_every_currently_tunneled_device() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("adb.log");
    let adb = AdbCli::with_program(write_logging_fake_adb_script(dir.path(), &log_path));

    let tunneled: TunneledDevices = Arc::new(Mutex::new(HashSet::from([
        "serial-a".to_string(),
        "serial-b".to_string(),
    ])));

    remove_all_reverse_tunnels(&adb, &tunneled, 8800).await;

    let log = std::fs::read_to_string(&log_path).unwrap_or_default();
    assert!(
        log.contains("-s serial-a reverse --remove tcp:8800"),
        "expected serial-a's tunnel to be removed; full log: {log}"
    );
    assert!(
        log.contains("-s serial-b reverse --remove tcp:8800"),
        "expected serial-b's tunnel to be removed; full log: {log}"
    );
}
