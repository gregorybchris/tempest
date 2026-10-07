//! The golden regression scenario against its saved baseline (D-077, D-132).
//!
//! By default the first few seeds run, which takes about a second. For more certainty, set
//! `TEMPEST_BASELINE_SEEDS` to run more, up to the scenario's `seeds`. After a deliberate
//! change, set `TEMPEST_UPDATE_BASELINE=1` to rewrite the baseline from every seed.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::PathBuf;

use tempest_sim::baseline::{Baseline, Metrics, table};
use tempest_sim::experiment::run_all;
use tempest_sim::{RunOptions, Scenario};

/// Seeds run by default.
const DEFAULT_SEEDS: u64 = 5;

fn path(dir: &str, file: &str) -> PathBuf {
    [env!("CARGO_MANIFEST_DIR"), dir, file].iter().collect()
}

fn metrics(scenario: &Scenario, seeds: u64) -> BTreeMap<u64, Metrics> {
    let seeds: Vec<u64> = (1..=seeds).collect();
    let threads = std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN);
    run_all(scenario, &seeds, threads, RunOptions::default(), &())
        .unwrap()
        .iter()
        .map(|r| (r.seed, Metrics::of(r, scenario)))
        .collect()
}

#[test]
fn golden_scenario_meets_its_baseline() {
    let scenario = Scenario::load(&path("scenarios", "golden.yaml")).unwrap();
    let file = path("baselines", "golden.json");
    let stored = u64::from(scenario.seeds);

    if std::env::var_os("TEMPEST_UPDATE_BASELINE").is_some() {
        let baseline = Baseline {
            scenario: scenario.name.clone(),
            runs: metrics(&scenario, stored),
        };
        let json = serde_json::to_string_pretty(&baseline).unwrap();
        std::fs::write(&file, json + "\n").unwrap();
        return;
    }

    let seeds = match std::env::var("TEMPEST_BASELINE_SEEDS") {
        Ok(n) => n.parse().expect("TEMPEST_BASELINE_SEEDS is a number"),
        Err(_) => DEFAULT_SEEDS,
    };
    assert!(
        (1..=stored).contains(&seeds),
        "the baseline has seeds 1 to {stored}"
    );
    let text = std::fs::read_to_string(&file).unwrap();
    let baseline: Baseline = serde_json::from_str(&text).unwrap();
    assert_eq!(baseline.scenario, scenario.name);

    let checks = baseline.compare(&metrics(&scenario, seeds)).unwrap();
    let report = table(&checks);
    println!("golden scenario, seeds 1 to {seeds}:\n{report}");
    assert!(
        checks.iter().all(|c| !c.regressed),
        "the golden scenario regressed:\n{report}"
    );
    if checks.iter().any(|c| c.improved) {
        println!("Some metrics improved: rerun with TEMPEST_UPDATE_BASELINE=1 to save them.");
    }
}
