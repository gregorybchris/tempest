//! The home relay (`protocol/relays.md`): a relay connection from the endpoint's own socket, which
//! lends this peer a relay address that peers who can't reach it directly dial instead.
//!
//! The relays to choose from are those configured, which Tempest ships none of (D-029), and, if
//! allowed, those trackers recommend. The endpoint opens a handshake with each at once and
//! registers with the first to answer, which is the one with the lowest latency. If it refuses,
//! the next to answer is tried. Once registered, packets the relay forwards are handed to the
//! [wrapping layer](crate::wrap). A lost relay connection is opened again, with a growing pause
//! between attempts that fail.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use quinn::{RecvStream, SendStream, TransportConfig, VarInt};
use tempest_protocol::message::GoodbyeReason;
use tempest_protocol::relay::RelayHost;
use tempest_protocol::relay::{RELAY_ALPN, Registered, RelayRejectReason};
use tempest_protocol::{Address, RelayDatagram, RelayIdentity, RelayMessage};
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio::time::{Instant, timeout};

use crate::endpoint::{Endpoint, HandshakeError, MIN_MTU};
use crate::stream::{ReadMessageError, read_relay_message, write_relay_message};

/// How often to send something on a quiet relay connection, so routers keep its mapping.
const KEEP_ALIVE: Duration = Duration::from_secs(15);
/// How long the relay connection may go without hearing from the relay before it is lost.
const IDLE_TIMEOUT: Duration = Duration::from_secs(45);
/// How long a handshake with a relay may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the relay may take to answer Register.
const REGISTER_TIMEOUT: Duration = Duration::from_secs(10);
/// How long to wait for path MTU discovery to make room for wrapped packets.
const MTU_WAIT: Duration = Duration::from_secs(3);
/// The most relays to open handshakes with at once.
const MAX_ATTEMPTS: usize = 8;
/// The most tracker-recommended relays remembered.
const MAX_RECOMMENDED: usize = 16;
/// The most relays taken from one tracker response, so one tracker can't push out the others'.
const MAX_RECOMMENDED_AT_ONCE: usize = 4;
/// How long a recommended relay is remembered after the last tracker response that named it.
/// Trackers are asked far more often, so this forgets only relays no tracker recommends now.
const RECOMMENDATION_LIFETIME: Duration = Duration::from_secs(60 * 60);
/// The pause after the first failed attempt, doubled after each further one.
const MIN_RETRY: Duration = Duration::from_secs(2);
/// The longest pause between attempts.
const MAX_RETRY: Duration = Duration::from_secs(5 * 60);
/// A registration that lasted this long resets the pause.
const STABLE: Duration = Duration::from_secs(60);
/// The largest Forward or Forwarded wrapper: a kind byte and an IPv6 address.
const MAX_WRAPPER: usize = 1 + 19;

/// The relays to pick a home relay from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelayChoice {
    /// Relays from the configuration, tried first.
    pub known: Vec<RelayIdentity>,
    /// Whether to also pick from the relays trackers recommend.
    pub from_trackers: bool,
}

/// What the home relay is doing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum RelayStatus {
    /// There is no relay to register with.
    #[default]
    Off,
    /// Connecting to relays or registering.
    Connecting,
    /// Registered: peers can reach this one at its relay address.
    Registered {
        /// The home relay.
        relay: RelayIdentity,
        /// The address the relay lends this peer.
        relay_address: Address,
        /// This peer's address as the relay sees it.
        observed_address: Address,
    },
    /// No relay could be registered with. Another attempt follows.
    Unavailable {
        /// Why, for each relay tried.
        failures: Vec<RelayFailure>,
    },
}

impl RelayStatus {
    /// The relay address, while registered.
    pub fn relay_address(&self) -> Option<Address> {
        match self {
            Self::Registered { relay_address, .. } => Some(*relay_address),
            _ => None,
        }
    }
}

/// Why registering with one relay failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{relay}: {error}")]
pub struct RelayFailure {
    /// The relay.
    pub relay: RelayIdentity,
    /// What went wrong.
    pub error: RelayError,
}

