//! Feeds a tracker's swarms arbitrary announce bodies from arbitrary addresses at arbitrary
//! times, as hostile clients might. The swarms must never panic, never list a peer to itself,
//! and must count their peers the way their stats endpoint reports them.

#![no_main]

use std::net::{IpAddr, Ipv6Addr};
use std::time::{Duration, Instant};

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use tempest_protocol::tracker::AnnounceRequest;
use tempest_tracker::{Swarms, TrackerConfig};

#[derive(Debug, Arbitrary)]
struct Announce {
    /// Seconds since the previous announce.
    wait: u16,
    ip: [u8; 16],
    body: Vec<u8>,
    sweep: bool,
}

fuzz_target!(|announces: Vec<Announce>| {
    let config = TrackerConfig {
        max_peers: 64,
        max_num_want: 16,
        ..TrackerConfig::default()
    };
    let mut swarms = Swarms::new(config, 0);
    let mut now = Instant::now();
    for announce in announces {
        now += Duration::from_secs(announce.wait.into());
        let Ok(request) = serde_json::from_slice::<AnnounceRequest>(&announce.body) else {
            continue;
        };
        let ip = IpAddr::V6(Ipv6Addr::from(announce.ip));
        if let Ok(response) = swarms.announce(now, &request, ip) {
            assert!(response.peers.len() <= 16);
            assert!(
                response
                    .peers
                    .iter()
                    .all(|entry| entry.peer != request.peer)
            );
            let stats = swarms.stats(now, &request.bundle);
            assert_eq!(
                (stats.complete, stats.incomplete),
                (response.complete, response.incomplete)
            );
        }
        assert!(swarms.listed() <= 64);
        if announce.sweep {
            swarms.sweep(now);
        }
    }
});
