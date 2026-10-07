//! Whole transfers between sessions, with every chunk verified against the manifest.

mod common;

use std::time::Duration;

use common::{CHUNK, Swarm, bundle, far_future};
use proptest::prelude::*;
use tempest_core::config::{ChunkSelection, Endgame, Upload, WhenComplete};
use tempest_core::content::MemoryContent;
use tempest_core::event::CloseCause;
use tempest_core::{Config, EventKind};
use tempest_protocol::message::GoodbyeReason;

fn assert_has_everything(swarm: &Swarm, node: usize, original: &MemoryContent) {
    let peer = &swarm.nodes[node];
    assert!(peer.session.is_complete(), "node {node} did not complete");
    assert_eq!(
        peer.content.files(),
        original.files(),
        "node {node} stored other bytes"
    );
    assert!(
        peer.completed_at().is_some(),
        "node {node} never reported completion"
    );
}

#[test]
fn two_peers_exchange_a_bundle() {
    // Odd sizes exercise file ends, padding and an empty file.
    let content = bundle(&[40_000, 0, 5 * CHUNK + 1, 33 * CHUNK]);
    let mut swarm = Swarm::new();
    let origin = swarm.add_complete(1, Config::default(), &content);
    let fetcher = swarm.add_empty(2, Config::default(), &content);
    swarm.introduce(fetcher, &[origin]);
    swarm.run(far_future());

    assert_has_everything(&swarm, fetcher, &content);
    let events = &swarm.nodes[fetcher].events;
    assert!(
        events
            .iter()
            .any(|e| matches!(e.kind, EventKind::ManifestReceived { .. }))
    );
    let requested = events
        .iter()
        .filter(|e| matches!(e.kind, EventKind::Requested { .. }))
        .count();
    // 3 + 2 + 6 + 33 chunks, mostly in requests of 16.
    assert!(requested < 20, "{requested} requests");
    let total = content.manifest().total_size();
    assert_eq!(swarm.nodes[fetcher].session.downloaded(), total);
    assert_eq!(swarm.nodes[origin].session.uploaded(), total);
}

#[test]
fn a_bundle_of_empty_files_completes_with_the_manifest() {
    let content = bundle(&[0, 0]);
    let mut swarm = Swarm::new();
    let origin = swarm.add_complete(1, Config::default(), &content);
    let fetcher = swarm.add_empty(2, Config::default(), &content);
    swarm.introduce(fetcher, &[origin]);
    swarm.run(far_future());
    assert_has_everything(&swarm, fetcher, &content);
}

#[test]
fn chunks_spread_through_a_swarm() {
    let content = bundle(&[200 * CHUNK + 7]);
    let mut swarm = Swarm::new();
    let mut config = Config::default();
    config.upload.parallel_responses = 2;
    let origin = swarm.add_complete(1, config.clone(), &content);
    let fetchers: Vec<usize> = (2..8)
        .map(|n| swarm.add_empty(n, config.clone(), &content))
        .collect();
    for &f in &fetchers {
        let mut others = fetchers.clone();
        others.retain(|&o| o != f);
        others.push(origin);
        swarm.introduce(f, &others);
    }
    swarm.run(far_future());

    let total = content.manifest().total_size();
    let mut from_fetchers = 0;
    for &f in &fetchers {
        assert_has_everything(&swarm, f, &content);
        let session = &swarm.nodes[f].session;
        assert_eq!(session.downloaded() - session.duplicate(), total);
        from_fetchers += session.uploaded();
    }
    assert!(from_fetchers > 0, "fetchers never traded with each other");
}

#[test]
fn bad_data_gets_a_peer_banned_and_the_transfer_still_completes() {
    let content = bundle(&[50 * CHUNK]);
    let mut swarm = Swarm::new();
    let liar = swarm.add_complete(1, Config::default(), &content);
    swarm.nodes[liar].corrupt = true;
    let honest = swarm.add_complete(3, Config::default(), &content);
    let fetcher = swarm.add_empty(2, Config::default(), &content);
    swarm.introduce(fetcher, &[liar, honest]);
    swarm.run(far_future());

    assert_has_everything(&swarm, fetcher, &content);
    let liar_id = swarm.nodes[liar].id;
    let events = &swarm.nodes[fetcher].events;
    assert!(
        events
            .iter()
            .any(|e| e.kind == EventKind::Banned { peer: liar_id })
    );
    assert!(events.iter().any(|e| matches!(
        &e.kind,
        EventKind::Closed {
            peer,
            cause: CloseCause::Local { reason: GoodbyeReason::BadData, .. },
            ..
        } if *peer == liar_id
    )));
}

#[test]
fn runs_replay_exactly() {
    let log = || {
        let content = bundle(&[100 * CHUNK, 3 * CHUNK]);
        let mut swarm = Swarm::new();
        let origin = swarm.add_complete(1, Config::default(), &content);
        swarm.nodes[origin].upload_rate = Some(2_000_000);
        let fetchers: Vec<usize> = (2..6)
            .map(|n| swarm.add_empty(n, Config::default(), &content))
            .collect();
        for (i, &f) in fetchers.iter().enumerate() {
            swarm.nodes[f].latency = Duration::from_millis(10 * i as u64 + 5);
            swarm.nodes[f].upload_rate = Some(500_000);
            let others: Vec<usize> = (0..swarm.nodes.len()).filter(|&o| o != f).collect();
            swarm.introduce(f, &others);
        }
        swarm.run(far_future());
        swarm
            .nodes
            .iter()
            .flat_map(|n| n.events.iter().map(|e| e.to_string()))
            .collect::<Vec<_>>()
    };
    let first = log();
    assert!(first.len() > 50);
    assert_eq!(first, log());
}

