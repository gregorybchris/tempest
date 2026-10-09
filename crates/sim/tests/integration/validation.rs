//! Validation against the classic BitTorrent results (D-078): Bharambe et al. and Legout et al.
//! found rarest-first plus choking close to optimal. Each test runs a scenario in
//! `scenarios/validation/` with all its seeds and checks the qualitative result with the
//! statistics `tempest-sim run` reports (D-167). `specs/simulation.md` records the outcome.
//!
//! The tests take from a few seconds to half a minute each in release, so they are marked slow
//! and run in CI's simulation job.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Mutex;

use tempest_core::Time;
use tempest_sim::experiment::{Observer, run_all, seeds};
use tempest_sim::log::{Row, Sink};
use tempest_sim::scenario::Start;
use tempest_sim::stats::{self, Bootstrap, Difference, Meaningful, Metric, Samples, Verdict};
use tempest_sim::{RunOptions, RunResult, Scenario};

fn load(name: &str) -> Scenario {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "scenarios", "validation", name]
        .iter()
        .collect();
    Scenario::load(&path).unwrap()
}

fn run(scenario: &Scenario, observer: &dyn Observer) -> Vec<RunResult> {
    let threads = std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN);
    let options = RunOptions::default();
    run_all(scenario, &seeds(scenario), threads, options, observer).unwrap()
}

/// `other` − `reference`, paired by seed.
fn compare(reference: &BTreeMap<u64, f64>, other: &BTreeMap<u64, f64>) -> Difference {
    Bootstrap::default()
        .compare(reference, other, Meaningful::default())
        .unwrap()
}

/// Asserts that `other` is meaningfully above `reference`, or below if `above` is false.
fn assert_shift(reference: &BTreeMap<u64, f64>, other: &BTreeMap<u64, f64>, above: bool) {
    let d = compare(reference, other);
    assert_eq!(d.verdict, Verdict::Meaningful, "{d:?}");
    assert_eq!(d.estimate.median > 0.0, above, "{d:?}");
}

/// The values of `metric` in the variant named `name`, by seed.
fn metric<'a>(samples: &'a [Samples], name: &str, metric: Metric) -> &'a BTreeMap<u64, f64> {
    let variant = samples.iter().find(|s| s.name.ends_with(name)).unwrap();
    &variant.values[&metric]
}

/// One value per run of `variant`, by seed.
fn per_seed(
    results: &[RunResult],
    variant: usize,
    f: impl Fn(&RunResult) -> f64,
) -> BTreeMap<u64, f64> {
    results
        .iter()
        .filter(|r| r.variant == variant)
        .map(|r| (r.seed, f(r)))
        .collect()
}

fn median(values: &[f64]) -> f64 {
    stats::median(values).unwrap()
}

/// When the origin leaves early, rarest-first has already spread every chunk, while random and
/// sequential selection leave chunks that nobody holds (Legout: rarest-first "prevents the
/// reappearance of rare pieces"; Bharambe: it avoids the last-block problem).
#[test]
#[ignore = "slow: a few seconds in release; CI runs it"]
fn rarest_first_keeps_every_chunk_when_the_origin_leaves() {
    let scenario = load("origin-leaves.yaml");
    let results = run(&scenario, &());
    let samples = stats::samples(&scenario, &results);
    let completed = |name| metric(&samples, name, Metric::Completed);
    let rarest = |name| metric(&samples, name, Metric::RarestCopies);

    // Whether everyone completes is all or nothing in a run, so the medians compare the
    // availability of the rarest chunk, and completions are counted.
    let total = |name| completed(name).values().sum::<f64>();
    assert!(completed("rarest-first").values().all(|&c| c == 1.0));
    assert!(total("=random") < total("rarest-first") - 1.0);
    assert_eq!(total("sequential"), 0.0);
    assert_shift(rarest("rarest-first"), rarest("=random"), false);
    assert_shift(rarest("rarest-first"), rarest("sequential"), false);
}

/// A crowd that arrives at once finishes close to the fastest possible time, set by the total
/// upload capacity, and faster with rarest-first than with random selection (Bharambe et al.).
#[test]
#[ignore = "slow: about 15 s in release; CI runs it"]
fn a_flash_crowd_finishes_near_the_bound() {
    let scenario = load("flash-crowd.yaml");
    let results = run(&scenario, &());

    // Every downloader needs the bundle, and all of it must be uploaded by someone.
    let size = scenario.bundle.build().unwrap().total_size() as f64;
    let downloaders = results[0].downloaders() as f64;
    let capacity: f64 = scenario
        .peers
        .iter()
        .map(|c| f64::from(c.count) * c.bandwidth.up.get())
        .sum();
    let bound = downloaders * size / capacity;
    let ends: Vec<f64> = per_seed(&results, 0, |r| r.end.since_epoch().as_secs_f64())
        .into_values()
        .collect();
    let end = median(&ends);
    assert!(
        end < 1.3 * bound,
        "{end:.1}s against a bound of {bound:.1}s"
    );

    let samples = stats::samples(&scenario, &results);
    let utilization = metric(&samples, "rarest-first", Metric::Utilization);
    assert!(utilization.values().all(|&u| u > 0.75), "{utilization:?}");
    let p90 = |name| metric(&samples, name, Metric::CompletionP90);
    assert_shift(p90("rarest-first"), p90("random"), true);
    let rarest = |name| metric(&samples, name, Metric::RarestCopies);
    assert_shift(rarest("rarest-first"), rarest("random"), false);
}

