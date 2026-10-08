//! Finding peers through trackers (`protocol/trackers.md`).
//!
//! A [`TrackerClient`] makes single requests, including those about a tracker's listings (see
//! [`crate::publish`]). An [`Announcer`] keeps one bundle announced to one tracker for as long as
//! the bundle runs: `started` when it begins, `update` every interval, `completed` when the
//! bundle completes and `stopped` when it stops, handing the peers each response lists to the
//! bundle. Its announces carry `listing` while the user chose to list the bundle there.

use std::time::Duration;

use rustls_platform_verifier::BuilderVerifierExt;
use serde::de::DeserializeOwned;
use tempest_protocol::message::PeerRecord;
use tempest_protocol::tracker::{
    self, AnnounceEvent, AnnounceRequest, AnnounceResponse, BundleStats, ErrorResponse,
    ListingRequest,
};
use tempest_protocol::{Address, BundleId, PeerId};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::addresses::is_public;
use crate::driver::{BundleHandle, PeerSink, Status};
use crate::home::HomeRelay;
use crate::portmap::PortMapping;

/// The largest response body read from a tracker.
pub const MAX_RESPONSE: usize = 1 << 20;

/// Why a tracker request failed.
#[derive(Debug, thiserror::Error)]
pub enum TrackerError {
    /// TLS could not be set up with the platform's certificate verifier.
    #[error("cannot set up TLS: {0}")]
    Tls(#[from] rustls::Error),
    /// The request did not complete, or the URL is not one HTTP can use.
    #[error("cannot reach the tracker: {0}")]
    Http(#[from] reqwest::Error),
    /// The tracker answered with an error status.
    #[error("the tracker answered {status}{}", describe(.error))]
    Refused {
        /// The HTTP status.
        status: u16,
        /// The error body, if it was one.
        error: Option<ErrorResponse>,
    },
    /// The body of a successful response is not what the endpoint returns.
    #[error("the tracker's response is malformed: {0}")]
    Malformed(#[from] serde_json::Error),
    /// The response is larger than [`MAX_RESPONSE`].
    #[error("the tracker's response is larger than {MAX_RESPONSE} bytes")]
    TooLarge,
    /// The tracker's URL is not an HTTP or HTTPS URL with a host.
    #[error("the tracker's URL is not an HTTP or HTTPS URL with a host")]
    NotHttp,
    /// The tracker has no sign-in or listings: it only announces.
    #[error("the tracker doesn't take listings")]
    NoListings,
}

fn describe(error: &Option<ErrorResponse>) -> String {
    match error {
        Some(error) if error.message.is_empty() => format!(" ({})", error.error),
        Some(error) => format!(" ({}: {})", error.error, error.message),
        None => String::new(),
    }
}

impl TrackerError {
    /// How long the tracker asked the client to wait before trying again, if it did.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::Refused {
                error: Some(error), ..
            } => error.retry_after_secs.map(Duration::from_secs),
            _ => None,
        }
    }
}

/// Makes requests to trackers over HTTP or HTTPS. Clones share connections.
#[derive(Debug, Clone)]
pub struct TrackerClient {
    pub(crate) http: reqwest::Client,
}

