//! The golden scenarios against their saved baselines (D-077, D-132), in quick mode.
//!
//! `tempest-sim baseline` runs the same check, and with `--long` over every saved seed; after a
//! deliberate change, `--update` rewrites the baselines.

use std::num::NonZeroUsize;
use std::path::PathBuf;

use tempest_sim::Scenario;
use tempest_sim::baseline::{Baseline, Golden, Mode, golden, measure, table};
use tempest_sim::units::Rate;

fn goldens() -> Vec<Golden> {
    let dir = |parts: &[&str]| -> PathBuf {
        std::iter::once(env!("CARGO_MANIFEST_DIR"))
            .chain(parts.iter().copied())
            .collect()
    };
    let goldens = golden(&dir(&["scenarios", "golden"]), &dir(&["baselines"])).unwrap();
    assert!(goldens.len() >= 5, "the golden scenarios are missing");
    goldens
}

fn threads() -> NonZeroUsize {
    std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN)
}

/// `scenario` with every peer's upload cut to 60%.
fn slowed(scenario: &Scenario) -> Scenario {
    let mut slow = scenario.clone();
    for class in &mut slow.peers {
        let up = &mut class.bandwidth.up;
        *up = Rate::bytes_per_second(up.get() * 0.6);
    }
    slow
}

/// Checks `scenario`'s runs in `mode` against the baseline of `golden`.
fn check_runs(golden: &Golden, scenario: &Scenario, mode: Mode) -> String {
    let baseline = Baseline::load(&golden.baseline).unwrap();
    let current = measure(scenario, &mode.seeds(scenario), threads()).unwrap();
    let checks = baseline.compare(&current, mode.method()).unwrap();
    let report = table(&checks);
    let slower = checks
        .iter()
        .any(|c| c.regressed && c.metric.starts_with("completion"));
    assert!(
        slower,
        "{} slowed didn't regress:\n{report}",
        golden.scenario.name
    );
    report
}

#[test]
fn golden_scenarios_meet_their_baselines() {
    let goldens = goldens();
    std::thread::scope(|scope| {
        let checks: Vec<_> = goldens
            .iter()
            .map(|g| scope.spawn(move || (g, g.check(Mode::Quick, threads()))))
            .collect();
        for check in checks {
            let (golden, checks) = check.join().unwrap();
            let name = &golden.scenario.name;
            let report = table(&checks.unwrap());
            println!("{name}:\n{report}");
            assert!(!report.contains("REGRESSED"), "{name} regressed:\n{report}");
            if report.contains("improved") {
                println!("Run `tempest-sim baseline --update` to save the improvements.");
            }
        }
    });
}

#[test]
fn baselines_hold_every_seed() {
    for golden in goldens() {
        let baseline = Baseline::load(&golden.baseline).unwrap();
        let seeds: Vec<u64> = baseline.runs.keys().copied().collect();
        assert_eq!(baseline.scenario, golden.scenario.name);
        assert_eq!(
            seeds,
            Mode::Long.seeds(&golden.scenario),
            "{}",
            baseline.scenario
        );
    }
}

#[test]
fn a_slower_swarm_fails_the_quick_check() {
    let goldens = goldens();
    std::thread::scope(|scope| {
        for golden in &goldens {
            scope.spawn(move || check_runs(golden, &slowed(&golden.scenario), Mode::Quick));
        }
    });
}

#[test]
#[ignore = "slow: every saved seed of every golden scenario; run with --include-ignored"]
fn a_slower_swarm_fails_the_long_check() {
    for golden in goldens() {
        let report = check_runs(&golden, &slowed(&golden.scenario), Mode::Long);
        println!("{} slowed:\n{report}", golden.scenario.name);
    }
}
