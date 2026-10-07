//! The availability of chunks over time: how many present peers hold each chunk.

use tempest_sim::{RunOptions, RunResult, Scenario, run};

fn run_first(yaml: &str) -> RunResult {
    let scenario = Scenario::from_yaml(yaml).unwrap();
    run(&scenario, &scenario.variants()[0], 1, RunOptions::default()).unwrap()
}

#[test]
fn copies_only_grow_while_everyone_stays() {
    let result = run_first(
        "
name: spread
sample_interval: 200ms
bundle: { files: [{ size: 4MiB }] }
settings: { overrides: { lingering.keep_sharing: true } }
peers:
  - { class: origin, starts: complete, bandwidth: { up: 10Mbps, down: 10Mbps } }
  - { class: home, count: 8, bandwidth: { up: 5Mbps, down: 50Mbps } }
",
    );
    let samples = &result.availability;
    assert!(samples.len() > 10, "{}", samples.len());
    for sample in samples {
        assert_eq!(sample.peers, 9);
        assert!(
            sample.rarest >= 1,
            "the origin holds every chunk: {sample:?}"
        );
        assert!(f64::from(sample.rarest) <= sample.mean, "{sample:?}");
        assert!(sample.mean <= 9.0, "{sample:?}");
    }
    for pair in samples.windows(2) {
        assert!(pair[0].time < pair[1].time);
        assert!(pair[0].rarest <= pair[1].rarest && pair[0].mean <= pair[1].mean);
    }
    let last = samples.last().unwrap();
    assert!(last.mean > 8.0, "{last:?}");
    assert!(result.rarest_copies().unwrap() >= 1.0);
}

#[test]
fn copies_leave_with_their_peers() {
    let result = run_first(
        "
name: gone
duration: 10s
bundle: { files: [{ size: 1MiB }] }
sample_interval: 1s
peers:
  - class: origin
    starts: complete
    churn: { session: 2500ms, shape: 1000 }
  - class: late
    arrivals: { pattern: at-once, at: 5s }
",
    );
    let samples = &result.availability;
    assert_eq!(samples.len(), 10);
    assert_eq!((samples[0].peers, samples[0].rarest), (1, 1));
    assert_eq!(samples[0].mean, 1.0);
    let after = samples
        .iter()
        .find(|s| s.time.since_epoch().as_secs() == 3)
        .unwrap();
    assert_eq!((after.peers, after.rarest, after.mean), (0, 0, 0.0));
    let last = samples.last().unwrap();
    assert_eq!((last.peers, last.rarest), (1, 0));
    assert_eq!(result.peers[1].completed, None);
}