impl TrackerClient {
    /// A client whose requests give up after `timeout`. HTTPS certificates are checked against
    /// the platform's trust store.
    pub fn new(timeout: Duration) -> Result<Self, TrackerError> {
        let tls = rustls::ClientConfig::builder_with_provider(crate::tls::provider())
            .with_safe_default_protocol_versions()?
            .with_platform_verifier()?
            .with_no_client_auth();
        let http = reqwest::Client::builder()
            .tls_backend_preconfigured(tls)
            .timeout(timeout)
            .user_agent(concat!("tempest/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self { http })
    }

    /// Announces to the tracker at `tracker`, a base URL such as `https://tracker.example.org`.
    pub async fn announce(
        &self,
        tracker: &str,
        request: &AnnounceRequest,
    ) -> Result<AnnounceResponse, TrackerError> {
        let body = serde_json::to_vec(request).expect("an announce serializes");
        let request = self
            .http
            .post(tracker::announce_url(tracker))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body);
        self.exchange(request).await
    }

    /// Asks the tracker at `tracker` for a bundle's swarm counts.
    pub async fn stats(
        &self,
        tracker: &str,
        bundle: &BundleId,
    ) -> Result<BundleStats, TrackerError> {
        let request = self.http.get(tracker::bundle_stats_url(tracker, bundle));
        self.exchange(request).await
    }

    pub(crate) async fn exchange<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, TrackerError> {
        let mut response = request.send().await?;
        let status = response.status();
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if body.len() + chunk.len() > MAX_RESPONSE {
                return Err(TrackerError::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            return Err(TrackerError::Refused {
                status: status.as_u16(),
                error: serde_json::from_slice(&body).ok(),
            });
        }
        Ok(serde_json::from_slice(&body)?)
    }
}

/// Of `addresses`, the ones that may be reachable from the internet, which are the only ones a
/// client should announce: no loopback, private, link-local, shared (carrier-grade NAT) or
/// unique local addresses.
pub fn public_addresses(addresses: &[Address]) -> Vec<Address> {
    addresses
        .iter()
        .copied()
        .filter(|address| is_public(address.ip()))
        .collect()
}

/// What a peer tells trackers about itself, for every bundle it announces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnounceConfig {
    /// This peer's peer ID.
    pub peer: PeerId,
    /// The UDP port of this peer's QUIC endpoint.
    pub port: u16,
    /// Direct addresses beyond the one the tracker observes. See [`public_addresses`]. The
    /// external address comes from the bundle's [port mapping](BundleHandle::port_mapping), the
    /// relay address from its [home relay](BundleHandle::home_relay).
    pub addrs: Vec<Address>,
    /// How many peers to ask for.
    pub num_want: u32,
    /// The shortest interval obeyed, whatever a tracker asks for.
    pub min_interval: Duration,
    /// How long to wait after the first failure that came without `retry_after_secs`. Each
    /// further failure doubles it, up to `max_retry`.
    pub retry: Duration,
    /// The longest wait after a failure without `retry_after_secs`.
    pub max_retry: Duration,
}

impl AnnounceConfig {
    /// Announces for `peer` on `port` with the default settings.
    pub fn new(peer: PeerId, port: u16) -> Self {
        Self {
            peer,
            port,
            addrs: Vec::new(),
            num_want: tracker::DEFAULT_NUM_WANT,
            min_interval: Duration::from_secs(30),
            retry: Duration::from_secs(15),
            max_retry: Duration::from_secs(30 * 60),
        }
    }
}

/// How announcing to one tracker is going.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackerState {
    /// The number of announces the tracker accepted.
    pub announces: u64,
    /// The swarm's complete peers, as of the last accepted announce.
    pub complete: u64,
    /// The swarm's incomplete peers, as of the last accepted announce.
    pub incomplete: u64,
    /// The number of peers the last accepted announce listed.
    pub peers: usize,
    /// Why the last announce failed, if it did.
    pub error: Option<String>,
}

