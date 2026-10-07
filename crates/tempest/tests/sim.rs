//! `tempest sim run` on the example scenarios and on a broken one.

use std::path::Path;
use std::process::{Command, Output};

fn tempest(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tempest"))
        .args(args)
        .output()
        .unwrap()
}

fn example(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../sim/scenarios")
        .join(name)
        .display()
        .to_string()
}

#[test]
fn runs_a_scenario_and_prints_a_summary() {
    let output = tempest(&["sim", "run", &example("two-peers.yaml"), "--seeds", "2"]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("1 variant × 2 seeds = 2 runs"), "{stdout}");
    let row = stdout.lines().last().unwrap();
    assert!(
        row.starts_with("default") && row.contains("2/2"),
        "{stdout}"
    );
}

#[test]
fn a_bad_scenario_names_the_field() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.yaml");
    std::fs::write(
        &path,
        "name: bad\nbundle: { files: [{ size: 1MiB }] }\npeers: [{ class: a, count: lots }]\n",
    )
    .unwrap();
    let output = tempest(&["sim", "run", path.to_str().unwrap()]);
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("bad.yaml") && stderr.contains("line 3"),
        "{stderr}"
    );
}
