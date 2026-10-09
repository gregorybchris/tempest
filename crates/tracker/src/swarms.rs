//! The peer-list service's state: who is in each swarm (`protocol/trackers.md`).
//!
//! [`Swarms`] is plain data with no I/O. The caller passes the time and the address it observed,
//! so the rules can be tested without a server or a clock.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::time::{Duration, Instant};

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use tempest_protocol::tracker::{
    AnnounceEvent, AnnounceRequest, AnnounceResponse, BundleStats, ErrorCode, ErrorResponse,
    PeerEntry,
};
use tempest_protocol::{Address, BundleId, PeerId, RelayIdentity};

use crate::limits::client_key;

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
    /// The most peers listed across all swarms from one IP address, or one IPv6 /64, so no one
    /// client can fill the tracker with made-up peer IDs. New peers from it are refused with
    /// `rate_limited` beyond it.
    pub max_peers_per_ip: usize,
    /// The most peers listed in one swarm from one IP address, or one IPv6 /64, so no one client
    /// can crowd a swarm's responses with made-up peer IDs. Generous enough for many people
    /// behind one carrier-grade NAT.
    pub max_swarm_peers_per_ip: usize,
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
            max_peers_per_ip: 10_000,
            max_swarm_peers_per_ip: 100,
            max_addrs: 16,
            allowed: None,
            relays: Vec::new(),
        }
    }
}

