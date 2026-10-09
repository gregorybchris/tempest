//! What the results UI reads agrees with the runs that made the results: listings filter and
//! sort, run details add up, comparisons match the statistics `tempest-sim run` prints, and
//! experiments are checked against golden baselines.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::Path;

use tempest_sim::baseline::{self, Baseline};
use tempest_sim::browse::{self, Baselines, BrowseError};
use tempest_sim::experiment::{run_all, seeds};
use tempest_sim::results::{Database, ScenarioFile};
use tempest_sim::stats::{self, Bootstrap, Meaningful};
use tempest_sim::{RunOptions, RunResult, Scenario};
use tempest_sim_api::data::{Method, MetricUnit, Verdict};
use tempest_sim_api::query::{
    CompareQuery, ExperimentQuery, ExperimentSort, Outcome, RunQuery, RunSort, VariantRef,
};

/// Two variants with six seeds each: enough paired seeds for a verdict (D-167).
const SWARM: &str = "
name: swarm
description: A small crowd, to browse.
seeds: 6
sample_interval: 500ms
bundle: { files: [{ size: 1MiB }] }
network: { latency: 10ms }
peers:
  - { class: origin, starts: complete, bandwidth: { up: 20Mbps, down: 100Mbps } }
  - class: home
    count: 6
    bandwidth: { up: 5Mbps, down: 50Mbps }
    arrivals: { pattern: flash-crowd, decay: 2s }
  - { class: free-rider, behavior: free-rider, bandwidth: { up: 5Mbps, down: 50Mbps } }
experiment:
  vary:
    strategies.upload: [bittorrent, cooperative]
";

/// One variant, as golden scenarios have.
const GOLDEN: &str = "
name: golden-pair
seeds: 4
bundle: { files: [{ size: 512KiB }] }
peers:
  - { class: origin, starts: complete, bandwidth: { up: 20Mbps, down: 100Mbps } }
  - { class: home, count: 3, bandwidth: { up: 5Mbps, down: 50Mbps } }
";

fn threads() -> NonZeroUsize {
    std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN)
}

fn record(db: &Database, yaml: &str, events: bool) -> (Scenario, Vec<RunResult>) {
    let scenario = Scenario::from_yaml(yaml).unwrap();
    let seeds = seeds(&scenario);
    let file = ScenarioFile::new(Path::new("scenario.yaml"), yaml);
    let recorder = db.record(&scenario, Some(&file), &seeds, events).unwrap();
    let results = run_all(
        &scenario,
        &seeds,
        threads(),
        RunOptions::default(),
        &recorder,
    )
    .unwrap();
    recorder.finish().unwrap();
    (scenario, results)
}

/// A database with the swarm (experiment 1, with event logs) and the golden scenario
/// (experiment 2, without), and a baseline directory holding the golden scenario's exact
/// results.
struct Fixture {
    db: Database,
    swarm: (Scenario, Vec<RunResult>),
    golden: (Scenario, Vec<RunResult>),
    baselines: tempfile::TempDir,
}

fn fixture() -> Fixture {
    let db = Database::open_in_memory().unwrap();
    let swarm = record(&db, SWARM, true);
    let golden = record(&db, GOLDEN, false);
    let baselines = tempfile::tempdir().unwrap();
    let saved = Baseline {
        scenario: "golden-pair".to_owned(),
        scenario_sha256: Scenario::file_sha256(GOLDEN),
        runs: golden
            .1
            .iter()
            .map(|r| (r.seed, baseline::Metrics::of(r, &golden.0)))
            .collect(),
    };
    saved
        .save(&baselines.path().join("golden-pair.json"))
        .unwrap();
    Fixture {
        db,
        swarm,
        golden,
        baselines,
    }
}

impl Fixture {
    fn baselines(&self) -> Baselines {
        Baselines::new(Some(self.baselines.path().to_owned()))
    }
}

