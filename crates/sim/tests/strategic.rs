//! Strategic (BitTyrant-style) clients among BitTorrent-strategy peers (D-166).

use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use tempest_sim::experiment::run_all;
use tempest_sim::stats::{Bootstrap, Meaningful, Verdict};
use tempest_sim::{RunOptions, Scenario};

/// A swarm with a few fast peers, many home peers and some slow ones, where peers leave once
/// complete. The four `probe` peers, home peers otherwise, behave as `behavior`.
fn swarm(behavior: &str) -> Scenario {
    Scenario::from_yaml(&format!(
        "
name: strategic
bundle: {{ files: [{{ size: 256MiB }}] }}
network: {{ latency: 30ms }}
tracker: {{ peers: 30, interval: 2m }}
settings: {{ overrides: {{ lingering.ratio: 0.01 }} }}
peers:
  - class: origin
    starts: complete
    bandwidth: {{ up: 100Mbps, down: 100Mbps }}
  - class: fast
    count: 4
    bandwidth: {{ up: 50Mbps, down: 100Mbps }}
    arrivals: {{ pattern: flash-crowd, decay: 10s }}
  - class: home
    count: 30
    bandwidth: {{ up: 5Mbps, down: 50Mbps }}
    arrivals: {{ pattern: flash-crowd, decay: 10s }}
  - class: slow
    count: 10
    bandwidth: {{ up: 1Mbps, down: 20Mbps }}
    arrivals: {{ pattern: flash-crowd, decay: 10s }}
  - class: probe
    count: 6
    behavior: {behavior}
    bandwidth: {{ up: 5Mbps, down: 50Mbps }}
    arrivals: {{ pattern: flash-crowd, decay: 10s }}
"
    ))
    .unwrap()
}

/// The mean completion time of the probes in each seed, in seconds.
fn probe_times(behavior: &str, seeds: u64) -> BTreeMap<u64, f64> {
    let scenario = swarm(behavior);
    let probe = scenario.peers.len() - 1;
    let seeds: Vec<u64> = (1..=seeds).collect();
    let threads = std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN);
    run_all(&scenario, &seeds, threads, RunOptions::default(), &())
        .unwrap()
        .iter()
        .map(|result| {
            let times: Vec<f64> = result
                .peers
                .iter()
                .filter(|p| p.class == probe)
                .map(|p| p.completed.unwrap().since(p.arrived.unwrap()).as_secs_f64())
                .collect();
            (result.seed, times.iter().sum::<f64>() / times.len() as f64)
        })
        .collect()
}

/// Piatek et al. found BitTyrant downloads faster than BitTorrent in swarms with uneven upload
/// capacity. Here it gets more from the fast peers while giving them less.
#[test]
#[ignore = "slow: about 15 s in release; CI runs it"]
fn strategic_clients_download_faster_among_bittorrent_peers() {
    let normal = probe_times("normal", 10);
    let strategic = probe_times("strategic", 10);
    let difference = Bootstrap::default()
        .compare(&normal, &strategic, Meaningful::default())
        .unwrap();
    assert_eq!(difference.verdict, Verdict::Meaningful, "{difference:?}");
    assert!(difference.estimate.median < 0.0, "{difference:?}");
}
