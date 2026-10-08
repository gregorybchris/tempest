//! The other programs that tests run beside `tempest`, the tracker and the relay, and running
//! child processes.
//!
//! Cargo builds only this package's binary for its tests, so each program is built on first use
//! (`cargo build -p`, a no-op when it is up to date) and found beside `tempest` in the target
//! folder. A full `cargo test --workspace` has built them already.

use std::env::consts::EXE_SUFFIX;
use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{OnceLock, mpsc};
use std::time::Duration;

/// `tempest-tracker`, built if needed.
pub fn tracker() -> Command {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    Command::new(PATH.get_or_init(|| build("tempest-tracker-cli", "tempest-tracker")))
}

/// `tempest-relay`, built if needed.
pub fn relay() -> Command {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    Command::new(PATH.get_or_init(|| build("tempest-relay-cli", "tempest-relay")))
}

/// Builds `package` like the test run was built, and returns the path of its binary `bin`.
fn build(package: &str, bin: &str) -> PathBuf {
    let profile_dir = Path::new(env!("CARGO_BIN_EXE_tempest"))
        .parent()
        .expect("the binary is in a folder");
    let target_dir = profile_dir
        .parent()
        .expect("the profile is in the target folder");
    let mut cargo = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cargo
        .args(["build", "--quiet", "--package", package])
        .env("CARGO_TARGET_DIR", target_dir);
    if profile_dir
        .file_name()
        .is_some_and(|name| name == "release")
    {
        cargo.arg("--release");
    }
    let status = cargo.status().expect("cargo runs");
    assert!(status.success(), "cannot build {package}");
    profile_dir.join(format!("{bin}{EXE_SUFFIX}"))
}

/// A child process, killed when dropped.
pub struct Running(pub Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The lines `stream` yields, read on another thread.
///
/// It keeps reading once the receiver is gone, so the child never writes to a closed pipe,
/// which would make its next `eprintln!` panic.
pub fn lines(stream: impl Read + Send + 'static) -> mpsc::Receiver<String> {
    let (lines, received) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            let _ = lines.send(line);
        }
    });
    received
}

/// Starts `tempest-tracker` on 127.0.0.1 with a port the system picks and `extra` arguments,
/// and returns it with its URL.
pub fn start_tracker(extra: &[&OsStr]) -> (Running, String) {
    let mut tracker = Running(
        tracker()
            .args(["--bind", "127.0.0.1", "--port", "0"])
            .args(extra)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let line = lines(tracker.0.stderr.take().unwrap())
        .recv_timeout(Duration::from_secs(60))
        .expect("the tracker printed nothing");
    let url = line
        .strip_prefix("tracker listening on ")
        .and_then(|rest| rest.split(';').next())
        .unwrap_or_else(|| panic!("unexpected line: {line}"))
        .to_owned();
    (tracker, url)
}