#[test]
fn the_results_ui_reads_what_the_runs_recorded() {
    let f = fixture();
    let conn = &f.db;
    let baselines = f.baselines();

    // Experiments: newest first by default, searchable, and flagged with their logs and
    // baselines.
    let listed = browse::experiments(&conn, &ExperimentQuery::default(), &baselines).unwrap();
    let ids: Vec<i64> = listed.iter().map(|e| e.id).collect();
    assert_eq!(ids, [2, 1]);
    let swarm = &listed[1];
    assert_eq!(
        (swarm.variants, swarm.seeds, swarm.runs, swarm.peers),
        (2, 6, 12, 8)
    );
    assert!(swarm.logged && !swarm.baseline);
    assert!(!listed[0].logged && listed[0].baseline);
    assert_eq!(swarm.created_at.len(), "2026-01-01 00:00:00".len());
    let found = browse::experiments(
        &conn,
        &ExperimentQuery {
            search: "SMALL CROWD".to_owned(),
            sort: ExperimentSort::Runs,
            descending: false,
        },
        &baselines,
    )
    .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, "swarm");

    let detail = browse::experiment(&conn, 1, &baselines).unwrap().unwrap();
    assert_eq!(detail.variants.len(), 2);
    assert_eq!(detail.variants[1].vary[0].key, "strategies.upload");
    assert_eq!(detail.variants[1].vary[0].value, "cooperative");
    assert_eq!(detail.variants[1].runs, 6);
    assert_eq!(detail.scenario["name"], "swarm");
    assert!(browse::experiment(&conn, 9, &baselines).unwrap().is_none());

    // Runs: filtered, sorted and paged.
    let all = browse::runs(&conn, &RunQuery::default()).unwrap();
    assert_eq!((all.total, all.runs.len()), (16, 16));
    let page = browse::runs(
        &conn,
        &RunQuery {
            experiment: Some(1),
            variant: Some(1),
            sort: RunSort::CompletionP50,
            descending: true,
            limit: 4,
            offset: 1,
            ..RunQuery::default()
        },
    )
    .unwrap();
    assert_eq!((page.total, page.runs.len()), (6, 4));
    assert!(
        page.runs
            .iter()
            .all(|r| r.variant == 1 && r.experiment == 1)
    );
    let p50: Vec<f64> = page
        .runs
        .iter()
        .filter_map(|r| r.completion_p50_s)
        .collect();
    assert!(p50.windows(2).all(|w| w[0] >= w[1]), "{p50:?}");
    let seeded = browse::runs(
        &conn,
        &RunQuery {
            seed: Some(2),
            search: "cooperative".to_owned(),
            ..RunQuery::default()
        },
    )
    .unwrap();
    assert_eq!(seeded.total, 1);
    assert_eq!(seeded.runs[0].variant_name, "strategies.upload=cooperative");
    for outcome in [Outcome::Complete, Outcome::Incomplete] {
        let runs = browse::runs(
            &conn,
            &RunQuery {
                outcome,
                ..RunQuery::default()
            },
        )
        .unwrap();
        assert!(
            runs.runs
                .iter()
                .all(|r| (r.completed == r.downloaders) == (outcome == Outcome::Complete))
        );
    }

    // A run's detail adds up to its headline numbers.
    let run = browse::run(&conn, 1).unwrap().unwrap();
    let result = &f.swarm.1[0];
    assert_eq!((run.run.seed, run.run.variant), (result.seed, 0));
    assert!(run.logged);
    let classes: Vec<&str> = run.classes.iter().map(|c| c.class.as_str()).collect();
    assert_eq!(classes, ["origin", "home", "free-rider"]);
    assert_eq!(run.classes[1].peers, 6);
    let completed: u32 = run
        .classes
        .iter()
        .filter(|c| c.starts == "empty")
        .map(|c| c.completed)
        .sum();
    assert_eq!(completed, run.run.completed);
    let downloaded: u64 = run.classes.iter().map(|c| c.downloaded).sum();
    let uploaded: u64 = run.classes.iter().map(|c| c.uploaded).sum();
    assert!(downloaded > 0 && uploaded <= run.run.payload_bytes);
    assert!(
        run.traffic.windows(2).all(|w| w[0].bytes >= w[1].bytes),
        "most bytes first"
    );
    let peers = browse::peers(&conn, 1).unwrap().unwrap();
    assert_eq!(peers.len(), 8);
    assert_eq!(peers[0].starts, "complete");
    assert!(browse::run(&conn, 99).unwrap().is_none());

    // Its time series: completions count up to the run's, and the Data sent per second adds
    // back up to the Data in its traffic.
    let series = browse::series(&conn, 1).unwrap().unwrap();
    assert!(!series.availability.is_empty());
    assert_eq!(
        series.completed.last().map(|c| c.1),
        Some(run.run.completed)
    );
    let data = run.traffic.iter().find(|t| t.message == "Data").unwrap();
    let sent: f64 = series.throughput.iter().map(|p| p.1 * series.bin_s).sum();
    assert!(
        (sent - data.bytes as f64).abs() < 1.0,
        "{sent} vs {}",
        data.bytes
    );
    let quiet = browse::series(&conn, 13).unwrap().unwrap();
    assert!(
        quiet.throughput.is_empty(),
        "experiment 2 kept no event log"
    );
}

