//! Whole runs: transfers complete with verified bytes, and runs replay exactly.

use std::num::NonZeroUsize;
use std::path::Path;
use std::time::Duration;

use tempest_core::EventKind;
use tempest_sim::experiment::run_all;
use tempest_sim::log::Record;
use tempest_sim::scenario::Start;
use tempest_sim::{RunOptions, RunResult, Scenario, run};

fn scenario(yaml: &str) -> Scenario {
    Scenario::from_yaml(yaml).unwrap()
}

fn run_seed(scenario: &Scenario, seed: u64, keep_log: bool) -> RunResult {
    let variant = &scenario.variants()[0];
    run(scenario, variant, seed, RunOptions { keep_log }).unwrap()
}

fn example(name: &str) -> Scenario {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("scenarios")
        .join(name);
    Scenario::load(&path).unwrap()
}

/// Every peer that started without the bundle completed.
fn all_completed(result: &RunResult) -> bool {
    result
        .peers
        .iter()
        .filter(|p| p.starts == Start::Empty)
        .all(|p| p.completed.is_some())
}

#[test]
fn two_peers_exchange_real_verified_bytes() {
    let scenario = example("two-peers.yaml");
    for seed in 1..=3 {
        let result = run_seed(&scenario, seed, true);
        let downloader = &result.peers[1];
        assert!(downloader.completed.is_some(), "seed {seed}");
        assert_eq!(downloader.intact, Some(true), "the bytes match the bundle");
        assert_eq!(downloader.bad, 0);
        let total = 3 * 1024 * 1024 + 2 * 100 * 1024;
        assert_eq!(downloader.downloaded, total);
        assert_eq!(result.peers[0].uploaded, total);
        // 3.2 MiB at 20 Mbps takes about 1.4 s, plus the manifest and round trips.
        let took = downloader.completed.unwrap().since_epoch();
        assert!(
            took > Duration::from_millis(1300) && took < Duration::from_secs(3),
            "{took:?}"
        );
        // Every chunk arrived as verified Data.
        let received: u64 = result
            .log
            .iter()
            .filter(|e| e.peer == 1)
            .filter_map(|e| match &e.record {
                Record::Decision(EventKind::Received { bytes, .. }) => Some(*bytes),
                _ => None,
            })
            .sum();
        assert_eq!(received, total);
    }
}