/// `at + after`, or a time about a century after `at` if that overflows, so no setting can make
/// the arithmetic panic.
pub(crate) fn later(at: Instant, after: Duration) -> Instant {
    const CENTURY: Duration = Duration::from_secs(100 * 365 * 24 * 3600);
    at.checked_add(after.min(CENTURY)).unwrap_or(at)
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

/// How many peers each client lists, by the address [`client_key`] groups clients by.
#[derive(Debug, Default)]
struct PerClient(HashMap<IpAddr, usize>);

impl PerClient {
    fn get(&self, client: &IpAddr) -> usize {
        self.0.get(client).copied().unwrap_or(0)
    }

    fn add(&mut self, client: IpAddr) {
        *self.0.entry(client).or_default() += 1;
    }

    fn remove(&mut self, client: IpAddr) {
        if let Some(count) = self.0.get_mut(&client) {
            *count -= 1;
            if *count == 0 {
                self.0.remove(&client);
            }
        }
    }
}

/// The most announce paths a peer's addresses are kept from.
const MAX_PATHS: usize = 4;

/// What one announce said about a peer: the address it came from, then the ones it announced.
#[derive(Debug, Clone)]
struct Path {
    /// The IP the announce came from.
    observed: IpAddr,
    /// The observed address first, then the ones announced.
    addrs: Vec<Address>,
    /// When the announce arrived.
    at: Instant,
}

/// One peer in a swarm.
#[derive(Debug, Clone)]
struct Listing {
    /// Where to reach it: every live path's addresses, the latest announce's first.
    addrs: Vec<Address>,
    /// The announces the addresses come from, the latest first. A peer that announces to this
    /// tracker by more than one route, such as a public URL and a local one, is seen from a
    /// different IP on each, and any of them may be the one that reaches it. A path is
    /// forgotten when it stops announcing.
    paths: Vec<Path>,
    /// The client that last announced it, as [`client_key`] groups them.
    client: IpAddr,
    relay: Option<Address>,
    complete: bool,
    last_announce: Instant,
    /// When it is forgotten unless it announces again: twice the interval it was given.
    expires: Instant,
}

/// One bundle's swarm.
///
/// Its peers are kept in a list, indexed by peer ID, so a response can sample them in time
/// proportional to its size rather than the swarm's.
#[derive(Debug)]
struct Swarm {
    peers: Vec<(PeerId, Listing)>,
    index: HashMap<PeerId, usize>,
    complete: u64,
    /// How many of its peers each client lists.
    per_client: PerClient,
    /// No peer expires before this, so until then the swarm needs no expiry pass and its
    /// counts are exact. Re-announces only move expiries later, so it stays a lower bound.
    earliest_expiry: Instant,
}

impl Swarm {
    fn new(now: Instant) -> Self {
        Self {
            peers: Vec::new(),
            index: HashMap::new(),
            complete: 0,
            per_client: PerClient::default(),
            earliest_expiry: later(now, Duration::MAX),
        }
    }

    fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    fn get(&self, peer: &PeerId) -> Option<&Listing> {
        self.index.get(peer).map(|&i| &self.peers[i].1)
    }

    fn counts(&self) -> BundleStats {
        BundleStats {
            complete: self.complete,
            incomplete: self.peers.len() as u64 - self.complete,
        }
    }

    /// Lists `peer`, or updates its listing, counting it for its client here and in `all`, the
    /// counts across all swarms.
    fn insert(&mut self, peer: PeerId, listing: Listing, all: &mut PerClient) {
        self.complete += u64::from(listing.complete);
        self.earliest_expiry = self.earliest_expiry.min(listing.expires);
        self.per_client.add(listing.client);
        all.add(listing.client);
        match self.index.get(&peer) {
            Some(&i) => {
                let old = std::mem::replace(&mut self.peers[i].1, listing);
                self.complete -= u64::from(old.complete);
                self.per_client.remove(old.client);
                all.remove(old.client);
            }
            None => {
                self.index.insert(peer, self.peers.len());
                self.peers.push((peer, listing));
            }
        }
    }

    fn remove(&mut self, peer: &PeerId, all: &mut PerClient) -> bool {
        let Some(i) = self.index.remove(peer) else {
            return false;
        };
        let (_, old) = self.peers.swap_remove(i);
        if let Some((moved, _)) = self.peers.get(i) {
            self.index.insert(*moved, i);
        }
        self.complete -= u64::from(old.complete);
        self.per_client.remove(old.client);
        all.remove(old.client);
        true
    }

    /// Forgets the peers that expired by `now`, and returns how many.
    fn expire(&mut self, now: Instant, all: &mut PerClient) -> usize {
        if now < self.earliest_expiry {
            return 0;
        }
        let before = self.peers.len();
        let per_client = &mut self.per_client;
        self.peers.retain(|(_, listing)| {
            let live = listing.expires > now;
            if !live {
                per_client.remove(listing.client);
                all.remove(listing.client);
            }
            live
        });
        self.index = (self.peers.iter().enumerate())
            .map(|(i, (peer, _))| (*peer, i))
            .collect();
        self.complete = self.peers.iter().filter(|(_, l)| l.complete).count() as u64;
        self.earliest_expiry = (self.peers.iter())
            .map(|(_, listing)| listing.expires)
            .min()
            .unwrap_or_else(|| later(now, Duration::MAX));
        before - self.peers.len()
    }

    /// Up to `wanted` peers other than `except`, chosen at random.
    fn sample(&self, rng: &mut ChaCha8Rng, wanted: usize, except: &PeerId) -> Vec<PeerEntry> {
        // One more than wanted, in case `except` is among them.
        let amount = (wanted + 1).min(self.peers.len());
        let mut entries: Vec<PeerEntry> = rand::seq::index::sample(rng, self.peers.len(), amount)
            .into_iter()
            .map(|i| &self.peers[i])
            .filter(|(peer, _)| peer != except)
            .map(|(peer, listing)| PeerEntry {
                peer: *peer,
                addrs: listing.addrs.clone(),
                relay: listing.relay,
            })
            .collect();
        entries.truncate(wanted);
        entries
    }
}

/// Whether `ip` can only be reached from its own network or machine: loopback, private,
/// link-local or unique local.
fn is_local(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v4) => {
            v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified()
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
                || v6.is_unspecified()
        }
    }
}

/// `peers` as a requester at `requester` can use them. The address the tracker observed for a
/// peer is often its router's or the tracker's own machine's, which means nothing to a requester
/// elsewhere and costs it a dial timeout. A requester on a local network or machine keeps them.
/// A peer left with no address and no relay is dropped, since nobody could dial it.
fn reachable_by(requester: IpAddr, mut peers: Vec<PeerEntry>) -> Vec<PeerEntry> {
    if is_local(requester) {
        return peers;
    }
    for entry in &mut peers {
        entry.addrs.retain(|addr| !is_local(addr.ip()));
    }
    peers.retain(|entry| !entry.addrs.is_empty() || entry.relay.is_some());
    peers
}

/// Every swarm a tracker knows of.
#[derive(Debug)]
pub struct Swarms {
    config: TrackerConfig,
    swarms: HashMap<BundleId, Swarm>,
    /// The number of peers listed across all swarms.
    listed: usize,
    /// How many peers each client lists across all swarms.
    per_client: PerClient,
    rng: ChaCha8Rng,
}