#[test]
fn comparisons_match_the_statistics_sim_run_prints() {
    let f = fixture();
    let conn = &f.db;
    let (scenario, results) = &f.swarm;
    let query = CompareQuery::experiment(1, 2);
    let compared = browse::compare(&conn, &query).unwrap();
    assert_eq!(compared.variants[1].seeds, [1, 2, 3, 4, 5, 6]);
    assert_eq!(compared.confidence, 0.95);

    let expected = stats::compare(
        &stats::samples(scenario, results),
        0,
        Bootstrap::default(),
        Meaningful::default(),
    );
    assert_eq!(compared.metrics.len(), stats::Metric::ALL.len());
    for (row, rows) in compared.metrics.iter().zip(expected.chunks(2)) {
        assert_eq!(row.name, rows[0].metric.name());
        for (cell, want) in row.cells.iter().zip(rows) {
            let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
            match (cell.estimate, want.estimate) {
                (Some(got), Some(want)) => {
                    assert!(close(got.median, want.median), "{}", row.name);
                    assert!(close(got.low, want.interval.low), "{}", row.name);
                    assert!(close(got.high, want.interval.high), "{}", row.name);
                }
                (got, want) => assert_eq!(got.is_none(), want.is_none(), "{}", row.name),
            }
            assert_eq!(cell.difference.is_some(), want.difference.is_some());
        }
    }
    let completed = &compared.metrics[0];
    assert_eq!(completed.unit, MetricUnit::Fraction);
    assert!(completed.cells[0].difference.is_none(), "the reference");
    let verdict = completed.cells[1].difference.unwrap().verdict;
    assert_ne!(verdict, Verdict::TooFewPairs, "six paired seeds");

    // Across experiments: variants are named by experiment, and the reference may be any.
    let across = CompareQuery {
        variants: vec![
            VariantRef {
                experiment: 2,
                variant: 0,
            },
            VariantRef {
                experiment: 1,
                variant: 1,
            },
        ],
        reference: 1,
    };
    let compared = browse::compare(&conn, &across).unwrap();
    assert_eq!(compared.variants[0].experiment_name, "golden-pair");
    assert_eq!(compared.reference, 1);
    let missing = CompareQuery {
        variants: vec![VariantRef {
            experiment: 1,
            variant: 5,
        }],
        reference: 0,
    };
    assert!(matches!(
        browse::compare(&conn, &missing),
        Err(BrowseError::NotFound(_))
    ));

    // Over time: each band ends at the median of the runs' completed fractions.
    let series = browse::compare_series(&conn, &query).unwrap();
    let keys: Vec<&str> = series.series.iter().map(|s| s.key.as_str()).collect();
    assert_eq!(keys, ["completed", "rarest", "present"]);
    let completed = &series.series[0];
    for (variant, band) in completed.variants.iter().enumerate() {
        let fractions: Vec<f64> = results
            .iter()
            .filter(|r| r.variant == variant)
            .map(|r| r.completion_times().len() as f64 / r.downloaders() as f64)
            .collect();
        let last = band.last().unwrap();
        assert_eq!(last.median, stats::median(&fractions).unwrap());
        assert!(band.iter().all(|p| p.low <= p.median && p.median <= p.high));
        assert!(band.windows(2).all(|w| w[0].median <= w[1].median));
    }
}

