//! Shells out to the `adb` binary for the operations that only make sense
//! as one-shot commands (unlike `host:track-devices`, which needs a
//! long-lived connection and so talks to the adb server directly — see
//! `lib.rs`).

use std::ffi::OsString;
use std::io;
use tokio::process::Command;

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

    async fn run(&self, args: &[&str]) -> io::Result<()> {
        let output = Command::new(&self.program).args(args).output().await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "adb {} failed ({}): {}",
                args.join(" "),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim(),
            )))
        }
    }
}
