//! Swarms that show each strategy doing its job: endgame, pipelining, superseeding, lingering,
//! peer exchange, the connection limit, snubbing, streaming and reciprocity.

mod common;

use std::time::Duration;

use common::{CHUNK, Swarm, at, bundle, far_future};
use tempest_core::config::{ChunkSelection, Endgame, LingerSettings, WhenComplete};
use tempest_core::event::{
    CancelReason, CloseCause, InFlightReason, SelectReason, ServeReason, StopReason, SuperseedEnd,
};
use tempest_core::upload::{ServeDecision, UploadContext, UploadStrategy};
use tempest_core::{Config, EventKind, Rng, Time};
use tempest_protocol::message::GoodbyeReason;

/// A fetcher between a fast and a very slow provider, with the given endgame.
fn slow_provider_swarm(endgame: Endgame) -> (Swarm, usize) {
    let content = bundle(&[64 * CHUNK]);
    let mut config = Config::default();
    config.requests.chunks = 4;
    config.strategies.endgame = endgame;
    let mut swarm = Swarm::new();
    let fast = swarm.add_complete(1, config.clone(), &content);
    let slow = swarm.add_complete(2, config.clone(), &content);
    let fetcher = swarm.add_empty(3, config, &content);
    swarm.nodes[fast].upload_rate = Some(10_000_000);
    swarm.nodes[slow].upload_rate = Some(20_000);
    swarm.nodes[fetcher].latency = Duration::from_millis(10);
    swarm.introduce(fetcher, &[fast, slow]);
    swarm.run(far_future());
    assert_eq!(swarm.nodes[fetcher].content.files(), content.files());
    (swarm, fetcher)
}

#[test]
fn endgame_does_not_wait_for_a_slow_provider() {
    let (swarm, fetcher) = slow_provider_swarm(Endgame::Off);
    let without = swarm.nodes[fetcher].completed_at().unwrap();
    // The slow provider's four requests of 64 KiB take about 13 s at 20 kB/s.
    assert!(without > at(5.0), "finished at {without}");

    for endgame in [Endgame::Limited, Endgame::Classic] {
        let (swarm, fetcher) = slow_provider_swarm(endgame);
        let node = &swarm.nodes[fetcher];
        let with = node.completed_at().unwrap();
        assert!(with < at(1.0), "{endgame:?} finished at {with}");
        assert_eq!(
            node.events_where(|k| matches!(k, EventKind::EndgameStarted { .. }))
                .len(),
            1
        );
        let cancelled = node.events_where(|k| {
            matches!(
                k,
                EventKind::Cancelled {
                    reason: CancelReason::Duplicate,
                    ..
                }
            )
        });
        assert!(!cancelled.is_empty(), "{endgame:?} cancelled nothing");
        assert!(
            node.events_where(|k| matches!(
                k,
                EventKind::Requested {
                    reason: SelectReason::Endgame { .. },
                    ..
                }
            ))
            .len()
                >= 4
        );
    }
}

/// How long one fetcher takes to download 8 MiB over a 5 MB/s link with a 200 ms round trip.
fn long_link(adaptive: bool) -> (Time, Vec<u32>) {
    let content = bundle(&[512 * CHUNK]);
    let mut config = Config::default();
    config.requests.adaptive = adaptive;
    if !adaptive {
        config.requests.initial_in_flight = 1;
        config.requests.min_in_flight = 1;
    }
    let mut swarm = Swarm::new();
    let origin = swarm.add_complete(1, config.clone(), &content);
    let fetcher = swarm.add_empty(2, config, &content);
    swarm.nodes[origin].upload_rate = Some(5_000_000);
    swarm.nodes[origin].latency = Duration::from_millis(50);
    swarm.nodes[fetcher].latency = Duration::from_millis(50);
    swarm.introduce(fetcher, &[origin]);
    swarm.run(far_future());
    let node = &swarm.nodes[fetcher];
    let sizes = node
        .events
        .iter()
        .filter_map(|e| match e.kind {
            EventKind::InFlightChanged { requests, .. } => Some(requests),
            _ => None,
        })
        .collect();
    (node.completed_at().unwrap(), sizes)
}

