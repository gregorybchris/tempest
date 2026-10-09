//! Listing pending bundles: fetching their manifests from their swarms (D-129).
//!
//! A listing stays pending until the tracker knows what the bundle holds. The fetcher looks at
//! the pending listings every so often and, for each one that is due, asks a few of the
//! bundle's live peers for its manifest over QUIC, as any peer would. A manifest that matches
//! the bundle ID gives the listing its content, which lists it. A listing still pending after
//! [`FetcherConfig::pending_lifetime`] is dropped, so requests for bundles nobody serves don't
//! pile up; its sharer's next announce asks again.
//!
//! [`Schedule`] decides when each bundle is due, and [`choose`] which peers to ask; both are
//! plain data, so the rules are tested without a network or a clock.

use std::collections::{HashMap, HashSet};
use std::io;
use std::time::{Duration, Instant};

use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use tempest_net::{Endpoint, EndpointConfig, FetchLimits, Identity, fetch_manifest};
use tempest_protocol::message::PeerRecord;
use tempest_protocol::tracker::PeerEntry;
use tempest_protocol::{BundleId, Manifest, PeerId};
use tempest_tracker_api::listing::Content;
use tokio::task::JoinSet;

use crate::catalog;
use crate::server::{Tracker, blocking};

/// The most addresses dialed for one peer: the one the tracker observed comes first.
const MAX_ADDRS_PER_PEER: usize = 4;

/// How the fetcher works and what bounds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetcherConfig {
    /// The most bundles fetched at once.
    pub concurrency: usize,
    /// The most peers asked in one attempt, one after another.
    pub peers_per_attempt: usize,
    /// The limits of each request to a peer, including the largest manifest accepted.
    pub limits: FetchLimits,
    /// How long to wait after the first failed attempt. Each further failure doubles it, up to
    /// `max_retry`. A bundle whose swarm is empty is looked at again after this long too.
    pub retry: Duration,
    /// The longest wait between attempts.
    pub max_retry: Duration,
    /// How long a listing may stay pending, from when it was made or last edited, before it is
    /// dropped.
    pub pending_lifetime: Duration,
    /// How often the pending listings are looked at.
    pub scan_interval: Duration,
    /// Where the fetcher's QUIC endpoint binds. It dials out only.
    pub endpoint: EndpointConfig,
}

impl Default for FetcherConfig {
    fn default() -> Self {
        Self {
            concurrency: 8,
            peers_per_attempt: 3,
            limits: FetchLimits::default(),
            retry: Duration::from_secs(30),
            max_retry: Duration::from_secs(60 * 60),
            pending_lifetime: Duration::from_secs(3 * 24 * 60 * 60),
            scan_interval: Duration::from_secs(5),
            endpoint: EndpointConfig::any(0),
        }
    }
}

/// One pending bundle's attempts so far.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Attempts {
    /// Failures in a row.
    failures: u32,
    /// Nothing is tried before this.
    next: Instant,
}

/// When each pending bundle is due.
#[derive(Debug)]
pub(crate) struct Schedule {
    retry: Duration,
    max_retry: Duration,
    attempts: HashMap<BundleId, Attempts>,
}

/// What became of one bundle's attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Its manifest arrived.
    Fetched,
    /// No peer in its swarm to ask.
    NoPeers,
    /// Every peer asked failed.
    Failed,
}

impl Schedule {
    pub(crate) fn new(config: &FetcherConfig) -> Self {
        Self {
            retry: config.retry,
            max_retry: config.max_retry,
            attempts: HashMap::new(),
        }
    }

    /// Of `pending`, at most `slots` bundles due at `now` and not `busy`, oldest first. Forgets
    /// bundles no longer pending.
    pub(crate) fn due<T: Copy>(
        &mut self,
        now: Instant,
        pending: &[(BundleId, T)],
        busy: &HashSet<BundleId>,
        slots: usize,
    ) -> Vec<(BundleId, T)> {
        let still: HashSet<&BundleId> = pending.iter().map(|(bundle, _)| bundle).collect();
        self.attempts.retain(|bundle, _| still.contains(bundle));
        pending
            .iter()
            .filter(|(bundle, _)| !busy.contains(bundle))
            .filter(|(bundle, _)| self.attempts.get(bundle).is_none_or(|a| a.next <= now))
            .take(slots)
            .copied()
            .collect()
    }