#[test]
fn virtual_payloads_give_the_same_transfer() {
    let real = example("two-peers.yaml");
    let mut virtual_ = real.clone();
    virtual_.payload = tempest_sim::scenario::Payload::Virtual;
    let a = run_seed(&real, 1, true);
    let b = run_seed(&virtual_, 1, true);
    // The manifests differ only in their roots and creator, so the same requests go out.
    let requests = |r: &RunResult| {
        r.log
            .iter()
            .filter_map(|e| match &e.record {
                Record::Decision(EventKind::Requested { start, count, .. }) => {
                    Some((*start, *count))
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(requests(&a), requests(&b));
    assert_eq!(a.log.len(), b.log.len());
    assert_eq!(a.peers[1].downloaded, b.peers[1].downloaded);
    assert_eq!(b.peers[1].intact, None);
}

const BUSY: &str = "
name: busy
duration: 30m
bundle:
  files: [{ size: 4MiB }, { size: 300KiB }]
network: { latency: 15ms, loss: 1% }
tracker: { peers: 8, interval: 1m }
peers:
  - class: origin
    starts: complete
    bandwidth: { up: 8Mbps, down: 50Mbps }
  - class: home
    count: 24
    bandwidth: { up: 4Mbps, down: 30Mbps }
    latency: 40ms
    arrivals: { pattern: flash-crowd, decay: 20s }
  - class: free-rider
    count: 3
    behavior: free-rider
    bandwidth: { up: 4Mbps, down: 30Mbps }
  - class: bad
    count: 2
    behavior: bad-data
    starts: complete
    bandwidth: { up: 8Mbps, down: 8Mbps }
  - class: leaver
    count: 4
    bandwidth: { up: 4Mbps, down: 30Mbps }
    arrivals: { pattern: steady, start: 5s, interval: 3s }
    churn: { session: 20s }
";

#[test]
fn the_same_seed_replays_the_same_log() {
    let scenario = scenario(BUSY);
    let a = run_seed(&scenario, 7, true);
    let b = run_seed(&scenario, 7, true);
    assert_eq!(a.digest, b.digest);
    assert_eq!(a.log, b.log);
    assert_eq!(a.peers, b.peers);
    assert!(
        a.log.len() > 10_000,
        "the run is busy: {} entries",
        a.log.len()
    );

    let c = run_seed(&scenario, 8, false);
    assert_ne!(a.digest, c.digest, "another seed gives another run");
}

#[test]
fn a_busy_swarm_completes_despite_misbehaving_peers() {
    let scenario = scenario(BUSY);
    let result = run_seed(&scenario, 3, true);
    let class = |p: &tempest_sim::PeerOutcome| scenario.peers[p.class].class.as_str();
    for peer in &result.peers {
        match class(peer) {
            "home" => {
                assert!(peer.completed.is_some(), "every normal peer completes");
                assert!(peer.downloaded >= scenario.bundle.build().unwrap().total_size());
            }
            "leaver" => assert!(peer.left.is_some(), "every leaver leaves"),
            "free-rider" => assert_eq!(peer.uploaded, 0, "free riders never upload"),
            _ => {}
        }
    }
    // Bad-data peers were caught and banned.
    let bans = result
        .log
        .iter()
        .filter(|e| matches!(e.record, Record::Decision(EventKind::Banned { .. })))
        .count();
    assert!(bans > 0);
    let bad: u64 = result.peers.iter().map(|p| p.bad).sum();
    assert!(bad > 0);
    // The run ended once the last peer completed or left, before the time limit.
    assert!(result.end < tempest_core::Time::ZERO + scenario.duration);
    assert!(all_completed(&result) || result.peers.iter().any(|p| p.left.is_some()));
}

#[test]
fn variants_differ_and_threads_dont_matter() {
    let scenario = scenario(&format!(
        "{BUSY}seeds: 2\nexperiment:\n  vary:\n    strategies.chunk_selection: [rarest-first, random]\n"
    ));
    let seeds = [1, 2];
    let one = run_all(
        &scenario,
        &seeds,
        NonZeroUsize::new(1).unwrap(),
        RunOptions::default(),
        &(),
    )
    .unwrap();
    let four = run_all(
        &scenario,
        &seeds,
        NonZeroUsize::new(4).unwrap(),
        RunOptions::default(),
        &(),
    )
    .unwrap();
    let key = |r: &RunResult| (r.variant, r.seed, r.digest.clone());
    assert_eq!(one.len(), 4);
    assert_eq!(
        one.iter().map(key).collect::<Vec<_>>(),
        four.iter().map(key).collect::<Vec<_>>()
    );
    assert_ne!(one[0].digest, one[2].digest, "the variants differ");
    // Paired seeds: both variants see the same arrivals.
    let arrivals = |r: &RunResult| r.peers.iter().map(|p| p.arrived).collect::<Vec<_>>();
    assert_eq!(arrivals(&one[0]), arrivals(&one[2]));
}

#[test]
fn a_run_stops_at_its_duration() {
    let scenario = scenario(
        "
name: slow
duration: 10s
bundle: { files: [{ size: 100MiB }] }
peers:
  - { class: origin, starts: complete, bandwidth: { up: 1Mbps } }
  - { class: downloader }
",
    );
    let result = run_seed(&scenario, 1, false);
    assert_eq!(result.end.since_epoch(), Duration::from_secs(10));
    assert_eq!(result.peers[1].completed, None);
    let downloaded = result.peers[1].downloaded;
    // At most 10 s of 1 Mbps arrived.
    assert!(downloaded > 0 && downloaded <= 1_250_000, "{downloaded}");
}

#[test]
fn nobody_to_wait_for_ends_at_once() {
    let scenario = scenario(
        "
name: seeds-only
bundle: { files: [{ size: 1MiB }] }
peers: [{ class: origin, count: 3, starts: complete }]
",
    );
    let result = run_seed(&scenario, 1, false);
    assert_eq!(result.end, tempest_core::Time::ZERO);
}

/// Loads every scenario file under `dir`, returning how many there were.
fn load_all(dir: &Path) -> usize {
    let mut count = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            count += load_all(&path);
        } else {
            Scenario::load(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            count += 1;
        }
    }
    count
}

#[test]
fn example_scenarios_load() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios");
    assert!(load_all(&dir) >= 3);
}