/// What went wrong with one relay.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RelayError {
    /// Its host name could not be resolved.
    #[error("cannot resolve its host: {0}")]
    Resolve(String),
    /// The endpoint has no socket for any of its addresses' families.
    #[error("no socket for its address family")]
    NoSocket,
    /// The handshake failed, or the relay's key was not the expected one.
    #[error("cannot connect: {0}")]
    Connect(String),
    /// The relay refused the registration with RelayReject.
    #[error("refused the registration ({}){}", reason_name(*.reason), detail(.message))]
    Rejected {
        /// The reason code.
        reason: RelayRejectReason,
        /// The relay's explanation.
        message: String,
    },
    /// The relay broke the relay protocol.
    #[error("broke the relay protocol: {0}")]
    Protocol(&'static str),
    /// The relay took too long.
    #[error("did not answer in time")]
    Timeout,
}

fn reason_name(reason: RelayRejectReason) -> &'static str {
    match reason {
        RelayRejectReason::NotAllowed => "not_allowed",
        RelayRejectReason::Full => "full",
        RelayRejectReason::Invalid => "invalid",
    }
}

fn detail(message: &str) -> String {
    match message {
        "" => String::new(),
        message => format!(": {message}"),
    }
}

/// A handle to an endpoint's home relay. Clones share it.
#[derive(Debug, Clone, Default)]
pub struct HomeRelay(Arc<Shared>);

#[derive(Debug)]
struct Shared {
    status: watch::Sender<RelayStatus>,
    choice: watch::Sender<Candidates>,
    stopped: watch::Sender<bool>,
    running: AtomicBool,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            status: watch::channel(RelayStatus::Off).0,
            choice: watch::channel(Candidates::default()).0,
            stopped: watch::channel(false).0,
            running: AtomicBool::new(false),
        }
    }
}

/// The relays to choose from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Candidates {
    choice: RelayChoice,
    /// The relays trackers recommended, each with when one last did, in the order first heard.
    recommended: Vec<(RelayIdentity, Instant)>,
}

impl Candidates {
    /// Every relay to try, configured ones first, each once.
    fn list(&self) -> Vec<RelayIdentity> {
        let recommended = (self.recommended.iter())
            .map(|(relay, _)| relay)
            .filter(|_| self.choice.from_trackers);
        let mut list: Vec<RelayIdentity> = Vec::new();
        for relay in self.choice.known.iter().chain(recommended) {
            if !list.contains(relay) {
                list.push(relay.clone());
            }
        }
        list
    }

    /// Takes in the first few of a tracker's `relays` at `now`, and forgets the relays no
    /// tracker has recommended for [`RECOMMENDATION_LIFETIME`]. Beyond [`MAX_RECOMMENDED`], a
    /// newcomer replaces the relay recommended longest ago. Returns whether the relays changed.
    fn recommend(&mut self, relays: &[RelayIdentity], now: Instant) -> bool {
        let before = self.recommended.len();
        self.recommended
            .retain(|(_, at)| now.duration_since(*at) < RECOMMENDATION_LIFETIME);
        let mut changed = self.recommended.len() != before;
        for relay in relays.iter().take(MAX_RECOMMENDED_AT_ONCE) {
            if let Some((_, at)) = self.recommended.iter_mut().find(|(r, _)| r == relay) {
                *at = now;
                continue;
            }
            if self.recommended.len() >= MAX_RECOMMENDED {
                let oldest = (self.recommended.iter().enumerate())
                    .min_by_key(|(_, (_, at))| *at)
                    .map(|(i, _)| i);
                if let Some(oldest) = oldest {
                    self.recommended.remove(oldest);
                }
            }
            self.recommended.push((relay.clone(), now));
            changed = true;
        }
        changed
    }
}

impl HomeRelay {
    /// What the home relay is doing now.
    pub fn status(&self) -> RelayStatus {
        self.0.status.borrow().clone()
    }