#[test]
fn adaptive_pipelining_fills_a_long_link() {
    let (fixed, _) = long_link(false);
    let (adaptive, sizes) = long_link(true);
    // One request at a time moves 256 KiB per round trip, about 1 MB/s.
    assert!(fixed > at(6.0), "fixed took {fixed}");
    // Covering rate × RTT soon keeps the 5 MB/s link busy: 8 MiB take 1.7 s at that rate, after
    // 0.7 s of connecting and fetching the manifest.
    assert!(adaptive < at(3.0), "adaptive took {adaptive}");
    assert!(sizes.iter().any(|&n| n > 4), "never grew: {sizes:?}");
}

#[test]
fn a_reported_rtt_drives_the_pipeline() {
    let content = bundle(&[64 * CHUNK]);
    let mut swarm = Swarm::new();
    let origin = swarm.add_complete(1, Config::default(), &content);
    let fetcher = swarm.add_empty(2, Config::default(), &content);
    swarm.nodes[origin].upload_rate = Some(1_000_000);
    swarm.introduce(fetcher, &[origin]);
    swarm.run(at(0.3));
    let conn = swarm.nodes[fetcher]
        .events
        .iter()
        .find_map(|e| match e.kind {
            EventKind::Opened { conn, .. } => Some(conn),
            _ => None,
        })
        .unwrap();
    let now = swarm.now;
    swarm.nodes[fetcher]
        .session
        .handle_rtt(now, conn, Duration::from_secs(2));
    swarm.run(far_future());
    let measured = swarm.nodes[fetcher].events_where(|k| {
        matches!(
            k,
            EventKind::InFlightChanged {
                reason: InFlightReason::Measured { rtt, .. },
                ..
            } if *rtt == Duration::from_secs(2)
        )
    });
    assert!(!measured.is_empty());
}

#[test]
fn superseeding_sends_each_chunk_once_before_sharing_everything() {
    let content = bundle(&[64 * CHUNK, 0, 10 * CHUNK + 3]);
    let mut config = Config::default();
    config.requests.chunks = 4;
    let mut swarm = Swarm::new();
    let mut seed_config = config.clone();
    seed_config.strategies.superseed = true;
    let origin = swarm.add_complete(1, seed_config, &content);
    swarm.nodes[origin].upload_rate = Some(1_000_000);
    let fetchers: Vec<usize> = (2..6)
        .map(|n| swarm.add_empty(n, config.clone(), &content))
        .collect();
    for &f in &fetchers {
        swarm.nodes[f].upload_rate = Some(1_000_000);
        swarm.nodes[f].latency = Duration::from_millis(5);
    }
    let mut everyone = fetchers.clone();
    everyone.push(origin);
    swarm.introduce_all(&everyone);
    swarm.run(far_future());

    for &f in &fetchers {
        assert_eq!(swarm.nodes[f].content.files(), content.files());
    }
    let seed = &swarm.nodes[origin];
    assert!(!seed.session.is_superseeding());
    assert!(
        !seed
            .events_where(|k| matches!(k, EventKind::Offered { .. }))
            .is_empty()
    );
    let end = seed
        .events
        .iter()
        .position(|e| {
            e.kind
                == EventKind::SuperseedingEnded {
                    reason: SuperseedEnd::AllSent,
                }
        })
        .expect("superseeding ended");
    let mut sent = std::collections::BTreeSet::new();
    let mut bytes = 0;
    for event in &seed.events[..end] {
        if let EventKind::Uploaded {
            start,
            count,
            bytes: b,
            ..
        } = event.kind
        {
            for chunk in start..start + count {
                assert!(
                    sent.insert(chunk),
                    "chunk {chunk} sent twice while superseeding"
                );
            }
            bytes += b;
        }
        if let EventKind::ServingChanged { reason, .. } = event.kind {
            assert!(matches!(
                reason,
                ServeReason::Superseeding | ServeReason::NotWanted
            ));
        }
    }
    assert_eq!(bytes, content.manifest().total_size());
}

/// An origin and one fetcher with the given lingering settings for both.
fn lingering_pair(configure: impl Fn(&mut Config)) -> Swarm {
    let content = bundle(&[20 * CHUNK]);
    let mut config = Config::default();
    configure(&mut config);
    let mut swarm = Swarm::new();
    let origin = swarm.add_complete(1, config.clone(), &content);
    let fetcher = swarm.add_empty(2, config, &content);
    swarm.introduce(fetcher, &[origin]);
    swarm.run(far_future());
    swarm
}

fn stopped(swarm: &Swarm, node: usize) -> Option<(Time, StopReason)> {
    swarm.nodes[node].events.iter().find_map(|e| match e.kind {
        EventKind::StoppedSharing { reason } => Some((e.time, reason)),
        _ => None,
    })
}

