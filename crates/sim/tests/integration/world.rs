//! The world beyond bandwidth and latency (D-070): routers and relays, disk and hashing speed,
//! session lengths and arrival processes.

use std::time::Duration;

use tempest_core::EventKind;
use tempest_core::io::PathKind;
use tempest_sim::log::Record;
use tempest_sim::{RunOptions, RunResult, Scenario, run};

fn run_seed(yaml: &str, seed: u64) -> RunResult {
    let scenario = Scenario::from_yaml(yaml).unwrap();
    let variant = &scenario.variants()[0];
    run(&scenario, variant, seed, RunOptions { keep_log: true }).unwrap()
}

/// An origin and one downloader on fast links, with the given extra lines for each.
fn pair(origin: &str, downloader: &str, size: &str) -> String {
    format!(
        "
name: pair
duration: 1m
bundle: {{ files: [{{ size: {size} }}] }}
network: {{ latency: 10ms }}
relay: {{ latency: 20ms, bandwidth: 2Mbps }}
peers:
  - class: origin
    starts: complete
    bandwidth: {{ up: 100Mbps, down: 100Mbps }}
    {origin}
  - class: downloader
    bandwidth: {{ up: 100Mbps, down: 100Mbps }}
    {downloader}
"
    )
}

fn completion(result: &RunResult) -> Option<Duration> {
    result.peers[1].completed.map(|t| t.since_epoch())
}

fn upgrades(result: &RunResult) -> usize {
    result
        .log
        .iter()
        .filter(|e| matches!(e.record, Record::Upgraded { .. }))
        .count()
}

fn paths(result: &RunResult) -> Vec<PathKind> {
    result
        .log
        .iter()
        .filter_map(|e| match e.record {
            Record::Decision(EventKind::Connected { path, .. }) => Some(path),
            _ => None,
        })
        .collect()
}

#[test]
fn peers_behind_routers_without_relays_can_only_dial_out() {
    let hidden = "nat: { relay: false }";
    // The downloader dials the origin, which is reachable.
    let result = run_seed(&pair("", hidden, "1MiB"), 1);
    assert!(completion(&result).is_some());
    assert!(paths(&result).iter().all(|&p| p == PathKind::Direct));

    // Nobody can dial the origin.
    let result = run_seed(&pair(hidden, "", "1MiB"), 1);
    assert_eq!(completion(&result), None);
    let failed = result
        .log
        .iter()
        .any(|e| matches!(e.record, Record::Decision(EventKind::DialFailed { .. })));
    assert!(failed);
}

#[test]
fn relayed_connections_are_slow_until_a_hole_punch_works() {
    // 1 MiB through a 2 Mbps relay takes over 4 s.
    let stuck = "nat: { punchable: 0% }";
    let result = run_seed(&pair(stuck, stuck, "1MiB"), 1);
    let took = completion(&result).unwrap();
    assert!(took > Duration::from_secs(4), "{took:?}");
    assert!(paths(&result).iter().all(|&p| p == PathKind::Relayed));
    assert_eq!(upgrades(&result), 0);

    // A directly reachable downloader is dialed back: the connection goes direct.
    let result = run_seed(&pair(stuck, "", "1MiB"), 1);
    let took = completion(&result).unwrap();
    assert!(took < Duration::from_secs(1), "{took:?}");
    assert_eq!(upgrades(&result), 2, "both ends see the upgrade");

    // Two punchable routers punch too.
    let easy = "nat: { punchable: 100% }";
    let result = run_seed(&pair(easy, easy, "1MiB"), 1);
    assert!(completion(&result).unwrap() < Duration::from_secs(1));
    assert_eq!(upgrades(&result), 2);
}

#[test]
fn the_relay_detour_adds_latency() {
    let direct = run_seed(&pair("", "", "16KiB"), 1);
    let relayed = run_seed(&pair("nat: { punchable: 0% }", "", "16KiB"), 1);
    // The first round trips go through the relay, 2 × 20 ms further each way.
    let first_byte = |r: &RunResult| r.peers[1].first_byte.unwrap().since_epoch();
    assert!(first_byte(&relayed) >= first_byte(&direct) + Duration::from_millis(160));
}

#[test]
fn slow_disks_and_hashing_slow_downloads() {
    let fast = completion(&run_seed(&pair("", "", "4MiB"), 1)).unwrap();
    assert!(fast < Duration::from_secs(1), "{fast:?}");
    // 4 MiB written at 1 MB/s takes over 4 s; hashed at 2 MB/s, over 2 s.
    let disk = completion(&run_seed(&pair("", "disk: 1MB/s", "4MiB"), 1)).unwrap();
    assert!(disk > Duration::from_secs(4), "{disk:?}");
    let hashing = completion(&run_seed(&pair("", "hashing: 2MB/s", "4MiB"), 1)).unwrap();
    assert!(hashing > Duration::from_secs(2), "{hashing:?}");
    assert!(hashing < disk);
    // Hashing and writing overlap, so the slower one sets the pace.
    let both = pair("", "disk: 1MB/s\n    hashing: 2MB/s", "4MiB");
    let both = completion(&run_seed(&both, 1)).unwrap();
    assert!(
        both > disk && both < disk + Duration::from_millis(600),
        "{both:?}"
    );
}