#[test]
fn experiments_are_checked_against_their_golden_baseline() {
    let f = fixture();
    let conn = &f.db;
    let baselines = f.baselines();
    assert!(
        browse::baseline_report(&conn, 1, &baselines)
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        browse::baseline_report(&conn, 7, &baselines),
        Err(BrowseError::NotFound(_))
    ));

    // The same runs as the baseline: nothing regressed or improved.
    let report = browse::baseline_report(&conn, 2, &baselines)
        .unwrap()
        .unwrap();
    let checked = &report.variants[0];
    assert_eq!(
        checked.method,
        Method::Paired,
        "four seeds, more than a quick check"
    );
    assert_eq!(checked.seeds, [1, 2, 3, 4]);
    assert!(checked.missing.is_empty());
    assert!(checked.checks.iter().all(|c| !c.regressed && !c.improved));

    // A baseline that finished twice as fast, with fewer seeds: the runs regressed, and the
    // seeds it lacks are left out.
    let (scenario, results) = &f.golden;
    let mut faster: BTreeMap<u64, baseline::Metrics> = results
        .iter()
        .take(2)
        .map(|r| (r.seed, baseline::Metrics::of(r, scenario)))
        .collect();
    for m in faster.values_mut() {
        m.completion_p50_s = m.completion_p50_s.map(|s| s / 2.0);
    }
    Baseline {
        scenario: "golden-pair".to_owned(),
        scenario_sha256: Scenario::file_sha256(GOLDEN),
        runs: faster,
    }
    .save(&f.baselines.path().join("golden-pair.json"))
    .unwrap();
    let report = browse::baseline_report(&conn, 2, &baselines)
        .unwrap()
        .unwrap();
    let checked = &report.variants[0];
    assert_eq!(checked.method, Method::Mean);
    assert_eq!(checked.missing, [3, 4]);
    let p50 = checked
        .checks
        .iter()
        .find(|c| c.metric.starts_with("completion p50"))
        .unwrap();
    assert!(p50.regressed);

    let listed = browse::baselines(&conn, &baselines).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].seeds, [1, 2]);
    assert_eq!(listed[0].experiments.len(), 1);
    assert!(listed[0].experiments[0].regressed >= 1);
    assert!(
        browse::baselines(&conn, &Baselines::default())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn an_edited_scenario_is_not_checked_against_the_old_baseline() {
    let f = fixture();
    // The experiment came from the same file as the baseline, until the file changes.
    let experiment = |f: &Fixture| {
        browse::experiment(&f.db, 2, &f.baselines())
            .unwrap()
            .unwrap()
    };
    assert!(experiment(&f).experiment.baseline);
    let path = f.baselines.path().join("golden-pair.json");
    let mut saved = Baseline::load(&path).unwrap();
    saved.scenario_sha256 = Scenario::file_sha256(&format!("{GOLDEN}# edited\n"));
    saved.save(&path).unwrap();

    assert!(!experiment(&f).experiment.baseline);
    let listed = browse::experiments(&f.db, &ExperimentQuery::default(), &f.baselines()).unwrap();
    assert!(listed.iter().all(|e| !e.baseline));
    assert!(
        browse::baseline_report(&f.db, 2, &f.baselines())
            .unwrap()
            .is_none()
    );
    let outcome = &browse::baselines(&f.db, &f.baselines()).unwrap()[0].experiments[0];
    assert_eq!(outcome.experiment, 2);
    assert!(
        outcome
            .problem
            .as_deref()
            .unwrap()
            .contains("scenario file differs")
    );
}

#[test]
fn a_database_opens_read_only_beside_its_writer_closing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("results.duckdb");
    {
        let db = Database::open(&path).unwrap();
        record(&db, GOLDEN, false);
    }
    let first = Database::open_read_only(&path).unwrap();
    let second = Database::open_read_only(&path).unwrap();
    for db in [&first, &second] {
        let runs = browse::runs(&db, &RunQuery::default()).unwrap();
        assert_eq!(runs.total, 4);
    }
    assert!(Database::open_read_only(&dir.path().join("missing.duckdb")).is_err());
}