    /// Follows what the home relay is doing.
    pub fn subscribe(&self) -> watch::Receiver<RelayStatus> {
        self.0.status.subscribe()
    }

    /// This peer's relay address, while registered.
    pub fn address(&self) -> Option<Address> {
        self.0.status.borrow().relay_address()
    }

    /// Adds relays a tracker recommends, used if the [`RelayChoice`] allows it. Relays no
    /// tracker has recommended for an hour are forgotten as further recommendations arrive.
    pub fn recommend(&self, relays: &[RelayIdentity]) {
        let now = Instant::now();
        (self.0.choice).send_if_modified(|candidates| candidates.recommend(relays, now));
    }

    /// Sets the relays to choose from, and starts the home relay's task if it isn't running.
    pub(crate) fn choose(&self, endpoint: &Endpoint, choice: RelayChoice) {
        let wanted = !choice.known.is_empty() || choice.from_trackers;
        self.0
            .choice
            .send_modify(|candidates| candidates.choice = choice);
        if wanted && !*self.0.stopped.borrow() && !self.0.running.swap(true, Ordering::SeqCst) {
            tokio::spawn(run(endpoint.clone(), self.0.clone()));
        }
    }

    /// Leaves the home relay for good, as the endpoint closes.
    pub(crate) fn stop(&self) {
        self.0.stopped.send_replace(true);
    }
}

/// Why a registration ended.
enum End {
    /// The relay connection was lost or closed.
    Lost,
    /// The relay is no longer one to choose from.
    Unlisted,
    /// The endpoint is closing.
    Stopped,
}

/// The home relay's task: registers, and registers again whenever the registration ends.
async fn run(endpoint: Endpoint, shared: Arc<Shared>) {
    let mut choice = shared.choice.subscribe();
    let mut stopped = shared.stopped.subscribe();
    let mut retry = MIN_RETRY;
    while !*stopped.borrow() {
        let candidates = choice.borrow_and_update().list();
        if candidates.is_empty() {
            shared.status.send_replace(RelayStatus::Off);
            tokio::select! {
                _ = choice.changed() => continue,
                _ = stopped.wait_for(|stop| *stop) => break,
            }
        }
        shared.status.send_replace(RelayStatus::Connecting);
        tracing::debug!(
            "registering with a home relay; {} candidates",
            candidates.len()
        );
        let attempt = tokio::select! {
            attempt = register(&endpoint, &candidates) => attempt,
            _ = stopped.wait_for(|stop| *stop) => break,
        };
        match attempt {
            Ok(registration) => {
                let since = Instant::now();
                tracing::info!("registered with relay {}", registration.relay);
                let end = registration
                    .serve(&endpoint, &shared, &mut choice, &mut stopped)
                    .await;
                match &end {
                    End::Lost => tracing::warn!("lost the home relay after {:?}", since.elapsed()),
                    End::Unlisted => tracing::info!("left the home relay: no longer listed"),
                    End::Stopped => {}
                }
                match end {
                    End::Stopped => break,
                    // Another relay may be listed: no reason to wait.
                    End::Unlisted => continue,
                    End::Lost if since.elapsed() >= STABLE => retry = MIN_RETRY,
                    End::Lost => {}
                }
                shared.status.send_replace(RelayStatus::Connecting);
            }
            Err(failures) => {
                let reasons: Vec<String> = failures.iter().map(ToString::to_string).collect();
                tracing::warn!(
                    "cannot register with a relay ({}); retrying in {retry:?}",
                    reasons.join("; ")
                );
                shared
                    .status
                    .send_replace(RelayStatus::Unavailable { failures });
            }
        }
        tokio::select! {
            () = tokio::time::sleep(retry) => {}
            // A new list is worth trying at once.
            _ = choice.changed() => {}
            _ = stopped.wait_for(|stop| *stop) => break,
        }
        retry = (retry * 2).min(MAX_RETRY);
    }
    shared.status.send_replace(RelayStatus::Off);
    shared.running.store(false, Ordering::SeqCst);
}