impl Swarms {
    /// No swarms yet. `seed` seeds the random choice of peers in responses.
    pub fn new(config: TrackerConfig, seed: u64) -> Self {
        Self {
            config,
            swarms: HashMap::new(),
            listed: 0,
            per_client: PerClient::default(),
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
        let client = client_key(observed_ip);

        let swarm = (self.swarms.entry(request.bundle)).or_insert_with(|| Swarm::new(now));
        self.listed -= swarm.expire(now, &mut self.per_client);
        let known = swarm.get(&request.peer);
        let previous: Vec<Path> = known.map(|l| l.paths.clone()).unwrap_or_default();
        if let Some(listing) = known
            && request.event == AnnounceEvent::Update
        {
            let allowed_at = later(listing.last_announce, config.min_interval);
            if now < allowed_at {
                let refusal = Refusal::new(ErrorCode::RateLimited, "announcing too often");
                return Err(refusal.retry_after(allowed_at - now));
            }
        }

        let peers = if request.event == AnnounceEvent::Stopped {
            if swarm.remove(&request.peer, &mut self.per_client) {
                self.listed -= 1;
            }
            Vec::new()
        } else {
            // A peer new to the swarm, or announcing from another client, counts against that
            // client's limits.
            let refusal = if known.is_none() && self.listed >= config.max_peers {
                let refusal = Refusal::new(ErrorCode::Unavailable, "the tracker is full");
                Some(refusal.retry_after(FULL_RETRY))
            } else if known.is_none_or(|listing| listing.client != client)
                && (swarm.per_client.get(&client) >= config.max_swarm_peers_per_ip
                    || self.per_client.get(&client) >= config.max_peers_per_ip)
            {
                let message = "too many peers from this address";
                let refusal = Refusal::new(ErrorCode::RateLimited, message);
                Some(refusal.retry_after(config.interval))
            } else {
                None
            };
            if let Some(refusal) = refusal {
                if swarm.is_empty() {
                    self.swarms.remove(&request.bundle);
                }
                return Err(refusal);
            }
            if known.is_none() {
                self.listed += 1;
            }
            let mut here = vec![observed];
            for addr in &request.addrs {
                if !here.contains(addr) {
                    here.push(*addr);
                }
            }
            let window = config.interval.saturating_mul(2);
            let mut paths = vec![Path {
                observed: observed.ip(),
                addrs: here,
                at: now,
            }];
            paths.extend(
                previous
                    .into_iter()
                    .filter(|p| p.observed != observed.ip() && later(p.at, window) > now),
            );
            paths.truncate(MAX_PATHS);
            let mut addrs: Vec<Address> = Vec::new();
            for addr in paths.iter().flat_map(|p| &p.addrs) {
                if !addrs.contains(addr) {
                    addrs.push(*addr);
                }
            }
            let listing = Listing {
                addrs,
                paths,
                client,
                relay: request.relay,
                complete: request.complete,
                last_announce: now,
                expires: later(now, config.interval.saturating_mul(2)),
            };
            swarm.insert(request.peer, listing, &mut self.per_client);
            let wanted = request.num_want.min(config.max_num_want) as usize;
            swarm.sample(&mut self.rng, wanted, &request.peer)
        };

        let stats = swarm.counts();
        if swarm.is_empty() {
            self.swarms.remove(&request.bundle);
        }
        Ok(AnnounceResponse {
            interval_secs: config.interval.as_secs(),
            peers: reachable_by(observed_ip, peers),
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
        if now < swarm.earliest_expiry {
            return swarm.counts();
        }
        let live = (swarm.peers.iter()).filter(|(_, listing)| listing.expires > now);
        let (complete, incomplete) = live.fold((0, 0), |(c, i), (_, listing)| {
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

    /// Whether `peer` is a live, complete peer of `bundle`'s swarm at `now`.
    pub fn is_complete(&self, now: Instant, bundle: &BundleId, peer: &PeerId) -> bool {
        self.swarms
            .get(bundle)
            .and_then(|swarm| swarm.get(peer))
            .is_some_and(|listing| listing.complete && listing.expires > now)
    }

    /// A bundle's live peers at `now`, each with whether it is complete.
    pub fn peers(&self, now: Instant, bundle: &BundleId) -> Vec<(PeerEntry, bool)> {
        let Some(swarm) = self.swarms.get(bundle) else {
            return Vec::new();
        };
        swarm
            .peers
            .iter()
            .filter(|(_, listing)| listing.expires > now)
            .map(|(peer, listing)| {
                let entry = PeerEntry {
                    peer: *peer,
                    addrs: listing.addrs.clone(),
                    relay: listing.relay,
                };
                (entry, listing.complete)
            })
            .collect()
    }

    /// Forgets every peer that expired by `now`, and the swarms left empty.
    pub fn sweep(&mut self, now: Instant) {
        let mut forgotten = 0;
        let per_client = &mut self.per_client;
        self.swarms.retain(|_, swarm| {
            forgotten += swarm.expire(now, per_client);
            !swarm.is_empty()
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
            listing: None,
        }
    }

    fn swarms() -> Swarms {
        Swarms::new(TrackerConfig::default(), 7)
    }

    fn ip() -> IpAddr {
        IP.parse().unwrap()
    }

    #[test]
    fn huge_intervals_do_not_overflow() {
        let config = TrackerConfig {
            interval: Duration::MAX,
            min_interval: Duration::MAX,
            ..TrackerConfig::default()
        };
        let mut s = Swarms::new(config, 7);
        let t = Instant::now();
        s.announce(t, &request(1, AnnounceEvent::Started), ip())
            .unwrap();
        let refusal = s
            .announce(t, &request(1, AnnounceEvent::Update), ip())
            .unwrap_err();
        assert_eq!(refusal.body.error, ErrorCode::RateLimited);
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
    fn a_requester_elsewhere_is_not_given_local_addresses() {
        let mut s = swarms();
        let t = Instant::now();
        let mut first = request(1, AnnounceEvent::Started);
        first.addrs = vec![format!("{IP}:4773").parse().unwrap()];
        // The seeder announced from the tracker's own machine.
        s.announce(t, &first, "127.0.0.1".parse().unwrap()).unwrap();

        let outside = s.announce(t, &request(2, AnnounceEvent::Started), ip());
        let peers = outside.unwrap().peers;
        assert_eq!(peers[0].addrs, vec![format!("{IP}:4773").parse().unwrap()]);

        let local = s.announce(
            t,
            &request(3, AnnounceEvent::Started),
            "192.168.1.5".parse().unwrap(),
        );
        let peers = local.unwrap().peers;
        let wanted: Address = "127.0.0.1:4773".parse().unwrap();
        assert!(peers.iter().any(|entry| entry.addrs.contains(&wanted)));
    }

    #[test]
    fn a_peer_with_only_local_addresses_is_not_given_to_a_requester_elsewhere() {
        let mut s = swarms();
        let t = Instant::now();
        s.announce(
            t,
            &request(1, AnnounceEvent::Started),
            "10.0.0.7".parse().unwrap(),
        )
        .unwrap();
        let response = s.announce(t, &request(2, AnnounceEvent::Started), ip());
        assert!(response.unwrap().peers.is_empty());
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
    fn a_peer_seen_from_two_routes_keeps_both_until_one_goes_quiet() {
        let mut s = swarms();
        let t = Instant::now();
        let a: IpAddr = "198.51.100.9".parse().unwrap();
        let b: IpAddr = "203.0.113.7".parse().unwrap();
        s.announce(t, &request(1, AnnounceEvent::Started), a)
            .unwrap();
        let later_t = t + Duration::from_secs(10);
        s.announce(later_t, &request(1, AnnounceEvent::Started), b)
            .unwrap();
        let seen = |s: &mut Swarms, now: Instant| {
            let response = s.announce(now, &request(2, AnnounceEvent::Started), a);
            response.unwrap().peers[0].addrs.clone()
        };
        let both = seen(&mut s, later_t);
        assert_eq!(
            both,
            vec![
                "203.0.113.7:4773".parse::<Address>().unwrap(),
                "198.51.100.9:4773".parse().unwrap(),
            ]
        );

        // `b` keeps announcing; `a` doesn't, and is dropped after two intervals.
        let update = |s: &mut Swarms, secs: u64| {
            let now = t + Duration::from_secs(secs);
            s.announce(now, &request(1, AnnounceEvent::Update), b)
                .unwrap();
            now
        };
        update(&mut s, 400);
        let now = update(&mut s, 700);
        assert_eq!(
            seen(&mut s, now),
            vec!["203.0.113.7:4773".parse::<Address>().unwrap()]
        );
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

    #[test]
    fn one_client_lists_a_limited_number_of_peers() {
        let config = TrackerConfig {
            max_swarm_peers_per_ip: 2,
            max_peers_per_ip: 3,
            min_interval: Duration::ZERO,
            ..TrackerConfig::default()
        };
        let mut s = Swarms::new(config, 1);
        let t = Instant::now();
        let other: IpAddr = "203.0.113.2".parse().unwrap();
        let announce = |s: &mut Swarms, b: u8, p: u8, from: IpAddr| {
            let mut announce = request(p, AnnounceEvent::Started);
            announce.bundle = bundle(b);
            s.announce(t, &announce, from)
        };
        announce(&mut s, 1, 1, ip()).unwrap();
        announce(&mut s, 1, 2, ip()).unwrap();
        let refusal = announce(&mut s, 1, 3, ip()).unwrap_err();
        assert_eq!(refusal.status, 429);
        assert_eq!(refusal.body.error, ErrorCode::RateLimited);
        assert_eq!(refusal.body.retry_after_secs, Some(300));
        // Peers already listed still announce, and other clients still join.
        announce(&mut s, 1, 1, ip()).unwrap();
        announce(&mut s, 1, 3, other).unwrap();

        // Across swarms, the client's total is limited too.
        announce(&mut s, 2, 4, ip()).unwrap();
        assert!(announce(&mut s, 3, 5, ip()).is_err());
        assert!(!s.swarms.contains_key(&bundle(3)));

        // A peer that leaves frees its place.
        let mut stopped = request(4, AnnounceEvent::Stopped);
        stopped.bundle = bundle(2);
        s.announce(t, &stopped, ip()).unwrap();
        announce(&mut s, 3, 5, ip()).unwrap();

        // An IPv6 /64 is one client.
        for n in 6..8 {
            let from = format!("2001:db8::{n}").parse().unwrap();
            announce(&mut s, 4, n, from).unwrap();
        }
        assert!(announce(&mut s, 4, 8, "2001:db8::8".parse().unwrap()).is_err());
        assert!(announce(&mut s, 4, 8, "2001:db8:0:1::8".parse().unwrap()).is_ok());
    }

    /// Every swarm's counts and the total match its listings, whatever the announces.
    fn check_counts(s: &Swarms) {
        let mut listed = 0;
        let mut all = HashMap::new();
        for swarm in s.swarms.values() {
            let mut mine = HashMap::new();
            for (_, listing) in &swarm.peers {
                *mine.entry(listing.client).or_insert(0) += 1;
                *all.entry(listing.client).or_insert(0) += 1;
            }
            assert_eq!(swarm.per_client.0, mine);
            assert!(!swarm.peers.is_empty());
            let complete = swarm.peers.iter().filter(|(_, l)| l.complete).count();
            assert_eq!(swarm.complete, complete as u64);
            assert_eq!(swarm.index.len(), swarm.peers.len());
            for (i, (peer, listing)) in swarm.peers.iter().enumerate() {
                assert_eq!(swarm.index[peer], i);
                assert!(swarm.earliest_expiry <= listing.expires);
            }
            listed += swarm.peers.len();
        }
        assert_eq!(s.listed, listed);
        assert_eq!(s.per_client.0, all);
    }

    proptest::proptest! {
        #[test]
        fn counts_stay_consistent(
            steps in proptest::collection::vec((0u8..3, 0u8..6, 0u8..4, proptest::bool::ANY, 0u64..400, 0u8..3), 1..200)
        ) {
            let config = TrackerConfig {
                max_peers: 10,
                max_peers_per_ip: 5,
                max_swarm_peers_per_ip: 3,
                ..TrackerConfig::default()
            };
            let mut s = Swarms::new(config, 3);
            let mut t = Instant::now();
            for (b, p, event, complete, wait, from) in steps {
                let from = IpAddr::from([198, 51, 100, from]);
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
                if let Ok(response) = s.announce(t, &announce, from) {
                    proptest::prop_assert!(response.peers.iter().all(|e| e.peer != peer(p)));
                    let stats = s.stats(t, &bundle(b));
                    proptest::prop_assert_eq!(
                        (response.complete, response.incomplete),
                        (stats.complete, stats.incomplete)
                    );
                }
                check_counts(&s);
                for swarm in s.swarms.values() {
                    proptest::prop_assert!(swarm.per_client.0.values().all(|&n| n <= 3));
                }
                proptest::prop_assert!(s.per_client.0.values().all(|&n| n <= 5));
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