#[test]
fn a_finished_peer_stops_once_nobody_wants_anything() {
    let swarm = lingering_pair(|_| {});
    let done = swarm.nodes[1].completed_at().unwrap();
    let (time, reason) = stopped(&swarm, 1).expect("the fetcher stopped");
    assert_eq!(reason, StopReason::NobodyWants);
    assert_eq!(time, done + Duration::from_secs(600));
    assert!(swarm.nodes[1].session.is_stopped());
    // The origin started complete, so it never stops on its own.
    assert_eq!(stopped(&swarm, 0), None);

    let swarm = lingering_pair(|c| c.lingering = LingerSettings::KEEP_SHARING);
    assert_eq!(stopped(&swarm, 1), None);
    assert!(!swarm.nodes[1].session.is_stopped());
}

#[test]
fn a_finished_peer_stops_at_its_ratio() {
    let content = bundle(&[60 * CHUNK]);
    let mut config = Config::default();
    config.lingering.ratio = Some(0.05);
    config.requests.chunks = 2;
    let mut swarm = Swarm::new();
    let origin = swarm.add_complete(1, config.clone(), &content);
    swarm.nodes[origin].upload_rate = Some(100_000);
    let fetchers: Vec<usize> = (2..5)
        .map(|n| swarm.add_empty(n, config.clone(), &content))
        .collect();
    let mut everyone = fetchers.clone();
    everyone.push(origin);
    swarm.introduce_all(&everyone);
    swarm.run(far_future());
    for &f in &fetchers {
        assert_eq!(swarm.nodes[f].content.files(), content.files());
    }
    let reasons: Vec<_> = fetchers
        .iter()
        .filter_map(|&f| stopped(&swarm, f))
        .map(|(_, r)| r)
        .collect();
    assert!(reasons.contains(&StopReason::RatioReached), "{reasons:?}");
}

#[test]
fn a_direct_send_stops_once_delivered() {
    let swarm = lingering_pair(|c| c.lingering.direct_send = true);
    let (time, reason) = stopped(&swarm, 0).expect("the sender stopped");
    assert_eq!(reason, StopReason::Delivered);
    assert_eq!(time, swarm.nodes[1].completed_at().unwrap());
}

#[test]
fn peers_are_shared_so_others_can_connect() {
    let content = bundle(&[64 * CHUNK]);
    let mut swarm = Swarm::new();
    let origin = swarm.add_complete(1, Config::default(), &content);
    swarm.nodes[origin].upload_rate = Some(10_000);
    let near = swarm.add_empty(2, Config::default(), &content);
    let far = swarm.add_empty(3, Config::default(), &content);
    swarm.introduce(near, &[origin]);
    swarm.introduce(far, &[near]);
    swarm.run(far_future());

    let origin_id = swarm.nodes[origin].id;
    let connected = swarm.nodes[far]
        .events_where(|k| matches!(k, EventKind::Connected { peer, .. } if *peer == origin_id));
    assert!(!connected.is_empty(), "never learned of the origin");
    let shared = swarm.nodes[near].events_where(|k| matches!(k, EventKind::PeersShared { .. }));
    assert!(!shared.is_empty());
    // At most once a minute per connection, not counting the farewell (D-224).
    let regular: Vec<_> = (shared.iter())
        .filter(|e| {
            matches!(
                e.kind,
                EventKind::PeersShared {
                    farewell: false,
                    ..
                }
            )
        })
        .collect();
    for pair in regular.windows(2) {
        if let (EventKind::PeersShared { conn: a, .. }, EventKind::PeersShared { conn: b, .. }) =
            (&pair[0].kind, &pair[1].kind)
            && a == b
        {
            assert!(pair[1].time.since(pair[0].time) >= Duration::from_secs(60));
        }
    }
    assert_eq!(swarm.nodes[far].content.files(), content.files());
}