/// An open registration with a relay.
struct Registration {
    relay: RelayIdentity,
    addr: SocketAddr,
    conn: quinn::Connection,
    send: SendStream,
    recv: RecvStream,
    registered: Registered,
}

/// Opens handshakes with every candidate at once and registers with the first to answer that
/// accepts. Fails with each relay's failure if none does.
async fn register(
    endpoint: &Endpoint,
    candidates: &[RelayIdentity],
) -> Result<Registration, Vec<RelayFailure>> {
    let mut attempts = JoinSet::new();
    for relay in candidates.iter().take(MAX_ATTEMPTS).cloned() {
        let endpoint = endpoint.clone();
        attempts.spawn(async move {
            let opened = timeout(CONNECT_TIMEOUT, handshake(&endpoint, &relay))
                .await
                .unwrap_or(Err(RelayError::Timeout));
            (relay, opened)
        });
    }
    let mut failures = Vec::new();
    while let Some(joined) = attempts.join_next().await {
        // A panicked attempt is a bug, but other relays may still answer.
        let Ok((relay, opened)) = joined else {
            continue;
        };
        let registered = match opened {
            Ok((addr, conn)) => {
                let opening = timeout(REGISTER_TIMEOUT, send_register(&conn)).await;
                let result = opening.unwrap_or(Err(RelayError::Timeout));
                if result.is_err() {
                    close(&conn, GoodbyeReason::Done);
                }
                result.map(|(send, recv, registered)| Registration {
                    relay: relay.clone(),
                    addr,
                    conn,
                    send,
                    recv,
                    registered,
                })
            }
            Err(error) => Err(error),
        };
        match registered {
            // Dropping the other attempts closes their connections.
            Ok(registration) => return Ok(registration),
            Err(error) => failures.push(RelayFailure { relay, error }),
        }
    }
    Err(failures)
}

/// Completes a handshake with `relay` from the endpoint's socket, checking its key.
async fn handshake(
    endpoint: &Endpoint,
    relay: &RelayIdentity,
) -> Result<(SocketAddr, quinn::Connection), RelayError> {
    let addrs: Vec<SocketAddr> = match &relay.host {
        RelayHost::Ip(ip) => vec![SocketAddr::new(*ip, relay.port)],
        RelayHost::Domain(name) => tokio::net::lookup_host((name.as_str(), relay.port))
            .await
            .map_err(|error| RelayError::Resolve(error.to_string()))?
            .collect(),
    };
    let addr = addrs
        .into_iter()
        .map(|addr| Address::from(addr).socket_addr())
        .find(|&addr| endpoint.socket(addr).is_some())
        .ok_or(RelayError::NoSocket)?;
    let connection = endpoint
        .connect(addr, RELAY_ALPN, Some(relay.key), transport())
        .await
        .map_err(|error| match error {
            HandshakeError::UnexpectedPeer(_) => {
                RelayError::Connect("the relay's key is not the one in its identity".into())
            }
            error => RelayError::Connect(error.to_string()),
        })?;
    Ok((addr, connection.quic().clone()))
}

