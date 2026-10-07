//! The peer-list service's state: who is in each swarm (`protocol/trackers.md`).
//!
//! [`Swarms`] is plain data with no I/O. The caller passes the time and the address it observed,
//! so the rules can be tested without a server or a clock.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::time::{Duration, Instant};

use rand::SeedableRng;
use rand::seq::IteratorRandom;
use rand_chacha::ChaCha8Rng;
use tempest_protocol::tracker::{
    AnnounceEvent, AnnounceRequest, AnnounceResponse, BundleStats, ErrorCode, ErrorResponse,
    PeerEntry,
};
use tempest_protocol::{Address, BundleId, PeerId, RelayIdentity};

/// How a tracker treats announces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackerConfig {
    /// How long peers should wait between announces: `interval_secs` in every response.
    pub interval: Duration,
    /// The shortest time between a peer's `update`s that the tracker accepts. Sooner ones are
    /// refused with `rate_limited`.
    pub min_interval: Duration,
    /// The most peers one response lists, whatever `num_want` asks for.
    pub max_num_want: u32,
    /// The most peers listed across all swarms. New peers are refused with `unavailable` beyond
    /// it, which bounds the tracker's memory.
    pub max_peers: usize,
    /// The most addresses an announce may carry in `addrs`.
    pub max_addrs: usize,
    /// The only bundles served, for a private tracker; `None` serves every bundle.
    pub allowed: Option<HashSet<BundleId>>,
    /// The relays recommended in every response.
    pub relays: Vec<RelayIdentity>,
}

impl Default for TrackerConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(300),
            min_interval: Duration::from_secs(60),
            max_num_want: 200,
            max_peers: 1_000_000,
            max_addrs: 16,
            allowed: None,
            relays: Vec::new(),
        }
    }
}

/// How long a full tracker asks new peers to wait before trying again.
const FULL_RETRY: Duration = Duration::from_secs(60);

/// An announce the tracker refused: the error response, and the HTTP status to send it with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The HTTP status.
    pub status: u16,
    /// The JSON body.
    pub body: ErrorResponse,
}

impl Refusal {
    /// A refusal with `code`'s typical status.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            status: code.typical_status().unwrap_or(400),
            body: ErrorResponse {
                error: code,
                message: message.into(),
                retry_after_secs: None,
            },
        }
    }

    /// The refusal with `retry_after_secs` set to `after`, rounded up to whole seconds.
    pub fn retry_after(mut self, after: Duration) -> Self {
        self.body.retry_after_secs = Some(after.as_secs() + u64::from(after.subsec_nanos() > 0));
        self
    }
}

/// One peer in a swarm.
#[derive(Debug, Clone)]
struct Listing {
    /// Where to reach it: the observed address first, then the ones it announced.
    addrs: Vec<Address>,
    relay: Option<Address>,
    complete: bool,
    last_announce: Instant,
    /// When it is forgotten unless it announces again: twice the interval it was given.
    expires: Instant,
}

/// One bundle's swarm.
#[derive(Debug, Default)]
struct Swarm {
    peers: HashMap<PeerId, Listing>,
    complete: u64,
}

impl Swarm {
    fn stats(&self) -> BundleStats {
        BundleStats {
            complete: self.complete,
            incomplete: self.peers.len() as u64 - self.complete,
        }
    }

    fn insert(&mut self, peer: PeerId, listing: Listing) {
        self.complete += u64::from(listing.complete);
        if let Some(old) = self.peers.insert(peer, listing) {
            self.complete -= u64::from(old.complete);
        }
    }

    fn remove(&mut self, peer: &PeerId) -> bool {
        let Some(old) = self.peers.remove(peer) else {
            return false;
        };
        self.complete -= u64::from(old.complete);
        true
    }

    /// Forgets the peers that expired by `now`, and returns how many.
    fn expire(&mut self, now: Instant) -> usize {
        let before = self.peers.len();
        let mut complete = 0;
        self.peers.retain(|_, listing| {
            let keep = listing.expires > now;
            complete += u64::from(keep && listing.complete);
            keep
        });
        self.complete = complete;
        before - self.peers.len()
    }
}