/// Keeps one bundle announced to one tracker, until the bundle stops, [`Announcer::stop`] is
/// called or the announcer is dropped.
#[derive(Debug)]
pub struct Announcer {
    /// How it is going.
    pub state: watch::Receiver<TrackerState>,
    tracker: String,
    listing: watch::Sender<Option<ListingRequest>>,
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl Announcer {
    /// Starts announcing `bundle` to the tracker at `tracker`, with `listing` if the user chose
    /// to list it there (see [`Identity::listing`](crate::Identity::listing)). Must be called
    /// within a tokio runtime.
    pub fn spawn(
        client: TrackerClient,
        tracker: String,
        config: AnnounceConfig,
        bundle: &BundleHandle,
        listing: Option<ListingRequest>,
    ) -> Self {
        let (state, state_receiver) = watch::channel(TrackerState::default());
        let (stop, stop_receiver) = watch::channel(false);
        let (listing, listing_receiver) = watch::channel(listing);
        let task = AnnounceTask {
            client,
            tracker: tracker.clone(),
            config,
            listing: listing_receiver,
            bundle: bundle.bundle_id(),
            status: bundle.status.clone(),
            peers: bundle.peer_sink(),
            home_relay: bundle.home_relay().clone(),
            port_mapping: bundle.port_mapping().clone(),
            state,
        };
        Self {
            state: state_receiver,
            tracker,
            listing,
            stop,
            task: tokio::spawn(task.run(stop_receiver)),
        }
    }

    /// The tracker's URL.
    pub fn tracker(&self) -> &str {
        &self.tracker
    }

    /// Starts or stops asking the tracker to list the bundle. A new request goes out with the
    /// next update, as soon as the shortest interval allows.
    pub fn set_listing(&self, listing: Option<ListingRequest>) {
        self.listing.send_if_modified(|current| {
            let changed = *current != listing;
            *current = listing;
            changed
        });
    }

    /// Sends `stopped` (unless the tracker never heard `started`) and stops announcing.
    pub async fn stop(self) {
        self.stop.send_replace(true);
        self.finished().await;
    }

    /// Waits until the announcer is done: after the bundle stopped and `stopped` was sent.
    pub async fn finished(self) {
        // A panic in the task would be a bug; nothing to clean up either way.
        let _ = self.task.await;
    }
}

/// When to announce what: the announce rules, apart from any I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Schedule {
    /// An event that must reach the tracker, if one is waiting.
    pending: Option<AnnounceEvent>,
    /// Whether the tracker accepted `started`.
    started: bool,
    /// When the next `update` is due.
    next_update: Instant,
    /// Nothing is sent before this, after a failure.
    not_before: Instant,
    /// Failures in a row.
    failures: u32,
    /// When the tracker last accepted an announce.
    succeeded_at: Option<Instant>,
}

impl Schedule {
    fn new(now: Instant) -> Self {
        Self {
            pending: Some(AnnounceEvent::Started),
            started: false,
            next_update: now,
            not_before: now,
            failures: 0,
            succeeded_at: None,
        }
    }

    /// The event to send next.
    fn event(&self) -> AnnounceEvent {
        self.pending.unwrap_or(AnnounceEvent::Update)
    }

    /// When to send it.
    fn due(&self) -> Instant {
        match self.pending {
            Some(_) => self.not_before,
            None => self.next_update.max(self.not_before),
        }
    }

    fn succeeded(&mut self, now: Instant, interval: Duration) {
        self.started |= self.pending == Some(AnnounceEvent::Started);
        self.pending = None;
        self.failures = 0;
        self.succeeded_at = Some(now);
        self.next_update = now + interval;
        self.not_before = now;
    }

    fn failed(&mut self, now: Instant, retry_after: Option<Duration>, config: &AnnounceConfig) {
        let backoff = || {
            let doublings = self.failures.min(16);
            config
                .retry
                .saturating_mul(1 << doublings)
                .min(config.max_retry)
        };
        let wait = retry_after.unwrap_or_else(backoff);
        self.failures += 1;
        self.not_before = now + wait;
    }

    /// Something the tracker should hear changed, such as this peer's relay or external
    /// address: it goes out with the next update, as soon as `min_interval` since the last
    /// announce allows.
    fn changed(&mut self, now: Instant, min_interval: Duration) {
        let earliest = self.succeeded_at.map_or(now, |at| at + min_interval);
        self.next_update = self.next_update.min(earliest.max(now));
    }

    /// The bundle became complete. A `started` still waiting will say so itself.
    fn completed(&mut self) {
        if self.started {
            self.pending = Some(AnnounceEvent::Completed);
        }
    }

    /// Whether `stopped` should be sent at `now`: only to a tracker that heard `started`, and
    /// not while it asked us to wait.
    fn may_stop(&self, now: Instant) -> bool {
        self.started && self.not_before <= now
    }
}

/// The announcer's task.
struct AnnounceTask {
    client: TrackerClient,
    tracker: String,
    config: AnnounceConfig,
    listing: watch::Receiver<Option<ListingRequest>>,
    bundle: BundleId,
    status: watch::Receiver<Status>,
    peers: PeerSink,
    home_relay: HomeRelay,
    port_mapping: PortMapping,
    state: watch::Sender<TrackerState>,
}

