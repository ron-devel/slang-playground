//! Verified against a fake adb server (for host:track-devices) and a
//! fake, logging `adb` script (for AdbCli's `reverse` calls), so this
//! runs without any real adb installation or device present.
#![cfg(unix)]

use bridge_adb::AdbCli;
use bridge_cli::adb_watch::watch_and_tunnel;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

fn write_frame(stream: &mut impl Write, payload: &str) {
    let header = format!("{:04x}", payload.len());
    stream.write_all(header.as_bytes()).unwrap();
    stream.write_all(payload.as_bytes()).unwrap();
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

        // A usable device alongside one that should never be tunneled.
        write_frame(
            &mut socket,
            "good-serial\tdevice\nbad-serial\tunauthorized\n",
        );
        // good-serial disappears...
        write_frame(&mut socket, "");
        // ...then reappears, which should trigger `reverse` again.
        write_frame(&mut socket, "good-serial\tdevice\n");

        std::thread::sleep(Duration::from_secs(2));
    });

    // watch_and_tunnel only returns once the connection closes, which
    // this test doesn't do — race it against a generous timeout instead,
    // long enough for the three snapshots above to be fully processed.
    let _ = tokio::time::timeout(Duration::from_secs(1), watch_and_tunnel(addr, &adb, 8800)).await;

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
}
