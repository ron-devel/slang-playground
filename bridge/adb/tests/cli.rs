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
  "-s test-serial install -r /tmp/app.apk")
    echo "Success"
    exit 0
    ;;
  "-s test-serial install -r /tmp/bad.apk")
    # Legacy adb/Android: exits 0 even though the install failed.
    echo "Failure [INSTALL_FAILED_INVALID_APK]"
    exit 0
    ;;
  "-s test-serial shell dumpsys package com.example.app")
    cat <<'EOF'
Packages:
  Package [com.example.app] (deadbeef):
    versionCode=42 minSdk=24 targetSdk=34
    versionName=1.2.3
EOF
    exit 0
    ;;
  "-s test-serial shell dumpsys package com.example.missing")
    echo "Unable to find package: com.example.missing"
    exit 0
    ;;
  "-s test-serial shell pidof com.example.app")
    echo "12345"
    exit 0
    ;;
  "-s test-serial shell pidof com.example.missing")
    exit 1
    ;;
  "-s test-serial shell am start -n com.example.app/.MainActivity")
    echo "Starting: Intent { cmp=com.example.app/.MainActivity }"
    exit 0
    ;;
  "-s test-serial shell am start -n com.example.app/.MissingActivity")
    # Some adb/Android versions exit 0 even on a resolution failure.
    echo "Error: Activity not started, unable to resolve Intent"
    exit 0
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

#[tokio::test]
async fn install_succeeds_on_success_output() {
    let dir = tempfile::tempdir().unwrap();
    let adb = AdbCli::with_program(write_fake_adb_script(dir.path()));

    adb.install("test-serial", std::path::Path::new("/tmp/app.apk"))
        .await
        .unwrap();
}

#[tokio::test]
async fn install_fails_on_failure_output_even_with_exit_code_zero() {
    let dir = tempfile::tempdir().unwrap();
    let adb = AdbCli::with_program(write_fake_adb_script(dir.path()));

    let err = adb
        .install("test-serial", std::path::Path::new("/tmp/bad.apk"))
        .await
        .expect_err("a `Failure [...]` line on stdout should be treated as a failure");
    assert!(
        err.to_string().contains("INSTALL_FAILED_INVALID_APK"),
        "expected the failure reason in the error, got: {err}"
    );
}

#[tokio::test]
async fn installed_version_code_parses_dumpsys_output() {
    let dir = tempfile::tempdir().unwrap();
    let adb = AdbCli::with_program(write_fake_adb_script(dir.path()));

    let version = adb
        .installed_version_code("test-serial", "com.example.app")
        .await
        .unwrap();
    assert_eq!(version, Some(42));
}

#[tokio::test]
async fn installed_version_code_is_none_when_package_missing() {
    let dir = tempfile::tempdir().unwrap();
    let adb = AdbCli::with_program(write_fake_adb_script(dir.path()));

    let version = adb
        .installed_version_code("test-serial", "com.example.missing")
        .await
        .unwrap();
    assert_eq!(version, None);
}

#[tokio::test]
async fn is_process_running_reflects_pidof_output() {
    let dir = tempfile::tempdir().unwrap();
    let adb = AdbCli::with_program(write_fake_adb_script(dir.path()));

    assert!(adb
        .is_process_running("test-serial", "com.example.app")
        .await
        .unwrap());
    assert!(!adb
        .is_process_running("test-serial", "com.example.missing")
        .await
        .unwrap());
}

#[tokio::test]
async fn start_activity_succeeds_with_correct_arguments() {
    let dir = tempfile::tempdir().unwrap();
    let adb = AdbCli::with_program(write_fake_adb_script(dir.path()));

    adb.start_activity("test-serial", "com.example.app/.MainActivity")
        .await
        .unwrap();
}

#[tokio::test]
async fn start_activity_fails_on_error_output_even_with_exit_code_zero() {
    let dir = tempfile::tempdir().unwrap();
    let adb = AdbCli::with_program(write_fake_adb_script(dir.path()));

    let err = adb
        .start_activity("test-serial", "com.example.app/.MissingActivity")
        .await
        .expect_err("an `Error:` line on stdout should be treated as a failure");
    assert!(
        err.to_string().contains("unable to resolve Intent"),
        "expected the am start error message in the error, got: {err}"
    );
}
