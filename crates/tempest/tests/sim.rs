//! `tempest sim run` on an example scenario and on a broken one.

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
fn runs_a_scenario_records_it_and_prints_a_summary() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("results.duckdb");
    let digests = dir.path().join("digests.tsv");
    let output = tempest(&[
        "sim",
        "run",
        &example("two-peers.yaml"),
        "--seeds",
        "2",
        "--db",
        db.to_str().unwrap(),
        "--digests",
        digests.to_str().unwrap(),
    ]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("1 variant × 2 seeds = 2 runs"), "{stdout}");
    assert!(stdout.contains("(experiment 1)"), "{stdout}");
    let row = stdout.lines().last().unwrap();
    assert!(
        row.starts_with("default") && row.contains("2/2"),
        "{stdout}"
    );
    assert!(db.exists());

    // One line per run: variant, seed and a SHA-256 digest.
    let digests = std::fs::read_to_string(&digests).unwrap();
    let lines: Vec<Vec<&str>> = digests.lines().map(|l| l.split('\t').collect()).collect();
    assert_eq!(lines.len(), 2, "{digests}");
    for (seed, line) in lines.iter().enumerate() {
        assert_eq!(line[..2], ["default", &(seed + 1).to_string()]);
        assert_eq!(line[2].len(), 64);
    }
}

#[test]
fn no_db_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tempest"))
        .args([
            "sim",
            "run",
            &example("two-peers.yaml"),
            "--seed",
            "3",
            "--no-db",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
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
