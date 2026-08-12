//! Verified against a fake `adb` script rather than the real binary, so
//! this runs without any adb installation or device present. The
//! `unexpected args` fallback case doubles as a check that AdbCli invokes
//! adb with exactly the arguments it should — any mismatch falls through
//! to it and fails the test with a clear message.
#![cfg(unix)]

use bridge_adb::AdbCli;
use std::os::unix::fs::PermissionsExt;

fn write_fake_adb_script(dir: &std::path::Path) -> std::path::PathBuf {
    let script_path = dir.join("fake-adb.sh");
    let script = r#"#!/bin/sh
case "$*" in
  "-s test-serial reverse tcp:8800 tcp:8800") exit 0 ;;
  "-s test-serial reverse --remove tcp:8800") exit 0 ;;
  "connect 192.168.1.50:5555") exit 0 ;;
  "-s missing-serial reverse tcp:8800 tcp:8800")
    echo "error: device 'missing-serial' not found" >&2
    exit 1
    ;;
  *)
    echo "fake-adb: unexpected args: $*" >&2
    exit 1
    ;;
esac
"#;
    std::fs::write(&script_path, script).unwrap();
    let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms).unwrap();
    script_path
}

#[tokio::test]
async fn reverse_and_remove_succeed_with_correct_arguments() {
    let dir = tempfile::tempdir().unwrap();
    let adb = AdbCli::with_program(write_fake_adb_script(dir.path()));

    adb.reverse("test-serial", 8800, 8800).await.unwrap();
    adb.reverse_remove("test-serial", 8800).await.unwrap();
}

#[tokio::test]
async fn connect_succeeds_with_correct_arguments() {
    let dir = tempfile::tempdir().unwrap();
    let adb = AdbCli::with_program(write_fake_adb_script(dir.path()));

    adb.connect("192.168.1.50:5555").await.unwrap();
}

#[tokio::test]
async fn surfaces_adb_failure_with_stderr_message() {
    let dir = tempfile::tempdir().unwrap();
    let adb = AdbCli::with_program(write_fake_adb_script(dir.path()));

    let err = adb
        .reverse("missing-serial", 8800, 8800)
        .await
        .expect_err("should fail when adb reports the device isn't found");
    assert!(
        err.to_string().contains("not found"),
        "expected the adb stderr message in the error, got: {err}"
    );
}