    /// Records how an attempt on `bundle` at `now` went, and returns what became of it.
    pub(crate) fn record(
        &mut self,
        now: Instant,
        bundle: BundleId,
        fetched: Option<bool>,
    ) -> Outcome {
        let entry = self.attempts.entry(bundle).or_insert(Attempts {
            failures: 0,
            next: now,
        });
        match fetched {
            Some(true) => {
                self.attempts.remove(&bundle);
                Outcome::Fetched
            }
            // An empty swarm isn't the peers' failure; look again later.
            None => {
                entry.next = now + self.retry;
                Outcome::NoPeers
            }
            Some(false) => {
                let wait = self.retry.saturating_mul(1 << entry.failures.min(16));
                entry.failures += 1;
                entry.next = now + wait.min(self.max_retry);
                Outcome::Failed
            }
        }
    }
}

/// The peers to ask for a manifest, in order: the publisher, then complete peers, then
/// incomplete ones, which may have it too, each group in random order. At most `count`, each
/// with at most [`MAX_ADDRS_PER_PEER`] addresses.
pub(crate) fn choose(
    mut peers: Vec<(PeerEntry, bool)>,
    publisher: &PeerId,
    count: usize,
    rng: &mut impl Rng,
) -> Vec<PeerRecord> {
    peers.shuffle(rng);
    peers.sort_by_key(|(entry, complete)| (entry.peer != *publisher, !complete));
    peers
        .into_iter()
        .take(count)
        .map(|(entry, _)| PeerRecord {
            peer_id: entry.peer,
            addresses: entry.addrs.into_iter().take(MAX_ADDRS_PER_PEER).collect(),
            relay: entry.relay,
        })
        .collect()
}

/// What a manifest says a bundle holds.
pub(crate) fn content(manifest: &Manifest) -> Content {
    Content {
        name: manifest.name().to_owned(),
        size: manifest.total_size(),
        files: manifest
            .entries()
            .iter()
            .filter(|entry| entry.as_file().is_some())
            .count() as u64,
    }
}

/// Asks `peers` for `bundle`'s manifest one after another, until one sends it.
async fn fetch(
    endpoint: Endpoint,
    bundle: BundleId,
    peers: Vec<PeerRecord>,
    limits: FetchLimits,
) -> Option<Manifest> {
    for peer in &peers {
        if let Ok(manifest) = fetch_manifest(&endpoint, bundle, peer, limits).await {
            return Some(manifest);
        }
    }
    None
}

