//! The command line's shape.

use crate::common;

#[test]
fn the_version_names_the_program() {
    let output = common::tracker().arg("--version").output().unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.starts_with("tempest-tracker "), "{text}");
}

/// The relay now runs on its own (D-236).
#[test]
fn the_tracker_does_not_run_a_relay() {
    let output = common::tracker().arg("--run-relay").output().unwrap();
    assert!(!output.status.success());
}

/// Behind a proxy, no client reaches the tracker at a loopback address, so without a public URL
/// nobody could sign in (D-239).
#[test]
fn a_catalog_behind_a_proxy_needs_a_public_url() {
    let dir = tempfile::tempdir().unwrap();
    let output = common::tracker()
        .args(["--behind-proxy", "--port", "0", "--catalog"])
        .arg(dir.path().join("catalog.db"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    let text = String::from_utf8(output.stderr).unwrap();
    assert!(text.contains("--public-url"), "{text}");
}