/// Every swarm a tracker knows of.
#[derive(Debug)]
pub struct Swarms {
    config: TrackerConfig,
    swarms: HashMap<BundleId, Swarm>,
    /// The number of peers listed across all swarms.
    listed: usize,
    rng: ChaCha8Rng,
}

impl Swarms {
    /// No swarms yet. `seed` seeds the random choice of peers in responses.
    pub fn new(config: TrackerConfig, seed: u64) -> Self {
        Self {
            config,
            swarms: HashMap::new(),
            listed: 0,
            rng: ChaCha8Rng::seed_from_u64(seed),
        }
    }

    /// The tracker's settings.
    pub fn config(&self) -> &TrackerConfig {
        &self.config
    }

    /// The number of peers listed across all swarms.
    pub fn listed(&self) -> usize {
        self.listed
    }

    /// Handles an announce that arrived at `now` from `observed_ip`.
    pub fn announce(
        &mut self,
        now: Instant,
        request: &AnnounceRequest,
        observed_ip: IpAddr,
    ) -> Result<AnnounceResponse, Refusal> {
        let config = &self.config;
        if config
            .allowed
            .as_ref()
            .is_some_and(|allowed| !allowed.contains(&request.bundle))
        {
            return Err(Refusal::new(
                ErrorCode::BundleNotAllowed,
                "this tracker doesn't serve this bundle",
            ));
        }
        if request.port == 0 {
            return Err(Refusal::new(
                ErrorCode::InvalidRequest,
                "port must not be 0",
            ));
        }
        if request.addrs.len() > config.max_addrs {
            return Err(Refusal::new(
                ErrorCode::InvalidRequest,
                format!("addrs may hold at most {} addresses", config.max_addrs),
            ));
        }
        // Canonical, so an IPv4 client seen through a dual-stack socket reads as IPv4.
        let observed = Address::new(observed_ip, request.port);

        let swarm = self.swarms.entry(request.bundle).or_default();
        self.listed -= swarm.expire(now);
        let known = swarm.peers.get(&request.peer);
        if let Some(listing) = known
            && request.event == AnnounceEvent::Update
        {
            let allowed_at = listing.last_announce + config.min_interval;
            if now < allowed_at {
                let refusal = Refusal::new(ErrorCode::RateLimited, "announcing too often");
                return Err(refusal.retry_after(allowed_at - now));
            }
        }

        let peers = if request.event == AnnounceEvent::Stopped {
            if swarm.remove(&request.peer) {
                self.listed -= 1;
            }
            Vec::new()
        } else {
            if known.is_none() {
                if self.listed >= config.max_peers {
                    if swarm.peers.is_empty() {
                        self.swarms.remove(&request.bundle);
                    }
                    let refusal = Refusal::new(ErrorCode::Unavailable, "the tracker is full");
                    return Err(refusal.retry_after(FULL_RETRY));
                }
                self.listed += 1;
            }
            let mut addrs = vec![observed];
            for addr in &request.addrs {
                if !addrs.contains(addr) {
                    addrs.push(*addr);
                }
            }
            let listing = Listing {
                addrs,
                relay: request.relay,
                complete: request.complete,
                last_announce: now,
                expires: now + 2 * config.interval,
            };
            swarm.insert(request.peer, listing);
            let wanted = request.num_want.min(config.max_num_want) as usize;
            swarm
                .peers
                .iter()
                .filter(|(peer, _)| **peer != request.peer)
                .choose_multiple(&mut self.rng, wanted)
                .into_iter()
                .map(|(peer, listing)| PeerEntry {
                    peer: *peer,
                    addrs: listing.addrs.clone(),
                    relay: listing.relay,
                })
                .collect()
        };

        let stats = swarm.stats();
        if swarm.peers.is_empty() {
            self.swarms.remove(&request.bundle);
        }
        Ok(AnnounceResponse {
            interval_secs: config.interval.as_secs(),
            peers,
            complete: stats.complete,
            incomplete: stats.incomplete,
            your_ip: observed.ip(),
            relays: config.relays.clone(),
        })
    }

