//! The other programs that tests run beside `tempest`, the tracker and the relay, and running
//! child processes.
//!
//! Cargo builds only this package's binary for its tests, so each program is built on first use
//! (`cargo build -p`, a no-op when it is up to date) and found beside `tempest` in the target
//! folder. A full `cargo test --workspace` has built them already. Nextest runs every test in a
//! process of its own, so there the first test of a run builds them and the rest wait for it,
//! rather than each running cargo.

use std::env::consts::EXE_SUFFIX;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{OnceLock, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long a command that tests run to completion may take.
pub const COMMAND_LIMIT: Duration = Duration::from_secs(120);

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

/// Builds `package` like the test run was built, once per nextest run, and returns the path of
/// its binary `bin`.
fn build(package: &str, bin: &str) -> PathBuf {
    let profile_dir = Path::new(env!("CARGO_BIN_EXE_tempest"))
        .parent()
        .expect("the binary is in a folder");
    let path = profile_dir.join(format!("{bin}{EXE_SUFFIX}"));
    let Some(run) = std::env::var_os("NEXTEST_RUN_ID") else {
        cargo_build(profile_dir, package);
        return path;
    };
    // Tests of this run that need it wait here while the first builds it.
    let lock = File::create(profile_dir.join(format!(".{package}-tests.lock")))
        .expect("cannot create the build lock");
    lock.lock().expect("cannot take the build lock");
    let built = profile_dir.join(format!(".{package}-tests.run"));
    if fs::read(&built).ok().as_deref() != Some(run.as_encoded_bytes()) || !path.exists() {
        cargo_build(profile_dir, package);
        fs::write(&built, run.as_encoded_bytes()).expect("cannot record the build");
    }
    path
}

/// Builds `package` into the target folder holding `profile_dir`, with its profile.
fn cargo_build(profile_dir: &Path, package: &str) {
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
}

/// Running a command to completion within a time limit.
pub trait OutputWithin {
    /// Like [`Command::output`], but kills the process and panics if it hasn't finished within
    /// `limit`, so a hung command fails its test rather than the whole run.
    fn output_within(&mut self, limit: Duration) -> io::Result<Output>;
}

impl OutputWithin for Command {
    fn output_within(&mut self, limit: Duration) -> io::Result<Output> {
        let mut child = Running(
            self.stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?,
        );
        let read = |stream: Option<Box<dyn Read + Send>>| -> JoinHandle<Vec<u8>> {
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                if let Some(mut stream) = stream {
                    let _ = stream.read_to_end(&mut bytes);
                }
                bytes
            })
        };
        let stdout = read(child.0.stdout.take().map(|s| Box::new(s) as _));
        let stderr = read(child.0.stderr.take().map(|s| Box::new(s) as _));
        let deadline = Instant::now() + limit;
        let status = loop {
            if let Some(status) = child.0.try_wait()? {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "{self:?} didn't finish within {limit:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        Ok(Output {
            status,
            stdout: stdout.join().expect("stdout is read"),
            stderr: stderr.join().expect("stderr is read"),
        })
    }
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
    let received = lines(tracker.0.stderr.take().unwrap());
    // Log events come first.
    let url = std::iter::from_fn(|| received.recv_timeout(Duration::from_secs(60)).ok())
        .find_map(|line| {
            let rest = line.strip_prefix("tracker listening on ")?;
            Some(rest.split(';').next()?.to_owned())
        })
        .expect("the tracker never said where it listens");
    (tracker, url)
}