impl AnnounceTask {
    async fn run(mut self, mut stop: watch::Receiver<bool>) {
        let mut schedule = Schedule::new(Instant::now());
        let mut complete = self.status.borrow_and_update().complete;
        let mut relay = self.home_relay.subscribe();
        let mut relay_address = relay.borrow_and_update().relay_address();
        let mut mapping = self.port_mapping.subscribe();
        let mut external = mapping.borrow_and_update().address();
        loop {
            tokio::select! {
                () = tokio::time::sleep_until(schedule.due()) => {
                    let event = schedule.event();
                    let result = self.announce(event, complete).await;
                    let now = Instant::now();
                    match result {
                        Ok(interval) => schedule.succeeded(now, interval),
                        Err(retry_after) => schedule.failed(now, retry_after, &self.config),
                    }
                }
                changed = self.status.changed() => {
                    // The bundle stopped.
                    if changed.is_err() {
                        break;
                    }
                    if self.status.borrow_and_update().complete && !complete {
                        complete = true;
                        schedule.completed();
                    }
                }
                Ok(()) = relay.changed() => {
                    let address = relay.borrow_and_update().relay_address();
                    if address != relay_address {
                        relay_address = address;
                        schedule.changed(Instant::now(), self.config.min_interval);
                    }
                }
                Ok(()) = mapping.changed() => {
                    let address = mapping.borrow_and_update().address();
                    if address != external {
                        external = address;
                        schedule.changed(Instant::now(), self.config.min_interval);
                    }
                }
                Ok(()) = self.listing.changed() => {
                    schedule.changed(Instant::now(), self.config.min_interval);
                }
                // Asked to stop, or the announcer was dropped.
                _ = stop.changed() => break,
            }
        }
        if schedule.may_stop(Instant::now()) {
            let _ = self.announce(AnnounceEvent::Stopped, complete).await;
        }
    }