    /// A bundle's swarm counts at `now`: zero for a bundle the tracker knows nothing about.
    pub fn stats(&self, now: Instant, bundle: &BundleId) -> BundleStats {
        let Some(swarm) = self.swarms.get(bundle) else {
            return BundleStats::default();
        };
        let live = swarm.peers.values().filter(|listing| listing.expires > now);
        let (complete, incomplete) = live.fold((0, 0), |(c, i), listing| {
            if listing.complete {
                (c + 1, i)
            } else {
                (c, i + 1)
            }
        });
        BundleStats {
            complete,
            incomplete,
        }
    }

    /// Forgets every peer that expired by `now`, and the swarms left empty.
    pub fn sweep(&mut self, now: Instant) {
        let mut forgotten = 0;
        self.swarms.retain(|_, swarm| {
            forgotten += swarm.expire(now);
            !swarm.peers.is_empty()
        });
        self.listed -= forgotten;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempest_protocol::Hash;

    const IP: &str = "198.51.100.9";

    fn bundle(n: u8) -> BundleId {
        Hash::new([n; 32])
    }

    fn peer(n: u8) -> PeerId {
        Hash::new([n; 32])
    }

    fn request(peer_n: u8, event: AnnounceEvent) -> AnnounceRequest {
        AnnounceRequest {
            bundle: bundle(1),
            peer: peer(peer_n),
            port: 4773,
            addrs: vec![],
            relay: None,
            event,
            complete: false,
            num_want: 50,
        }
    }

    fn swarms() -> Swarms {
        Swarms::new(TrackerConfig::default(), 7)
    }

    fn ip() -> IpAddr {
        IP.parse().unwrap()
    }

    #[test]
    fn peers_see_each_other_but_not_themselves() {
        let mut s = swarms();
        let t = Instant::now();
        let first = s.announce(t, &request(1, AnnounceEvent::Started), ip());
        assert!(first.unwrap().peers.is_empty());

        let mut second = request(2, AnnounceEvent::Started);
        second.complete = true;
        second.addrs = vec!["[2001:db8::2]:4773".parse().unwrap()];
        second.relay = Some("203.0.113.7:40001".parse().unwrap());
        let response = s.announce(t, &second, "203.0.113.2".parse().unwrap());
        let response = response.unwrap();
        assert_eq!(response.peers.len(), 1);
        assert_eq!(response.peers[0].peer, peer(1));
        assert_eq!(
            response.peers[0].addrs,
            vec![format!("{IP}:4773").parse().unwrap()]
        );
        assert_eq!((response.complete, response.incomplete), (1, 1));
        assert_eq!(response.your_ip, "203.0.113.2".parse::<IpAddr>().unwrap());
        assert_eq!(response.interval_secs, 300);

        let later = t + Duration::from_secs(60);
        let response = s.announce(later, &request(1, AnnounceEvent::Update), ip());
        let entry = &response.unwrap().peers[0];
        assert_eq!(entry.peer, peer(2));
        assert_eq!(
            entry.addrs,
            vec![
                "203.0.113.2:4773".parse().unwrap(),
                "[2001:db8::2]:4773".parse().unwrap()
            ]
        );
        assert_eq!(entry.relay, second.relay);
    }

    #[test]
    fn mapped_ipv4_is_observed_as_ipv4() {
        let mut s = swarms();
        let mapped: IpAddr = "::ffff:198.51.100.9".parse().unwrap();
        let response = s.announce(Instant::now(), &request(1, AnnounceEvent::Started), mapped);
        assert_eq!(response.unwrap().your_ip, ip());
    }

    #[test]
    fn the_observed_address_is_not_repeated() {
        let mut s = swarms();
        let t = Instant::now();
        let mut first = request(1, AnnounceEvent::Started);
        first.addrs = vec![format!("{IP}:4773").parse().unwrap()];
        s.announce(t, &first, ip()).unwrap();
        let response = s.announce(t, &request(2, AnnounceEvent::Started), ip());
        assert_eq!(response.unwrap().peers[0].addrs.len(), 1);
    }

    #[test]
    fn stopped_removes_the_peer_at_once() {
        let mut s = swarms();
        let t = Instant::now();
        s.announce(t, &request(1, AnnounceEvent::Started), ip())
            .unwrap();
        s.announce(t, &request(2, AnnounceEvent::Started), ip())
            .unwrap();
        let response = s.announce(t, &request(1, AnnounceEvent::Stopped), ip());
        let response = response.unwrap();
        assert!(response.peers.is_empty());
        assert_eq!((response.complete, response.incomplete), (0, 1));
        assert_eq!(s.listed(), 1);
        assert_eq!(s.stats(t, &bundle(1)).incomplete, 1);
    }

    #[test]
    fn completed_moves_a_peer_to_complete() {
        let mut s = swarms();
        let t = Instant::now();
        s.announce(t, &request(1, AnnounceEvent::Started), ip())
            .unwrap();
        let mut done = request(1, AnnounceEvent::Completed);
        done.complete = true;
        let response = s.announce(t, &done, ip()).unwrap();
        assert_eq!((response.complete, response.incomplete), (1, 0));
    }

    #[test]
    fn silent_peers_are_forgotten_after_two_intervals() {
        let mut s = swarms();
        let t = Instant::now();
        s.announce(t, &request(1, AnnounceEvent::Started), ip())
            .unwrap();
        let almost = t + Duration::from_secs(599);
        assert_eq!(s.stats(almost, &bundle(1)).incomplete, 1);
        let gone = t + Duration::from_secs(600);
        assert_eq!(s.stats(gone, &bundle(1)), BundleStats::default());
        let response = s.announce(gone, &request(2, AnnounceEvent::Started), ip());
        assert!(response.unwrap().peers.is_empty());
        assert_eq!(s.listed(), 1);

        s.sweep(gone + Duration::from_secs(600));
        assert_eq!(s.listed(), 0);
        assert!(s.swarms.is_empty());
    }

    #[test]
    fn early_updates_are_rate_limited() {
        let mut s = swarms();
        let t = Instant::now();
        s.announce(t, &request(1, AnnounceEvent::Started), ip())
            .unwrap();
        let soon = t + Duration::from_millis(20_500);
        let refusal = s
            .announce(soon, &request(1, AnnounceEvent::Update), ip())
            .unwrap_err();
        assert_eq!(refusal.status, 429);
        assert_eq!(refusal.body.error, ErrorCode::RateLimited);
        assert_eq!(refusal.body.retry_after_secs, Some(40));

        // Other events are never early: they report a change.
        let mut done = request(1, AnnounceEvent::Completed);
        done.complete = true;
        assert!(s.announce(soon, &done, ip()).is_ok());
        let later = soon + Duration::from_secs(60);
        assert!(
            s.announce(later, &request(1, AnnounceEvent::Update), ip())
                .is_ok()
        );
    }

    #[test]
    fn num_want_is_honored_and_capped() {
        let config = TrackerConfig {
            max_num_want: 5,
            ..TrackerConfig::default()
        };
        let mut s = Swarms::new(config, 1);
        let t = Instant::now();
        for n in 0..20 {
            s.announce(t, &request(n, AnnounceEvent::Started), ip())
                .unwrap();
        }
        let mut few = request(100, AnnounceEvent::Started);
        few.num_want = 3;
        assert_eq!(s.announce(t, &few, ip()).unwrap().peers.len(), 3);
        let many = request(101, AnnounceEvent::Started);
        let peers = s.announce(t, &many, ip()).unwrap().peers;
        assert_eq!(peers.len(), 5);
        let distinct: HashSet<_> = peers.iter().map(|p| p.peer).collect();
        assert_eq!(distinct.len(), 5);
        assert!(!distinct.contains(&peer(101)));
    }

    #[test]
    fn a_full_tracker_refuses_new_peers_only() {
        let config = TrackerConfig {
            max_peers: 2,
            min_interval: Duration::ZERO,
            ..TrackerConfig::default()
        };
        let mut s = Swarms::new(config, 1);
        let t = Instant::now();
        s.announce(t, &request(1, AnnounceEvent::Started), ip())
            .unwrap();
        s.announce(t, &request(2, AnnounceEvent::Started), ip())
            .unwrap();
        let refusal = s
            .announce(t, &request(3, AnnounceEvent::Started), ip())
            .unwrap_err();
        assert_eq!(refusal.status, 503);
        assert_eq!(refusal.body.error, ErrorCode::Unavailable);
        assert_eq!(refusal.body.retry_after_secs, Some(60));
        assert!(
            s.announce(t, &request(1, AnnounceEvent::Update), ip())
                .is_ok()
        );
    }

    #[test]
    fn private_trackers_serve_only_allowed_bundles() {
        let config = TrackerConfig {
            allowed: Some(HashSet::from([bundle(2)])),
            ..TrackerConfig::default()
        };
        let mut s = Swarms::new(config, 1);
        let refusal = s
            .announce(Instant::now(), &request(1, AnnounceEvent::Started), ip())
            .unwrap_err();
        assert_eq!(refusal.status, 403);
        assert_eq!(refusal.body.error, ErrorCode::BundleNotAllowed);
    }

    #[test]
    fn malformed_announces_are_invalid() {
        let mut s = swarms();
        let t = Instant::now();
        let mut no_port = request(1, AnnounceEvent::Started);
        no_port.port = 0;
        assert_eq!(s.announce(t, &no_port, ip()).unwrap_err().status, 400);

        let mut crowded = request(1, AnnounceEvent::Started);
        crowded.addrs = (0..17)
            .map(|n| format!("203.0.113.{n}:1").parse().unwrap())
            .collect();
        let refusal = s.announce(t, &crowded, ip()).unwrap_err();
        assert_eq!(refusal.body.error, ErrorCode::InvalidRequest);
        assert_eq!(s.listed(), 0);
    }

    /// Every swarm's counts and the total match its listings, whatever the announces.
    fn check_counts(s: &Swarms) {
        let mut listed = 0;
        for swarm in s.swarms.values() {
            assert!(!swarm.peers.is_empty());
            let complete = swarm.peers.values().filter(|l| l.complete).count();
            assert_eq!(swarm.complete, complete as u64);
            listed += swarm.peers.len();
        }
        assert_eq!(s.listed, listed);
    }

    proptest::proptest! {
        #[test]
        fn counts_stay_consistent(
            steps in proptest::collection::vec((0u8..3, 0u8..6, 0u8..4, proptest::bool::ANY, 0u64..400), 1..200)
        ) {
            let config = TrackerConfig {
                max_peers: 10,
                ..TrackerConfig::default()
            };
            let mut s = Swarms::new(config, 3);
            let mut t = Instant::now();
            for (b, p, event, complete, wait) in steps {
                t += Duration::from_secs(wait);
                let event = [
                    AnnounceEvent::Started,
                    AnnounceEvent::Update,
                    AnnounceEvent::Completed,
                    AnnounceEvent::Stopped,
                ][event as usize];
                let mut announce = request(p, event);
                announce.bundle = bundle(b);
                announce.complete = complete;
                if let Ok(response) = s.announce(t, &announce, ip()) {
                    proptest::prop_assert!(response.peers.iter().all(|e| e.peer != peer(p)));
                    let stats = s.stats(t, &bundle(b));
                    proptest::prop_assert_eq!(
                        (response.complete, response.incomplete),
                        (stats.complete, stats.incomplete)
                    );
                }
                check_counts(&s);
                if wait > 350 {
                    s.sweep(t);
                    check_counts(&s);
                }
            }
        }
    }

    #[test]
    fn unknown_bundles_have_zero_counts() {
        assert_eq!(
            swarms().stats(Instant::now(), &bundle(9)),
            BundleStats::default()
        );
    }
}