/// Runs the fetcher for `tracker`'s catalog until the task is dropped.
pub(crate) async fn run(tracker: Tracker, config: FetcherConfig) -> io::Result<()> {
    let identity = Identity::generate().map_err(io::Error::other)?;
    let endpoint = Endpoint::bind(identity, &config.endpoint)?;
    let mut rng = ChaCha8Rng::seed_from_u64(getrandom::u64()?);
    let mut schedule = Schedule::new(&config);
    let mut tasks = JoinSet::new();
    let mut busy = HashMap::new();
    let mut tick = tokio::time::interval(config.scan_interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = tick.tick() => {
                let lifetime = config.pending_lifetime;
                let pending = blocking(tracker.clone(), move |tracker| {
                    let mut catalog = tracker.catalog();
                    catalog.drop_pending(catalog::unix_now(), lifetime)?;
                    catalog.pending()
                });
                let pending = match pending.await {
                    Ok(pending) => pending,
                    Err(error) => {
                        tracing::error!("cannot read the pending listings: {error}");
                        continue;
                    }
                };
                let now = Instant::now();
                let in_flight: HashSet<BundleId> = busy.values().copied().collect();
                let slots = config.concurrency.saturating_sub(busy.len());
                for (bundle, publisher) in schedule.due(now, &pending, &in_flight, slots) {
                    let swarm = tracker.lock().peers(now, &bundle);
                    let peers = choose(swarm, &publisher, config.peers_per_attempt, &mut rng);
                    if peers.is_empty() {
                        schedule.record(now, bundle, None);
                        continue;
                    }
                    let attempt = fetch(endpoint.clone(), bundle, peers, config.limits);
                    busy.insert(tasks.spawn(attempt).id(), bundle);
                }
            }
            Some(joined) = tasks.join_next_with_id() => {
                let (id, manifest) = match joined {
                    Ok((id, manifest)) => (id, manifest),
                    Err(error) => (error.id(), None),
                };
                let Some(bundle) = busy.remove(&id) else { continue };
                if let Some(manifest) = &manifest {
                    let content = content(manifest);
                    let stored = blocking(tracker.clone(), move |tracker| {
                        tracker.catalog().set_content(&bundle, &content)
                    });
                    if let Err(error) = stored.await {
                        tracing::error!("cannot list {bundle}: {error}");
                        continue;
                    }
                }
                schedule.record(Instant::now(), bundle, Some(manifest.is_some()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use tempest_protocol::Hash;

    use super::*;

    fn bundle(n: u8) -> BundleId {
        Hash::new([n; 32])
    }

    fn config() -> FetcherConfig {
        FetcherConfig {
            retry: Duration::from_secs(10),
            max_retry: Duration::from_secs(60),
            ..FetcherConfig::default()
        }
    }

    #[test]
    fn due_bundles_are_taken_oldest_first_within_the_free_slots() {
        let mut s = Schedule::new(&config());
        let t = Instant::now();
        let pending = [(bundle(1), ()), (bundle(2), ()), (bundle(3), ())];
        let busy = HashSet::from([bundle(1)]);
        let due: Vec<_> = s
            .due(t, &pending, &busy, 1)
            .into_iter()
            .map(|p| p.0)
            .collect();
        assert_eq!(due, [bundle(2)]);
        let due = s.due(t, &pending, &HashSet::new(), 10);
        assert_eq!(due.len(), 3);
    }

    #[test]
    fn failures_back_off_up_to_max_retry() {
        let mut s = Schedule::new(&config());
        let t = Instant::now();
        let pending = [(bundle(1), ())];
        let none = HashSet::new();
        assert_eq!(s.record(t, bundle(1), Some(false)), Outcome::Failed);
        assert!(
            s.due(t + Duration::from_secs(9), &pending, &none, 1)
                .is_empty()
        );
        assert_eq!(
            s.due(t + Duration::from_secs(10), &pending, &none, 1).len(),
            1
        );

        let t2 = t + Duration::from_secs(10);
        assert_eq!(s.record(t2, bundle(1), Some(false)), Outcome::Failed);
        assert!(
            s.due(t2 + Duration::from_secs(19), &pending, &none, 1)
                .is_empty()
        );
        assert_eq!(
            s.due(t2 + Duration::from_secs(20), &pending, &none, 1)
                .len(),
            1
        );

        // The wait stops growing at max_retry.
        for n in 0..5 {
            s.record(t2 + Duration::from_secs(n), bundle(1), Some(false));
        }
        let entry = &s.attempts[&bundle(1)];
        assert_eq!(entry.next, t2 + Duration::from_secs(4 + 60));
    }

    #[test]
    fn empty_swarms_wait_without_counting_as_failures() {
        let mut s = Schedule::new(&config());
        let t = Instant::now();
        let pending = [(bundle(1), ())];
        let none = HashSet::new();
        for n in 0..100 {
            let at = t + Duration::from_secs(10 * n);
            assert_eq!(s.record(at, bundle(1), None), Outcome::NoPeers);
        }
        let at = t + Duration::from_secs(1000);
        assert_eq!(s.due(at, &pending, &none, 1).len(), 1);
        assert_eq!(s.record(at, bundle(1), Some(false)), Outcome::Failed);
    }

    #[test]
    fn fetched_and_vanished_bundles_are_forgotten() {
        let mut s = Schedule::new(&config());
        let t = Instant::now();
        s.record(t, bundle(1), Some(false));
        s.record(t, bundle(2), Some(false));
        assert_eq!(s.record(t, bundle(1), Some(true)), Outcome::Fetched);
        assert!(!s.attempts.contains_key(&bundle(1)));
        s.due(t, &[(bundle(3), ())], &HashSet::new(), 1);
        assert!(s.attempts.is_empty());
    }

    #[test]
    fn the_publisher_then_complete_peers_are_asked_first() {
        let entry = |n: u8, addrs: usize| PeerEntry {
            peer: Hash::new([n; 32]),
            addrs: (0..addrs)
                .map(|port| format!("203.0.113.{n}:{}", 4000 + port).parse().unwrap())
                .collect(),
            relay: None,
        };
        let peers = vec![
            (entry(1, 1), false),
            (entry(2, 1), true),
            (entry(3, 9), false),
            (entry(4, 1), true),
        ];
        let mut rng = ChaCha8Rng::seed_from_u64(1);
        let chosen = choose(peers.clone(), &Hash::new([3; 32]), 3, &mut rng);
        let ids: Vec<u8> = chosen.iter().map(|p| p.peer_id.as_bytes()[0]).collect();
        assert_eq!(ids[0], 3);
        assert_eq!(chosen[0].addresses.len(), MAX_ADDRS_PER_PEER);
        let mut rest = ids[1..].to_vec();
        rest.sort();
        assert_eq!(rest, [2, 4]);

        let nobody = choose(Vec::new(), &Hash::new([3; 32]), 3, &mut rng);
        assert!(nobody.is_empty());
    }
}
