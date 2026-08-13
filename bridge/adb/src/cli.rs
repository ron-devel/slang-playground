//! Shells out to the `adb` binary for the operations that only make sense
//! as one-shot commands (unlike `host:track-devices`, which needs a
//! long-lived connection and so talks to the adb server directly — see
//! `lib.rs`).

use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::process::Output;
use std::time::Duration;
use tokio::process::Command;

/// ETXTBSY: spawning an executable can transiently fail with this if it
/// was very recently written (a package manager updating `adb`, or on
/// some platforms antivirus/EDR software briefly holding it open) — worth
/// a few quick retries rather than failing outright.
const ETXTBSY: i32 = 26;
const MAX_SPAWN_ATTEMPTS: u32 = 5;
const SPAWN_RETRY_DELAY: Duration = Duration::from_millis(20);

/// Wraps invocations of the `adb` command-line tool. The adb binary
/// defaults to whatever `adb` resolves to on `PATH`; tests point this at
/// a fake stand-in script instead.
pub struct AdbCli {
    program: OsString,
}

impl Default for AdbCli {
    fn default() -> Self {
        Self::new()
    }
}

impl AdbCli {
    pub fn new() -> Self {
        Self {
            program: "adb".into(),
        }
    }

    /// Uses a specific `adb` executable instead of resolving it from
    /// `PATH` — primarily for pointing tests at a fake stand-in script.
    pub fn with_program(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
        }
    }

    /// Sets up `adb -s <serial> reverse tcp:<remote> tcp:<local>`: on the
    /// device, connections to `tcp:remote` (typically the port the bridge
    /// app connects to on its own localhost) are tunneled to `tcp:local`
    /// on this host (where the bridge daemon actually listens).
    /// Idempotent — adb replaces any existing mapping for the same remote
    /// port on this device rather than erroring.
    pub async fn reverse(&self, serial: &str, remote_port: u16, local_port: u16) -> io::Result<()> {
        self.run(&[
            "-s",
            serial,
            "reverse",
            &format!("tcp:{remote_port}"),
            &format!("tcp:{local_port}"),
        ])
        .await
    }

    /// Removes a reverse tunnel previously set up with `reverse`. Safe to
    /// call even if no such mapping exists (adb treats that as a no-op,
    /// not an error) — e.g. after a device disconnects.
    pub async fn reverse_remove(&self, serial: &str, remote_port: u16) -> io::Result<()> {
        self.run(&[
            "-s",
            serial,
            "reverse",
            "--remove",
            &format!("tcp:{remote_port}"),
        ])
        .await
    }

    /// Runs `adb connect <host_port>` (e.g. `"192.168.1.50:5555"`) to pair
    /// with or reconnect to a wireless-debugging device.
    pub async fn connect(&self, host_port: &str) -> io::Result<()> {
        self.run(&["connect", host_port]).await
    }

    /// Installs (or, with `-r`, reinstalls/updates) the APK at `apk_path`
    /// on the given device. Some adb/Android versions report a failed
    /// install as `Failure [REASON]` on stdout while still exiting 0, so
    /// this checks stdout in addition to the exit status rather than
    /// trusting the exit status alone.
    pub async fn install(&self, serial: &str, apk_path: &Path) -> io::Result<()> {
        let apk_path = apk_path.to_string_lossy();
        let args = ["-s", serial, "install", "-r", apk_path.as_ref()];
        let output = self.spawn_capturing(&args).await?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        if output.status.success() && !stdout.contains("Failure") {
            Ok(())
        } else {
            Err(Self::command_failed(&args, &output))
        }
    }

    /// Reads the installed version code of `package` on the given device
    /// via `dumpsys package`, or `None` if it isn't installed. Returns an
    /// error only for an actual adb/shell failure, not for "not
    /// installed" (which shows up as output with no `versionCode=` line).
    pub async fn installed_version_code(
        &self,
        serial: &str,
        package: &str,
    ) -> io::Result<Option<i64>> {
        let args = ["-s", serial, "shell", "dumpsys", "package", package];
        let output = self.spawn_capturing(&args).await?;
        if !output.status.success() {
            return Err(Self::command_failed(&args, &output));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.split("versionCode=").nth(1).and_then(|rest| {
            rest.split_whitespace()
                .next()
                .and_then(|token| token.parse::<i64>().ok())
        }))
    }

    /// Checks whether `package` currently has a running process on the
    /// device, via `pidof`. A "not found" result from `pidof` (typically
    /// exit status 1, empty stdout) means "not running", not an error.
    pub async fn is_process_running(&self, serial: &str, package: &str) -> io::Result<bool> {
        let args = ["-s", serial, "shell", "pidof", package];
        let output = self.spawn_capturing(&args).await?;
        Ok(!String::from_utf8_lossy(&output.stdout).trim().is_empty())
    }

    /// Launches `component` (e.g. `"com.example.app/.MainActivity"`) via
    /// `am start`. Some Android versions report a failed launch as an
    /// `Error:` line on stdout while still exiting 0, so this checks
    /// stdout in addition to the exit status rather than trusting the
    /// exit status alone.
    pub async fn start_activity(&self, serial: &str, component: &str) -> io::Result<()> {
        let args = ["-s", serial, "shell", "am", "start", "-n", component];
        let output = self.spawn_capturing(&args).await?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        if output.status.success() && !stdout.contains("Error:") {
            Ok(())
        } else {
            Err(Self::command_failed(&args, &output))
        }
    }

    async fn run(&self, args: &[&str]) -> io::Result<()> {
        let output = self.spawn_capturing(args).await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(Self::command_failed(args, &output))
        }
    }

    /// Runs adb with the given arguments and returns its full output
    /// regardless of exit status, retrying transient `ETXTBSY` spawn
    /// failures. Callers decide for themselves what counts as success —
    /// e.g. `pidof`'s "not found" exit status isn't really a failure.
    async fn spawn_capturing(&self, args: &[&str]) -> io::Result<Output> {
        for attempt in 1..=MAX_SPAWN_ATTEMPTS {
            match Command::new(&self.program).args(args).output().await {
                Ok(output) => return Ok(output),
                Err(err) if err.raw_os_error() == Some(ETXTBSY) && attempt < MAX_SPAWN_ATTEMPTS => {
                    tokio::time::sleep(SPAWN_RETRY_DELAY).await;
                }
                Err(err) => return Err(err),
            }
        }
        unreachable!("loop above always returns before exhausting MAX_SPAWN_ATTEMPTS")
    }

    fn command_failed(args: &[&str], output: &Output) -> io::Error {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        io::Error::other(format!(
            "adb {} failed ({}): {} {}",
            args.join(" "),
            output.status,
            stderr.trim(),
            stdout.trim(),
        ))
    }
}