#[test]
fn a_leaving_peer_tells_the_others_who_else_has_the_bundle() {
    let content = bundle(&[40 * CHUNK]);
    let mut config = Config::default();
    // Without the farewell, nothing would tell D about the origin.
    config.connections.peer_exchange_interval = Duration::from_secs(86_400);
    let mut leaver = config.clone();
    leaver.lingering.ratio = Some(0.01);
    leaver.requests.chunks = 2;
    let mut swarm = Swarm::new();
    let origin = swarm.add_complete(1, config.clone(), &content);
    let a = swarm.add_empty(2, leaver, &content);
    let d = swarm.add_empty(3, config, &content);
    swarm.nodes[origin].upload_rate = Some(400_000);
    swarm.introduce(a, &[origin]);
    swarm.introduce(d, &[a]);
    swarm.run(far_future());

    let origin_id = swarm.nodes[origin].id;
    assert!(swarm.nodes[a].session.is_stopped(), "A never left");
    let farewells =
        swarm.nodes[a].events_where(|k| matches!(k, EventKind::PeersShared { farewell: true, .. }));
    assert!(!farewells.is_empty(), "A said no farewell");
    let met = swarm.nodes[d]
        .events_where(|k| matches!(k, EventKind::Connected { peer, .. } if *peer == origin_id));
    assert!(!met.is_empty(), "D never learned of the origin");
    assert_eq!(swarm.nodes[d].content.files(), content.files());
}

#[test]
fn an_idle_connection_makes_room_for_a_new_peer() {
    let content = bundle(&[4 * CHUNK]);
    let mut config = Config::default();
    config.connections.max = 2;
    let mut swarm = Swarm::new();
    // Nobody has the manifest, so the connections stay idle.
    let hub = swarm.add_empty(1, config, &content);
    let first: Vec<usize> = (2..4)
        .map(|n| swarm.add_empty(n, Config::default(), &content))
        .collect();
    for &f in &first {
        swarm.introduce(f, &[hub]);
    }
    swarm.run(at(10.0));
    assert_eq!(swarm.nodes[hub].session.connection_count(), 2);

    // Too early: the connections aren't idle yet. This peer doesn't try again.
    let mut patient = Config::default();
    patient.connections.redial_delay = Duration::from_secs(3600);
    let early = swarm.add_empty(4, patient, &content);
    swarm.introduce(early, &[hub]);
    swarm.run(at(20.0));
    let refused = swarm.nodes[hub].events_where(|k| {
        matches!(
            k,
            EventKind::Closed {
                cause: CloseCause::Local { reason: GoodbyeReason::TooManyConnections, message },
                ..
            } if message.is_empty()
        )
    });
    assert_eq!(refused.len(), 1);

    swarm.run(at(120.0));
    let late = swarm.add_empty(5, Config::default(), &content);
    swarm.introduce(late, &[hub]);
    swarm.run(at(121.0));
    let late_id = swarm.nodes[late].id;
    let hub_events = &swarm.nodes[hub];
    assert_eq!(
        hub_events
            .events_where(|k| matches!(
                k,
                EventKind::Closed {
                    cause: CloseCause::Local { reason: GoodbyeReason::TooManyConnections, message },
                    ..
                } if message.contains("idle")
            ))
            .len(),
        1
    );
    assert!(
        !hub_events
            .events_where(|k| matches!(k, EventKind::Opened { peer, .. } if *peer == late_id))
            .is_empty()
    );
    assert_eq!(swarm.nodes[hub].session.connection_count(), 2);
}

#[test]
fn a_snubbing_provider_loses_its_requests() {
    let content = bundle(&[32 * CHUNK]);
    let mut config = Config::default();
    config.requests.chunks = 4;
    config.strategies.endgame = Endgame::Off;
    let mut swarm = Swarm::new();
    let mute = swarm.add_complete(1, config.clone(), &content);
    swarm.nodes[mute].mute = true;
    let good = swarm.add_complete(2, config.clone(), &content);
    swarm.nodes[good].upload_rate = Some(1_000_000);
    let fetcher = swarm.add_empty(3, config, &content);
    swarm.introduce(fetcher, &[mute, good]);
    swarm.run(far_future());

    let node = &swarm.nodes[fetcher];
    assert_eq!(node.content.files(), content.files());
    let done = node.completed_at().unwrap();
    assert!(done >= at(60.0) && done < at(70.0), "finished at {done}");
    assert_eq!(
        node.events_where(|k| matches!(k, EventKind::Snubbed { .. }))
            .len(),
        1
    );
    assert!(
        !node
            .events_where(|k| matches!(
                k,
                EventKind::Cancelled {
                    reason: CancelReason::Snubbed,
                    ..
                }
            ))
            .is_empty()
    );
}

