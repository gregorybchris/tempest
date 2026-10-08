//! The other programs that tests run beside `tempest`: the tracker and the relay.
//!
//! Cargo builds only this package's binary for its tests, so each program is built on first use
//! (`cargo build -p`, a no-op when it is up to date) and found beside `tempest` in the target
//! folder. A full `cargo test --workspace` has built them already.

use std::env::consts::EXE_SUFFIX;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

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