/// The median completion time of each class of peers in a run, by class name.
fn class_times(scenario: &Scenario, result: &RunResult) -> BTreeMap<String, f64> {
    let mut times: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for peer in &result.peers {
        if let (Some(arrived), Some(completed)) = (peer.arrived, peer.completed) {
            let class = scenario.peers[peer.class].class.clone();
            let time = completed.since(arrived).as_secs_f64();
            times.entry(class).or_default().push(time);
        }
    }
    times
        .into_iter()
        .map(|(class, t)| (class, median(&t)))
        .collect()
}

/// The median upload/download ratio of the peers of class `class` in a run.
fn share_ratio(scenario: &Scenario, result: &RunResult, class: &str) -> f64 {
    let ratios: Vec<f64> = result
        .peers
        .iter()
        .filter(|p| scenario.peers[p.class].class == class && p.downloaded > 0)
        .map(|p| p.uploaded as f64 / p.downloaded as f64)
        .collect();
    median(&ratios)
}

/// Choking serves the peers that upload, so free riders take longer than contributors, which
/// they don't under a strategy that serves everyone (Legout: the choke algorithm "is robust to
/// free riders"). Fast peers give more than they get back (Bharambe: tit-for-tat doesn't make
/// heterogeneous peers fair).
#[test]
#[ignore = "slow: about 10 s in release; CI runs it"]
fn choking_slows_free_riders() {
    let scenario = load("free-riders.yaml");
    let results = run(&scenario, &());
    let names: Vec<String> = scenario.variants().iter().map(|v| v.name()).collect();
    let variant = |name: &str| names.iter().position(|n| n.ends_with(name)).unwrap();
    let penalty = |v| {
        per_seed(&results, v, |r| {
            let times = class_times(&scenario, r);
            times["free-rider"] / times["home"]
        })
    };
    let (bittorrent, cooperative) = (variant("bittorrent"), variant("cooperative"));
    assert_shift(&penalty(cooperative), &penalty(bittorrent), true);
    let penalties: Vec<f64> = penalty(bittorrent).into_values().collect();
    assert!(median(&penalties) > 1.05, "{penalties:?}");

    let ratio = |class| per_seed(&results, bittorrent, |r| share_ratio(&scenario, r, class));
    assert!(
        ratio("fast").values().all(|&r| r > 1.5),
        "{:?}",
        ratio("fast")
    );
    assert!(
        ratio("home").values().all(|&r| r < 1.2),
        "{:?}",
        ratio("home")
    );
}

/// A peer receiving verified payload: the peer, when, and how many bytes.
type Receipt = (u32, Time, u64);

/// Collects each run's receipts, by seed.
#[derive(Default)]
struct Receipts(Mutex<BTreeMap<u64, Vec<Receipt>>>);

struct ReceiptSink<'a> {
    receipts: &'a Receipts,
    seed: u64,
    rows: Vec<Receipt>,
}

impl Sink for ReceiptSink<'_> {
    fn row(&mut self, row: &Row) {
        if row.kind == "received" {
            self.rows.push((row.peer, row.time, row.bytes.unwrap_or(0)));
        }
    }
}

impl Drop for ReceiptSink<'_> {
    fn drop(&mut self) {
        let rows = std::mem::take(&mut self.rows);
        self.receipts.0.lock().unwrap().insert(self.seed, rows);
    }
}

impl Observer for Receipts {
    fn sink(&self, _variant: usize, seed: u64) -> Option<Box<dyn Sink + '_>> {
        Some(Box::new(ReceiptSink {
            receipts: self,
            seed,
            rows: Vec::new(),
        }))
    }
}

/// How long the median downloader took for its first 5% of the bundle, for each 5% in the
/// middle, and for its last 5%, in seconds.
fn phases(result: &RunResult, receipts: &[Receipt], size: u64) -> [f64; 3] {
    let mut phases = [Vec::new(), Vec::new(), Vec::new()];
    for (index, peer) in result.peers.iter().enumerate() {
        let (Some(arrived), Some(_)) = (peer.arrived, peer.completed) else {
            continue;
        };
        if peer.starts != Start::Empty {
            continue;
        }
        let (mut total, mut first, mut last) = (0, None, None);
        let mut done = arrived;
        for &(_, time, bytes) in receipts.iter().filter(|r| r.0 as usize == index) {
            total += bytes;
            if first.is_none() && total * 20 >= size {
                first = Some(time);
            }
            if last.is_none() && total * 20 >= size * 19 {
                last = Some(time);
            }
            done = time;
        }
        let (first, last) = (first.unwrap(), last.unwrap());
        phases[0].push(first.since(arrived).as_secs_f64());
        phases[1].push(last.since(first).as_secs_f64() / 18.0);
        phases[2].push(done.since(last).as_secs_f64());
    }
    phases.map(|p| median(&p))
}

/// A newcomer has nothing to trade, so its first chunks come slowly; rarest-first and endgame
/// keep the last ones from lagging as much (Legout: "the last pieces problem is overestimated",
/// while the "first blocks problem" is real).
#[test]
#[ignore = "slow: about 10 s in release; CI runs it"]
fn first_chunks_come_slower_than_last_ones() {
    let scenario = load("first-and-last.yaml");
    let receipts = Receipts::default();
    let results = run(&scenario, &receipts);
    let size = scenario.bundle.build().unwrap().total_size();
    let receipts = receipts.0.into_inner().unwrap();
    let by_seed: BTreeMap<u64, [f64; 3]> = results
        .iter()
        .map(|r| (r.seed, phases(r, &receipts[&r.seed], size)))
        .collect();
    let phase = |i: usize| -> BTreeMap<u64, f64> {
        by_seed.iter().map(|(&seed, p)| (seed, p[i])).collect()
    };
    let (first, middle, last) = (phase(0), phase(1), phase(2));
    assert_shift(&middle, &first, true);
    assert_shift(&last, &first, true);
}