/// `count` peers who can reach nobody, so that only the world's draws matter.
fn crowd(count: u32, duration: &str, lines: &str) -> String {
    format!(
        "
name: crowd
duration: {duration}
bundle: {{ files: [{{ size: 1MiB }}] }}
tracker: {{ peers: 0 }}
peers:
  - class: crowd
    count: {count}
    nat: {{ relay: false }}
    {lines}
"
    )
}

fn sorted_secs(mut values: Vec<f64>) -> Vec<f64> {
    values.sort_by(f64::total_cmp);
    values
}

#[test]
fn weibull_sessions_have_heavy_tails() {
    let result = run_seed(&crowd(2000, "3h", "churn: { session: 1m, shape: 0.5 }"), 1);
    let sessions = sorted_secs(
        result
            .peers
            .iter()
            .map(|p| p.left.unwrap().since(p.arrived.unwrap()).as_secs_f64())
            .collect(),
    );
    let mean = sessions.iter().sum::<f64>() / sessions.len() as f64;
    let median = sessions[sessions.len() / 2];
    assert!((mean / 60.0 - 1.0).abs() < 0.2, "mean {mean}");
    // For shape 0.5 the median is about a quarter of the mean; exponential's is 69%.
    assert!(median < 0.4 * mean, "median {median}, mean {mean}");
}

#[test]
fn poisson_arrivals_come_at_the_mean_rate() {
    let result = run_seed(
        &crowd(
            1000,
            "1h",
            "arrivals: { pattern: poisson, start: 10s, interval: 2s }",
        ),
        1,
    );
    let arrivals = sorted_secs(
        result
            .peers
            .iter()
            .map(|p| p.arrived.unwrap().since_epoch().as_secs_f64())
            .collect(),
    );
    assert!(arrivals[0] > 10.0);
    let gaps: Vec<f64> = arrivals.windows(2).map(|w| w[1] - w[0]).collect();
    let mean = gaps.iter().sum::<f64>() / gaps.len() as f64;
    assert!((mean / 2.0 - 1.0).abs() < 0.1, "mean gap {mean}");
    // Exponential gaps: about 39% are shorter than half the mean.
    let short = gaps.iter().filter(|&&g| g < 1.0).count() as f64 / gaps.len() as f64;
    assert!((short - 0.39).abs() < 0.05, "{short}");
}

#[test]
fn routers_are_paired_across_variants_and_replay() {
    let yaml = "
name: mixed
duration: 10m
seeds: 2
bundle: { files: [{ size: 2MiB }] }
tracker: { peers: 10 }
peers:
  - { class: origin, starts: complete, bandwidth: { up: 20Mbps } }
  - class: home
    count: 20
    bandwidth: { up: 5Mbps, down: 50Mbps }
    nat: { punchable: 50% }
    disk: 4MB/s
    arrivals: { pattern: poisson, interval: 1s }
    churn: { session: 5m, shape: 0.7 }
experiment:
  vary:
    strategies.chunk_selection: [rarest-first, random]
";
    let scenario = Scenario::from_yaml(yaml).unwrap();
    let variants = scenario.variants();
    let go = |v: usize| run(&scenario, &variants[v], 2, RunOptions { keep_log: true }).unwrap();
    let (a, b) = (go(0), go(0));
    assert_eq!(a.digest, b.digest);
    assert_eq!(a.log, b.log);
    assert!(upgrades(&a) > 0, "some pairs punch");
    assert!(paths(&a).contains(&PathKind::Relayed));
    // The other variant sees the same arrivals.
    let c = go(1);
    assert_ne!(a.digest, c.digest);
    let arrivals = |r: &RunResult| r.peers.iter().map(|p| p.arrived).collect::<Vec<_>>();
    assert_eq!(arrivals(&a), arrivals(&c));
}

/// Varying one class's count and settings changes that class alone (D-230): the other class's
/// peers arrive as they did.
#[test]
fn varied_classes_keep_the_others_paired() {
    let yaml = "
name: classes
duration: 10m
bundle: { files: [{ size: 1MiB }] }
peers:
  - { class: origin, starts: complete, bandwidth: { up: 20Mbps } }
  - class: early
    count: 2
    arrivals: { pattern: poisson, interval: 5s }
  - class: late
    count: 5
    arrivals: { pattern: poisson, interval: 5s }
experiment:
  vary:
    peers.early.count: [2, 6]
    peers.origin.settings.strategies.superseed: [off, on]
";
    let scenario = Scenario::from_yaml(yaml).unwrap();
    let variants = scenario.variants();
    let results: Vec<RunResult> = variants
        .iter()
        .map(|v| run(&scenario, v, 3, RunOptions { keep_log: true }).unwrap())
        .collect();
    let count = |r: &RunResult, class: usize| r.peers.iter().filter(|p| p.class == class).count();
    assert_eq!(count(&results[0], 1), 2);
    assert_eq!(count(&results[2], 1), 6);
    let late = |r: &RunResult| {
        r.peers
            .iter()
            .filter(|p| p.class == 2)
            .map(|p| p.arrived)
            .collect::<Vec<_>>()
    };
    for result in &results[1..] {
        assert_eq!(late(result), late(&results[0]));
    }
    let superseeded = |r: &RunResult| {
        r.log
            .iter()
            .any(|e| matches!(e.record, Record::Decision(EventKind::Offered { .. })))
    };
    assert!(!superseeded(&results[0]));
    assert!(superseeded(&results[1]));
}