/// Sends Register and reads the answer. Once registered, waits a moment for path MTU discovery
/// to make room for wrapped peer packets, so the relay address works when published.
async fn send_register(
    conn: &quinn::Connection,
) -> Result<(SendStream, RecvStream, Registered), RelayError> {
    let lost = |_| RelayError::Connect("the connection was lost".into());
    let (mut send, mut recv) = conn.open_bi().await.map_err(lost)?;
    write_relay_message(&mut send, &RelayMessage::Register)
        .await
        .map_err(|_| RelayError::Connect("the connection was lost".into()))?;
    let registered = match read_relay_message(&mut recv).await {
        Ok(Some(RelayMessage::Registered(registered))) => registered,
        Ok(Some(RelayMessage::RelayReject(reject))) => {
            return Err(RelayError::Rejected {
                reason: reject.reason,
                message: reject.message,
            });
        }
        Ok(Some(RelayMessage::Register)) => {
            return Err(RelayError::Protocol("it sent Register"));
        }
        Ok(None) => return Err(RelayError::Protocol("it ended the control stream")),
        Err(ReadMessageError::Read(_)) => {
            return Err(RelayError::Connect("the connection was lost".into()));
        }
        Err(_) => return Err(RelayError::Protocol("its answer did not decode")),
    };
    let room = usize::from(MIN_MTU) + MAX_WRAPPER;
    let _ = timeout(MTU_WAIT, async {
        while conn.max_datagram_size().is_none_or(|size| size < room) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    Ok((send, recv, registered))
}

impl Registration {
    /// Publishes the relay address and carries forwarded packets to the wrapping layer until
    /// the registration ends.
    async fn serve(
        self,
        endpoint: &Endpoint,
        shared: &Shared,
        choice: &mut watch::Receiver<Candidates>,
        stopped: &mut watch::Receiver<bool>,
    ) -> End {
        let Self {
            relay,
            addr,
            conn,
            send: _send,
            mut recv,
            registered,
        } = self;
        let Some(socket) = endpoint.socket(addr) else {
            close(&conn, GoodbyeReason::Done);
            return End::Lost;
        };
        socket.wrap.set_link(addr, conn.clone());
        let status = |registered: &Registered| RelayStatus::Registered {
            relay: relay.clone(),
            relay_address: registered.relay_address,
            observed_address: registered.observed_address,
        };
        shared.status.send_replace(status(&registered));

        let forward = async {
            while let Ok(datagram) = conn.read_datagram().await {
                // Anything but Forwarded is dropped.
                if let Ok(RelayDatagram::Forwarded { from, packet }) =
                    RelayDatagram::decode(&datagram)
                {
                    socket.wrap.deliver(addr, from, packet);
                }
            }
        };
        let control = async {
            loop {
                match read_relay_message(&mut recv).await {
                    // The relay saw this peer's address change. The relay address must stay
                    // the same, since it may already be published.
                    Ok(Some(RelayMessage::Registered(update)))
                        if update.relay_address == registered.relay_address =>
                    {
                        shared.status.send_replace(status(&update));
                    }
                    // Nothing more is defined for the relay to send.
                    Ok(None) => std::future::pending().await,
                    Err(ReadMessageError::Read(_)) => return,
                    Ok(Some(_)) | Err(_) => {
                        close(&conn, GoodbyeReason::ProtocolViolation);
                        return;
                    }
                }
            }
        };
        let unlisted = async {
            while choice.changed().await.is_ok() {
                if !choice.borrow_and_update().list().contains(&relay) {
                    return;
                }
            }
            std::future::pending::<()>().await;
        };
        let end = tokio::select! {
            () = forward => End::Lost,
            () = control => End::Lost,
            () = unlisted => {
                close(&conn, GoodbyeReason::Done);
                End::Unlisted
            }
            _ = stopped.wait_for(|stop| *stop) => {
                close(&conn, GoodbyeReason::ShuttingDown);
                End::Stopped
            }
        };
        socket.wrap.clear_link(&conn);
        end
    }
}

/// The relay connection's transport: datagrams, frequent keep-alives, and the peer
/// connections' MTU rules.
fn transport() -> Arc<TransportConfig> {
    let mut transport = TransportConfig::default();
    transport
        .initial_mtu(MIN_MTU)
        .min_mtu(MIN_MTU)
        .max_concurrent_bidi_streams(VarInt::from_u32(0))
        .max_concurrent_uni_streams(VarInt::from_u32(0))
        .datagram_receive_buffer_size(Some(1 << 20))
        .datagram_send_buffer_size(1 << 20)
        .keep_alive_interval(Some(KEEP_ALIVE))
        .max_idle_timeout(Some(
            IDLE_TIMEOUT.try_into().expect("idle timeout below 2^62 µs"),
        ));
    Arc::new(transport)
}

fn close(conn: &quinn::Connection, reason: GoodbyeReason) {
    conn.close(VarInt::from_u32(reason as u32), &[]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relay(n: u8) -> RelayIdentity {
        format!("{}@198.51.100.{n}", "ab".repeat(32))
            .parse()
            .unwrap()
    }

    #[test]
    fn configured_relays_come_first_and_recommendations_need_consent() {
        let mut candidates = Candidates {
            choice: RelayChoice {
                known: vec![relay(1), relay(2)],
                from_trackers: false,
            },
            recommended: Vec::new(),
        };
        candidates.recommend(&[relay(2), relay(3)], Instant::now());
        assert_eq!(candidates.list(), [relay(1), relay(2)]);
        candidates.choice.from_trackers = true;
        assert_eq!(candidates.list(), [relay(1), relay(2), relay(3)]);
    }

    fn recommended(candidates: &Candidates) -> Vec<u8> {
        let relays = candidates.recommended.iter();
        let host = |relay: &RelayIdentity| match &relay.host {
            RelayHost::Ip(std::net::IpAddr::V4(ip)) => ip.octets()[3],
            host => panic!("{host:?}"),
        };
        relays.map(|(relay, _)| host(relay)).collect()
    }

    #[test]
    fn recommendations_are_kept_once_and_capped() {
        let mut candidates = Candidates::default();
        let start = Instant::now();
        let relays: Vec<_> = (0..40).map(relay).collect();
        assert!(candidates.recommend(&relays[..3], start));
        assert!(!candidates.recommend(&relays[..3], start));
        // A response adds only its first few.
        candidates.recommend(&relays[3..], start);
        assert_eq!(recommended(&candidates), [0, 1, 2, 3, 4, 5, 6]);
        for (n, chunk) in relays[7..].chunks(4).enumerate() {
            candidates.recommend(chunk, start + Duration::from_secs(n as u64 + 1));
        }
        assert_eq!(candidates.recommended.len(), MAX_RECOMMENDED);
    }

    #[test]
    fn newcomers_replace_the_relays_recommended_longest_ago() {
        let mut candidates = Candidates::default();
        let start = Instant::now();
        let at = |secs| start + Duration::from_secs(secs);
        for n in 0..4 {
            let batch: Vec<_> = (n * 4..n * 4 + 4).map(relay).collect();
            candidates.recommend(&batch, at(u64::from(n)));
        }
        assert_eq!(candidates.recommended.len(), MAX_RECOMMENDED);
        // Relays 0 to 3 are recommended again, so 4 to 7 are the oldest.
        candidates.recommend(&(0..4).map(relay).collect::<Vec<_>>(), at(10));
        candidates.recommend(&[relay(50), relay(51)], at(11));
        let kept = recommended(&candidates);
        assert!(kept.contains(&0) && kept.contains(&50) && kept.contains(&51));
        assert!(!kept.contains(&4) && !kept.contains(&5));
    }

    #[test]
    fn relays_no_tracker_recommends_are_forgotten() {
        let mut candidates = Candidates::default();
        let start = Instant::now();
        candidates.recommend(&[relay(1), relay(2)], start);
        let later = start + RECOMMENDATION_LIFETIME - Duration::from_secs(1);
        candidates.recommend(&[relay(2)], later);
        assert!(candidates.recommend(&[], start + RECOMMENDATION_LIFETIME));
        assert_eq!(recommended(&candidates), [2]);
        assert!(candidates.recommend(&[], later + RECOMMENDATION_LIFETIME));
        assert!(candidates.recommended.is_empty());
    }

    #[test]
    fn failures_read_well() {
        let failure = RelayFailure {
            relay: relay(1),
            error: RelayError::Rejected {
                reason: RelayRejectReason::NotAllowed,
                message: "members only".into(),
            },
        };
        assert!(
            failure.to_string().ends_with(
                "@198.51.100.1:4775: refused the registration (not_allowed): members only"
            ),
            "{failure}"
        );
    }
}