fn any_selection() -> impl Strategy<Value = ChunkSelection> {
    prop_oneof![
        Just(ChunkSelection::RarestFirst),
        Just(ChunkSelection::Random),
        Just(ChunkSelection::Sequential),
        Just(ChunkSelection::Streaming),
    ]
}

fn any_upload() -> impl Strategy<Value = Upload> {
    prop_oneof![
        Just(Upload::Bittorrent),
        Just(Upload::Propshare),
        Just(Upload::Fairtorrent),
        Just(Upload::Cooperative),
    ]
}

fn any_config() -> impl Strategy<Value = Config> {
    (
        (0..=6u32, 0..=6u32, 1..=8u32, 1..=4u32, 1..=8u32),
        (any_selection(), 0..8u32, any_upload(), any::<bool>()),
    )
        .prop_map(
            |(
                (request_log, max_log, in_flight, slots, max_open),
                (selection, random_first, upload, adaptive),
            )| {
                let mut config = Config::default();
                config.requests.chunks = 1 << request_log;
                config.requests.initial_in_flight = in_flight;
                config.requests.adaptive = adaptive;
                config.upload.max_request_chunks = 1 << max_log;
                config.upload.parallel_responses = slots;
                config.upload.max_open_requests = max_open;
                config.strategies.chunk_selection = selection;
                config.strategies.upload = upload;
                config.selection.random_first = random_first;
                config
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn every_swarm_completes_with_verified_bytes(
        sizes in proptest::collection::vec(0..20 * CHUNK, 1..4),
        configs in proptest::collection::vec(any_config(), 4),
        latencies in proptest::collection::vec(0..50u64, 4),
        fetchers in 1..4usize,
    ) {
        let content = bundle(&sizes);
        let mut swarm = Swarm::new();
        let origin = swarm.add_complete(1, configs[0].clone(), &content);
        let ids: Vec<usize> = (0..fetchers)
            .map(|i| swarm.add_empty(2 + i as u8, configs[1 + i].clone(), &content))
            .collect();
        for (node, &latency) in swarm.nodes.iter_mut().zip(&latencies) {
            node.latency = Duration::from_millis(latency);
        }
        for &f in &ids {
            let mut others: Vec<usize> = ids.iter().copied().filter(|&o| o != f).collect();
            others.push(origin);
            swarm.introduce(f, &others);
        }
        swarm.run(far_future());
        for &f in &ids {
            prop_assert!(swarm.nodes[f].session.is_complete());
            prop_assert_eq!(swarm.nodes[f].content.files(), content.files());
        }
    }
}

/// Every combination of strategies completes a swarm with verified bytes.
#[test]
fn every_strategy_combination_completes() {
    let content = bundle(&[37 * CHUNK + 5, 0, 30 * CHUNK]);
    let selections = [
        ChunkSelection::RarestFirst,
        ChunkSelection::Random,
        ChunkSelection::Sequential,
        ChunkSelection::Streaming,
    ];
    let uploads = [
        Upload::Bittorrent,
        Upload::Propshare,
        Upload::Fairtorrent,
        Upload::Cooperative,
    ];
    let endgames = [Endgame::Limited, Endgame::Classic, Endgame::Off];
    let completions = [WhenComplete::RoundRobin, WhenComplete::Unchanged];
    let mut swarms = 0;
    for selection in selections {
        for upload in uploads {
            for endgame in endgames {
                for when_complete in completions {
                    for superseed in [false, true] {
                        let mut config = Config::default();
                        config.strategies.chunk_selection = selection;
                        config.strategies.upload = upload;
                        config.strategies.endgame = endgame;
                        config.strategies.when_complete = when_complete;
                        config.strategies.superseed = superseed;
                        config.requests.chunks = 4;
                        let label = format!("{:?}", config.strategies);

                        let mut swarm = Swarm::new();
                        let origin = swarm.add_complete(1, config.clone(), &content);
                        swarm.nodes[origin].upload_rate = Some(400_000);
                        let fetchers: Vec<usize> = (2..7)
                            .map(|n| swarm.add_empty(n, config.clone(), &content))
                            .collect();
                        for (i, &f) in fetchers.iter().enumerate() {
                            swarm.nodes[f].latency = Duration::from_millis(3 * i as u64);
                            swarm.nodes[f].upload_rate = Some(200_000);
                        }
                        let mut everyone = fetchers.clone();
                        everyone.push(origin);
                        swarm.introduce_all(&everyone);
                        swarm.run(far_future());
                        for &f in &fetchers {
                            let node = &swarm.nodes[f];
                            assert!(node.session.is_complete(), "{label}: node {f}");
                            assert_eq!(node.content.files(), content.files(), "{label}");
                        }
                        swarms += 1;
                    }
                }
            }
        }
    }
    assert_eq!(swarms, 4 * 4 * 3 * 2 * 2);
}