    /// Sends one announce and delivers the peers it lists. Returns the interval until the next
    /// one, or when to retry if the tracker said.
    async fn announce(
        &mut self,
        event: AnnounceEvent,
        complete: bool,
    ) -> Result<Duration, Option<Duration>> {
        let request = AnnounceRequest {
            bundle: self.bundle,
            peer: self.config.peer,
            port: self.config.port,
            addrs: self.port_mapping.advertise(&self.config.addrs),
            relay: self.home_relay.address(),
            event,
            complete,
            num_want: self.config.num_want,
            listing: *self.listing.borrow(),
        };
        let response = match self.client.announce(&self.tracker, &request).await {
            Ok(response) => response,
            Err(error) => {
                let retry_after = error.retry_after();
                self.state
                    .send_modify(|state| state.error = Some(error.to_string()));
                return Err(retry_after);
            }
        };
        self.home_relay.recommend(&response.relays);
        self.state.send_modify(|state| {
            state.announces += 1;
            state.complete = response.complete;
            state.incomplete = response.incomplete;
            state.peers = response.peers.len();
            state.error = None;
        });
        let records: Vec<PeerRecord> = response
            .peers
            .into_iter()
            .map(|entry| PeerRecord {
                peer_id: entry.peer,
                addresses: entry.addrs,
                relay: entry.relay,
            })
            .collect();
        if !records.is_empty() {
            self.peers.add_peers(records).await;
        }
        let interval = Duration::from_secs(response.interval_secs);
        Ok(interval.max(self.config.min_interval))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> AnnounceConfig {
        AnnounceConfig::new(PeerId::new([1; 32]), 4773)
    }

    #[test]
    fn only_public_addresses_are_announced() {
        let all: Vec<Address> = [
            "203.0.113.5:1",
            "8.8.8.8:1",
            "10.0.0.1:1",
            "172.16.0.1:1",
            "192.168.1.2:1",
            "127.0.0.1:1",
            "169.254.0.1:1",
            "100.64.0.1:1",
            "0.0.0.0:1",
            "[2606:4700::1]:1",
            "[fd00::1]:1",
            "[fe80::1]:1",
            "[::1]:1",
            "[2001:db8::1]:1",
        ]
        .iter()
        .map(|text| text.parse().unwrap())
        .collect();
        let public: Vec<String> = public_addresses(&all)
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(public, ["8.8.8.8:1", "[2606:4700::1]:1"]);
    }

    #[test]
    fn starts_then_updates_every_interval() {
        let t = Instant::now();
        let mut s = Schedule::new(t);
        assert_eq!((s.event(), s.due()), (AnnounceEvent::Started, t));
        s.succeeded(t, Duration::from_secs(300));
        assert_eq!(s.event(), AnnounceEvent::Update);
        assert_eq!(s.due(), t + Duration::from_secs(300));
        assert!(s.may_stop(t));
    }

    #[test]
    fn completion_is_announced_at_once_after_started() {
        let t = Instant::now();
        let mut s = Schedule::new(t);
        s.succeeded(t, Duration::from_secs(300));
        let later = t + Duration::from_secs(5);
        s.completed();
        assert_eq!(s.event(), AnnounceEvent::Completed);
        assert_eq!(s.due(), t);
        s.succeeded(later, Duration::from_secs(300));
        assert_eq!(s.event(), AnnounceEvent::Update);
    }

    #[test]
    fn a_new_relay_address_is_announced_soon() {
        let t = Instant::now();
        let min_interval = Duration::from_secs(30);
        let mut s = Schedule::new(t);
        s.succeeded(t, Duration::from_secs(1800));
        s.changed(t + Duration::from_secs(5), min_interval);
        assert_eq!(s.due(), t + min_interval);
        s.succeeded(t + min_interval, Duration::from_secs(1800));
        let later = t + Duration::from_secs(600);
        s.changed(later, min_interval);
        assert_eq!(s.due(), later);
    }

    #[test]
    fn completion_before_started_rides_on_started() {
        let t = Instant::now();
        let mut s = Schedule::new(t);
        s.completed();
        assert_eq!(s.event(), AnnounceEvent::Started);
        assert!(!s.may_stop(t));
    }

    #[test]
    fn failures_back_off_and_honor_retry_after() {
        let config = config();
        let t = Instant::now();
        let mut s = Schedule::new(t);
        s.failed(t, None, &config);
        assert_eq!(s.due(), t + Duration::from_secs(15));
        assert_eq!(s.event(), AnnounceEvent::Started);
        s.failed(t, None, &config);
        assert_eq!(s.due(), t + Duration::from_secs(30));
        for _ in 0..20 {
            s.failed(t, None, &config);
        }
        assert_eq!(s.due(), t + config.max_retry);

        s.failed(t, Some(Duration::from_secs(7)), &config);
        assert_eq!(s.due(), t + Duration::from_secs(7));
        s.succeeded(t, Duration::from_secs(300));
        assert_eq!(s.due(), t + Duration::from_secs(300));
    }

    #[test]
    fn a_failed_update_waits_for_its_retry_and_blocks_stopped() {
        let config = config();
        let t = Instant::now();
        let mut s = Schedule::new(t);
        s.succeeded(t, Duration::from_secs(60));
        let update = t + Duration::from_secs(60);
        s.failed(update, Some(Duration::from_secs(120)), &config);
        assert_eq!(s.event(), AnnounceEvent::Update);
        assert_eq!(s.due(), update + Duration::from_secs(120));
        assert!(!s.may_stop(update));
        assert!(s.may_stop(update + Duration::from_secs(120)));
    }

    #[test]
    fn refusals_carry_retry_after() {
        let error = TrackerError::Refused {
            status: 429,
            error: Some(ErrorResponse {
                error: tracker::ErrorCode::RateLimited,
                message: "slow down".into(),
                retry_after_secs: Some(42),
            }),
        };
        assert_eq!(error.retry_after(), Some(Duration::from_secs(42)));
        assert_eq!(
            error.to_string(),
            "the tracker answered 429 (rate_limited: slow down)"
        );
        let bare = TrackerError::Refused {
            status: 502,
            error: None,
        };
        assert_eq!(bare.retry_after(), None);
        assert_eq!(bare.to_string(), "the tracker answered 502");
    }
}
