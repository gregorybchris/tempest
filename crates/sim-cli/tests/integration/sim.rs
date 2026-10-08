//! `tempest-sim run` on an example scenario and on a broken one, and `tempest-sim baseline`.

use std::path::Path;
use std::process::{Command, Output};

fn tempest_sim(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tempest-sim"))
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
    let output = tempest_sim(&[
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
    assert!(
        stdout
            .lines()
            .any(|row| row.starts_with("default") && row.contains("2/2")),
        "{stdout}"
    );
    assert!(stdout.contains("across seeds"), "{stdout}");
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
    let output = Command::new(env!("CARGO_BIN_EXE_tempest-sim"))
        .args(["run", &example("two-peers.yaml"), "--seed", "3", "--no-db"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn compares_variants_with_the_chosen_reference() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vary.yaml");
    let text = std::fs::read_to_string(example("two-peers.yaml")).unwrap();
    let vary = "experiment: { vary: { requests.chunks: [16, 1] } }\n";
    std::fs::write(&path, text + vary).unwrap();
    let run = |reference: &str| {
        tempest_sim(&[
            "run",
            path.to_str().unwrap(),
            "--no-db",
            "--seeds",
            "5",
            "--reference",
            reference,
        ])
    };

    let output = run("requests.chunks=1");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let comparison = stdout.split("across seeds").nth(1).unwrap();
    let rows: Vec<&str> = comparison
        .lines()
        .filter(|l| l.contains("requests.chunks="))
        .collect();
    // One row per metric and variant. Requesting one chunk at a time gets the first byte
    // sooner on every seed.
    assert_eq!(rows.len(), 14, "{stdout}");
    assert!(rows[2].starts_with("completion p50"), "{stdout}");
    assert!(rows[3].contains("requests.chunks=1") && rows[3].ends_with("reference"));
    assert!(rows[6].starts_with("first byte p50"), "{stdout}");
    assert!(rows[6].ends_with("worse"), "{stdout}");

    // A part of more than one name is ambiguous.
    let output = run("requests");
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("exactly one of the variants"), "{stderr}");
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
    let output = tempest_sim(&["run", path.to_str().unwrap()]);
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("bad.yaml") && stderr.contains("line 3"),
        "{stderr}"
    );
}

#[test]
fn baselines_are_saved_then_checked() {
    let dir = tempfile::tempdir().unwrap();
    let scenarios = dir.path().join("scenarios");
    let baselines = dir.path().join("baselines");
    std::fs::create_dir_all(&scenarios).unwrap();
    std::fs::create_dir_all(&baselines).unwrap();
    let scenario = scenarios.join("two-peers.yaml");
    std::fs::copy(example("two-peers.yaml"), &scenario).unwrap();
    let baseline = |extra: &[&str]| {
        let mut args = vec![
            "baseline",
            "--scenarios",
            scenarios.to_str().unwrap(),
            "--baselines",
            baselines.to_str().unwrap(),
        ];
        args.extend(extra);
        tempest_sim(&args)
    };

    assert!(baseline(&["--update"]).status.success());
    assert!(baselines.join("two-peers.json").exists());
    for mode in [&[][..], &["--long"]] {
        let output = baseline(mode);
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(output.status.success(), "{stdout}");
        assert!(stdout.contains("completion p50") && !stdout.contains("REGRESSED"));
    }

    // A slower origin fails the check.
    let text = std::fs::read_to_string(&scenario).unwrap();
    std::fs::write(&scenario, text.replacen("up: 20Mbps", "up: 5Mbps", 1)).unwrap();
    let output = baseline(&[]);
    assert!(!output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("REGRESSED"), "{stdout}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("regressed: two-peers"), "{stderr}");
}