#[test]
fn streaming_requests_from_the_playback_position() {
    let content = bundle(&[128 * CHUNK]);
    let mut config = Config::default();
    config.strategies.chunk_selection = ChunkSelection::Streaming;
    config.requests.chunks = 4;
    let mut swarm = Swarm::new();
    let origin = swarm.add_complete(1, config.clone(), &content);
    swarm.nodes[origin].upload_rate = Some(1_000_000);
    let fetcher = swarm.add_empty(2, config, &content);
    swarm.nodes[fetcher]
        .session
        .set_playback_position(Time::ZERO, 50);
    swarm.introduce(fetcher, &[origin]);
    swarm.run(far_future());

    let requests: Vec<(u32, SelectReason)> = swarm.nodes[fetcher]
        .events
        .iter()
        .filter_map(|e| match e.kind {
            EventKind::Requested { start, reason, .. } => Some((start, reason)),
            _ => None,
        })
        .collect();
    // The window of 64 chunks from 50 comes first, in order.
    let window: Vec<u32> = requests
        .iter()
        .take_while(|(_, r)| matches!(r, SelectReason::NearPlayback { .. }))
        .map(|&(start, _)| start)
        .collect();
    assert_eq!(window.first(), Some(&48));
    assert!(window.windows(2).all(|w| w[0] < w[1]));
    assert!(*window.last().unwrap() >= 112);
    assert!(requests.len() > window.len());
    assert_eq!(swarm.nodes[fetcher].content.files(), content.files());
}

#[test]
fn bittorrent_serves_by_reciprocity() {
    let content = bundle(&[256 * CHUNK]);
    let mut swarm = Swarm::new();
    let origin = swarm.add_complete(1, Config::default(), &content);
    swarm.nodes[origin].upload_rate = Some(200_000);
    let fetchers: Vec<usize> = (2..14)
        .map(|n| swarm.add_empty(n, Config::default(), &content))
        .collect();
    for &f in &fetchers {
        swarm.nodes[f].upload_rate = Some(300_000);
        swarm.nodes[f].latency = Duration::from_millis(10);
    }
    let mut everyone = fetchers.clone();
    everyone.push(origin);
    swarm.introduce_all(&everyone);
    swarm.run(far_future());

    let reasons: Vec<ServeReason> = fetchers
        .iter()
        .flat_map(|&f| swarm.nodes[f].events.iter())
        .filter_map(|e| match e.kind {
            EventKind::ServingChanged { reason, .. } => Some(reason),
            _ => None,
        })
        .collect();
    assert!(
        reasons
            .iter()
            .any(|r| matches!(r, ServeReason::Reciprocal { .. }))
    );
    assert!(reasons.contains(&ServeReason::Optimistic));
    assert!(reasons.contains(&ServeReason::Outpaced));
    for &f in &fetchers {
        assert_eq!(swarm.nodes[f].content.files(), content.files());
    }
}

/// Serves nobody.
#[derive(Debug)]
struct Withhold;

impl UploadStrategy for Withhold {
    fn review(&mut self, ctx: &UploadContext<'_>, _rng: &mut Rng) -> Vec<ServeDecision> {
        ctx.peers
            .iter()
            .map(|p| ServeDecision {
                conn: p.conn,
                serve: false,
                reason: ServeReason::NotWanted,
            })
            .collect()
    }
}

/// What a peer that withholds upload while downloading sends, once complete, under
/// `when_complete`.
fn withholder_uploads(when_complete: WhenComplete) -> u64 {
    let content = bundle(&[64 * CHUNK]);
    let mut swarm = Swarm::new();
    let origin = swarm.add_complete(1, Config::default(), &content);
    swarm.nodes[origin].upload_rate = Some(100_000);
    let mut config = Config::default();
    config.strategies.when_complete = when_complete;
    let withholder = swarm.add_empty(2, config, &content);
    swarm.nodes[withholder]
        .session
        .set_upload_strategy(Box::new(Withhold));
    let other = swarm.add_empty(3, Config::default(), &content);
    swarm.nodes[other].upload_rate = Some(50_000);
    swarm.introduce_all(&[origin, withholder, other]);
    swarm.run(far_future());
    assert!(swarm.nodes[withholder].completed_at() <= swarm.nodes[other].completed_at());
    swarm.nodes[withholder].session.uploaded()
}

#[test]
fn a_custom_upload_strategy_lasts_until_complete_unless_kept() {
    assert!(withholder_uploads(WhenComplete::RoundRobin) > 0);
    assert_eq!(withholder_uploads(WhenComplete::Unchanged), 0);
}
