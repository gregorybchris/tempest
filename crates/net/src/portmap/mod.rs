//! Port mapping (`protocol/connections.md`, Getting through routers): asking the router to
//! forward the IPv4 socket's port, so peers outside the home network can dial this one
//! directly.
//!
//! PCP is tried first, at the default gateway, falling back to NAT-PMP if the router only speaks
//! that, then UPnP-IGD. The mapping is renewed halfway through its lifetime and removed when the
//! endpoint closes or mapping is turned off. Failing is normal on many networks: it is reported
//! in the [status](PortMappingStatus) and tried again later, and nothing else depends on it.
//!
//! IPv6 needs no mapping, since it has no NAT; a firewall in the way is left alone.

mod lease;
mod pcp;
mod upnp;
pub mod wire;

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use tempest_protocol::Address;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep_until, timeout};

use crate::addresses::is_public;
use lease::{Backoff, Lease, REQUESTED_LIFETIME};

/// How long removing a mapping may take as the endpoint closes.
const REMOVE_TIMEOUT: Duration = Duration::from_secs(2);

/// Whether and how to map the port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortMappingConfig {
    /// Whether to ask the router for a mapping at all.
    pub enabled: bool,
    /// Where to send PCP and NAT-PMP requests instead of the default gateway's port 5351.
    pub pcp_server: Option<SocketAddr>,
    /// Whether to try UPnP-IGD when PCP and NAT-PMP fail.
    pub upnp: bool,
}

/// Every protocol, at the default gateway.
impl Default for PortMappingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            pcp_server: None,
            upnp: true,
        }
    }
}

impl PortMappingConfig {
    /// No port mapping.
    pub fn off() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }
}

/// What port mapping is doing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum PortMappingStatus {
    /// Turned off, or there is nothing to map: no IPv4 socket, or one bound to loopback or to
    /// a public address.
    #[default]
    Off,
    /// Asking the router.
    Mapping,
    /// The router forwards a port to this peer.
    Mapped {
        /// The protocol the router granted it with.
        protocol: MappingProtocol,
        /// The address peers outside reach this one at.
        external: Address,
    },
    /// No router granted a mapping. Another attempt follows.
    Unavailable {
        /// Why, for each protocol tried.
        failures: Vec<MappingFailure>,
    },
}

impl PortMappingStatus {
    /// The external address, while mapped.
    pub fn address(&self) -> Option<Address> {
        match self {
            Self::Mapped { external, .. } => Some(*external),
            _ => None,
        }
    }
}

/// A protocol for asking a router to map a port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingProtocol {
    /// The Port Control Protocol (RFC 6887).
    Pcp,
    /// NAT Port Mapping Protocol (RFC 6886), PCP's predecessor.
    NatPmp,
    /// UPnP Internet Gateway Device.
    Upnp,
}

impl fmt::Display for MappingProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Pcp => "PCP",
            Self::NatPmp => "NAT-PMP",
            Self::Upnp => "UPnP-IGD",
        })
    }
}

/// Why mapping with one protocol failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{protocol}: {error}")]
pub struct MappingFailure {
    /// The protocol.
    pub protocol: MappingProtocol,
    /// What went wrong.
    pub error: MappingError,
}

/// What went wrong with one protocol.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MappingError {
    /// The default gateway, which PCP and NAT-PMP requests go to, could not be found.
    #[error("cannot find the default gateway: {0}")]
    NoGateway(String),
    /// No router answered.
    #[error("no router answered")]
    NoAnswer,
    /// The router turned the request down.
    #[error("the router refused: {0}")]
    Refused(String),
    /// The router's answer made no sense.
    #[error("the router's answer is unusable: {0}")]
    Invalid(&'static str),
    /// The router's external address is not public, so another router is in the way, as with
    /// carrier-grade NAT.
    #[error("the router's external address {0} is not public")]
    NotPublic(IpAddr),
    /// A socket failed.
    #[error("{0}")]
    Io(String),
}

/// A mapping a router granted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Granted {
    /// The address peers outside reach this one at.
    external: SocketAddrV4,
    /// How long it lasts, or `None` if until removed.
    lifetime: Option<Duration>,
}

/// A handle to an endpoint's port mapping. Clones share it.
#[derive(Debug, Clone, Default)]
pub struct PortMapping(Arc<Shared>);

#[derive(Debug)]
struct Shared {
    status: watch::Sender<PortMappingStatus>,
    config: watch::Sender<PortMappingConfig>,
    stopped: watch::Sender<bool>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            status: watch::channel(PortMappingStatus::Off).0,
            config: watch::channel(PortMappingConfig::off()).0,
            stopped: watch::channel(false).0,
            task: Mutex::new(None),
        }
    }
}

impl PortMapping {
    /// What port mapping is doing now.
    pub fn status(&self) -> PortMappingStatus {
        self.0.status.borrow().clone()
    }

    /// Follows what port mapping is doing.
    pub fn subscribe(&self) -> watch::Receiver<PortMappingStatus> {
        self.0.status.subscribe()
    }

    /// The external address, while mapped.
    pub fn address(&self) -> Option<Address> {
        self.0.status.borrow().address()
    }

    /// The addresses to advertise: the external address, while mapped, then `direct`.
    pub fn advertise(&self, direct: &[Address]) -> Vec<Address> {
        let external = self.address();
        external
            .into_iter()
            .chain(direct.iter().copied().filter(|&a| Some(a) != external))
            .collect()
    }

    /// Sets how to map `local`, the endpoint's IPv4 socket, and starts mapping if it isn't
    /// running. Must be called within a tokio runtime.
    pub(crate) fn configure(&self, local: Option<SocketAddrV4>, config: PortMappingConfig) {
        let enabled = config.enabled;
        self.0.config.send_if_modified(|current| {
            let changed = *current != config;
            *current = config;
            changed
        });
        let Some(local) = local.filter(|&local| mappable(local)) else {
            return;
        };
        let mut task = self.0.task.lock().expect("the lock is never poisoned");
        if enabled && task.is_none() && !*self.0.stopped.borrow() {
            // Mapping from now on, so nobody who waits for the outcome sees `Off` first.
            self.0.status.send_replace(PortMappingStatus::Mapping);
            let run = run(
                local,
                Arc::downgrade(&self.0),
                self.0.config.subscribe(),
                self.0.stopped.subscribe(),
            );
            *task = Some(tokio::spawn(run));
        }
    }

    /// Removes the mapping and stops for good, as the endpoint closes.
    pub(crate) fn stop(&self) {
        self.0.stopped.send_replace(true);
    }

    /// Waits until the mapping is removed, after [`stop`](Self::stop).
    pub(crate) async fn finished(&self) {
        let task = self
            .0
            .task
            .lock()
            .expect("the lock is never poisoned")
            .take();
        if let Some(task) = task {
            // A panic in the task would be a bug; nothing to clean up either way.
            let _ = task.await;
        }
    }
}

/// Whether a socket bound to `local` may be behind a router that maps ports: one bound to every
/// interface, or to a private address.
fn mappable(local: SocketAddrV4) -> bool {
    !local.ip().is_loopback() && !is_public(IpAddr::V4(*local.ip()))
}

/// A mapping, and the router that granted it.
#[derive(Debug)]
struct Mapping {
    router: Router,
    external: SocketAddrV4,
    lease: Lease,
}

/// A router, through the protocol it granted a mapping with.
#[derive(Debug)]
enum Router {
    Pcp(pcp::Client),
    Upnp(upnp::Client),
}

impl Router {
    fn protocol(&self) -> MappingProtocol {
        match self {
            Self::Pcp(client) => match client.version() {
                pcp::Version::Pcp => MappingProtocol::Pcp,
                pcp::Version::NatPmp => MappingProtocol::NatPmp,
            },
            Self::Upnp(_) => MappingProtocol::Upnp,
        }
    }

    async fn map(
        &mut self,
        port: u16,
        suggested: Option<SocketAddrV4>,
    ) -> Result<Granted, MappingError> {
        match self {
            Self::Pcp(client) => client.map(port, suggested, REQUESTED_LIFETIME).await,
            Self::Upnp(client) => client.map(port, suggested, REQUESTED_LIFETIME).await,
        }
    }

    /// Removes the mapping of `port` at `external`, giving up after a moment.
    async fn remove(&mut self, port: u16, external: SocketAddrV4) {
        let remove = async {
            match self {
                Self::Pcp(client) => client
                    .map(port, Some(external), Duration::ZERO)
                    .await
                    .map(drop),
                Self::Upnp(client) => client.remove(external.port()).await,
            }
        };
        // The lease runs out by itself if the router doesn't hear this.
        let _ = timeout(REMOVE_TIMEOUT, remove).await;
    }
}

impl Mapping {
    /// Asks `router` for a mapping of `port`.
    async fn new(mut router: Router, port: u16) -> Result<Self, MappingFailure> {
        let granted = router.map(port, None).await;
        // Known only now: a PCP client may have found the router speaks NAT-PMP.
        let protocol = router.protocol();
        let fail = |error| MappingFailure { protocol, error };
        let granted = granted.map_err(fail)?;
        if !is_public(IpAddr::V4(*granted.external.ip())) {
            router.remove(port, granted.external).await;
            return Err(fail(MappingError::NotPublic(IpAddr::V4(
                *granted.external.ip(),
            ))));
        }
        Ok(Self {
            router,
            external: granted.external,
            lease: Lease {
                granted: Instant::now(),
                lifetime: granted.lifetime,
            },
        })
    }

    /// Asks the router to extend the mapping of `port`.
    async fn renew(&mut self, port: u16) -> Result<(), MappingFailure> {
        let protocol = self.router.protocol();
        let fail = |error| MappingFailure { protocol, error };
        let granted = self
            .router
            .map(port, Some(self.external))
            .await
            .map_err(fail)?;
        self.external = granted.external;
        self.lease = Lease {
            granted: Instant::now(),
            lifetime: granted.lifetime,
        };
        match is_public(IpAddr::V4(*granted.external.ip())) {
            true => Ok(()),
            false => Err(fail(MappingError::NotPublic(IpAddr::V4(
                *granted.external.ip(),
            )))),
        }
    }

    fn status(&self) -> PortMappingStatus {
        PortMappingStatus::Mapped {
            protocol: self.router.protocol(),
            external: Address::from(SocketAddr::V4(self.external)),
        }
    }
}

/// Asks each protocol in turn for a mapping of `local`'s port, until one is granted.
async fn map(
    local: SocketAddrV4,
    config: &PortMappingConfig,
) -> Result<Mapping, Vec<MappingFailure>> {
    let mut failures = Vec::new();
    match pcp_client(local, config).await {
        Ok(client) => match Mapping::new(Router::Pcp(client), local.port()).await {
            Ok(mapping) => return Ok(mapping),
            Err(failure) => failures.push(failure),
        },
        Err(error) => failures.push(MappingFailure {
            protocol: MappingProtocol::Pcp,
            error,
        }),
    }
    if config.upnp {
        let failure = match upnp::Client::find(*local.ip()).await {
            Ok(client) => match Mapping::new(Router::Upnp(client), local.port()).await {
                Ok(mapping) => return Ok(mapping),
                Err(failure) => failure,
            },
            Err(error) => MappingFailure {
                protocol: MappingProtocol::Upnp,
                error,
            },
        };
        failures.push(failure);
    }
    Err(failures)
}

/// A PCP client for the configured server or the default gateway.
async fn pcp_client(
    local: SocketAddrV4,
    config: &PortMappingConfig,
) -> Result<pcp::Client, MappingError> {
    let server = match config.pcp_server {
        Some(server) => server,
        None => SocketAddr::from((default_gateway().await?, wire::SERVER_PORT)),
    };
    pcp::Client::new(server, *local.ip())
        .await
        .map_err(|error| MappingError::Io(error.to_string()))
}

/// The default gateway's IPv4 address.
async fn default_gateway() -> Result<Ipv4Addr, MappingError> {
    let lookup = tokio::task::spawn_blocking(netdev::get_default_gateway);
    let gateway = lookup
        .await
        .map_err(|error| MappingError::NoGateway(error.to_string()))?
        .map_err(MappingError::NoGateway)?;
    gateway
        .ipv4
        .first()
        .copied()
        .ok_or_else(|| MappingError::NoGateway("it has no IPv4 address".to_owned()))
}

/// The address this machine sends to `ip` from.
fn local_ip_towards(ip: IpAddr) -> Result<Ipv4Addr, MappingError> {
    let io = |error: std::io::Error| MappingError::Io(error.to_string());
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).map_err(io)?;
    // Connecting a UDP socket sends nothing; it only picks the route.
    socket.connect((ip, 9)).map_err(io)?;
    match socket.local_addr().map_err(io)?.ip() {
        IpAddr::V4(local) => Ok(local),
        IpAddr::V6(_) => Err(MappingError::Invalid("an IPv6 gateway")),
    }
}

/// The port mapping task: maps, renews and maps again whenever a mapping is lost, until stopped.
async fn run(
    local: SocketAddrV4,
    shared: Weak<Shared>,
    mut config: watch::Receiver<PortMappingConfig>,
    mut stopped: watch::Receiver<bool>,
) {
    let publish = |status: PortMappingStatus| {
        if let Some(shared) = shared.upgrade() {
            shared.status.send_replace(status);
        }
    };
    let port = local.port();
    tracing::info!("port mapping started for {local}");
    let mut unavailable = false;
    let mut mapping: Option<Mapping> = None;
    let mut backoff = Backoff::default();
    let mut due = Instant::now();
    while !*stopped.borrow_and_update() {
        let current = config.borrow_and_update().clone();
        if !current.enabled {
            if let Some(mut old) = mapping.take() {
                tracing::info!("port mapping turned off; removing {}", old.external);
                old.router.remove(port, old.external).await;
            }
            publish(PortMappingStatus::Off);
        }
        tokio::select! {
            () = sleep_until(due), if current.enabled => {}
            changed = config.changed() => {
                if changed.is_err() {
                    break;
                }
                // New settings: start over with them.
                tracing::info!("port mapping settings changed; mapping again");
                if let Some(mut old) = mapping.take() {
                    old.router.remove(port, old.external).await;
                }
                backoff.reset();
                due = Instant::now();
                continue;
            }
            // Stopped, or every handle is gone.
            _ = stopped.changed() => break,
        }

        if mapping.is_none() {
            publish(PortMappingStatus::Mapping);
        }
        let renewing = mapping.is_some();
        let attempt = async {
            match mapping.as_mut() {
                Some(mapping) => mapping.renew(port).await.map_err(|failure| vec![failure]),
                None => map(local, &current).await.map(|new| mapping = Some(new)),
            }
        };
        let result = tokio::select! {
            result = attempt => result,
            _ = stopped.changed() => break,
        };
        let now = Instant::now();
        match (result, &mapping) {
            (Ok(()), Some(mapping)) => {
                let status = mapping.status();
                if renewing || unavailable {
                    tracing::debug!("port mapping renewed: {status:?}");
                } else {
                    tracing::info!(
                        "port mapped: {status:?}, lease {:?}",
                        mapping.lease.lifetime
                    );
                }
                unavailable = false;
                backoff.reset();
                publish(mapping.status());
                due = mapping.lease.renew_at();
            }
            (Err(failures), Some(held)) => {
                let retry = match failures[0].error {
                    MappingError::NotPublic(_) => None,
                    _ => held.lease.retry_at(now),
                };
                match retry {
                    // Still mapped until the lease runs out.
                    Some(at) => {
                        tracing::warn!("port mapping renewal failed, will retry: {}", failures[0]);
                        due = at;
                    }
                    None => {
                        tracing::warn!("port mapping lost: {}", failures[0]);
                        unavailable = true;
                        if let Some(mut lost) = mapping.take() {
                            lost.router.remove(port, lost.external).await;
                        }
                        publish(PortMappingStatus::Unavailable { failures });
                        due = backoff.failed(now);
                    }
                }
            }
            (Err(failures), None) => {
                let reasons: Vec<String> = failures.iter().map(ToString::to_string).collect();
                // Once per outage; attempts go on with backoff.
                match unavailable {
                    false => tracing::warn!("no port mapping: {}", reasons.join("; ")),
                    true => tracing::debug!("still no port mapping: {}", reasons.join("; ")),
                }
                unavailable = true;
                publish(PortMappingStatus::Unavailable { failures });
                due = backoff.failed(now);
            }
            (Ok(()), None) => unreachable!("a successful attempt leaves a mapping"),
        }
    }
    if let Some(mut mapping) = mapping {
        tracing::info!("port mapping stopped; removing {}", mapping.external);
        mapping.router.remove(port, mapping.external).await;
    }
    publish(PortMappingStatus::Off);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_sockets_behind_a_router_are_mapped() {
        let socket = |ip: [u8; 4]| SocketAddrV4::new(ip.into(), 4773);
        assert!(mappable(socket([0, 0, 0, 0])));
        assert!(mappable(socket([192, 168, 1, 20])));
        assert!(mappable(socket([100, 64, 0, 1])));
        assert!(!mappable(socket([127, 0, 0, 1])));
        assert!(!mappable(socket([8, 8, 8, 8])));
    }

    #[test]
    fn the_external_address_is_advertised_first() {
        let mapping = PortMapping::default();
        let direct: Vec<Address> = ["192.168.1.20:4773", "1.2.3.4:4773"]
            .iter()
            .map(|a| a.parse::<SocketAddr>().unwrap().into())
            .collect();
        assert_eq!(mapping.advertise(&direct), direct);
        let external = Address::from("1.2.3.4:4773".parse::<SocketAddr>().unwrap());
        mapping.0.status.send_replace(PortMappingStatus::Mapped {
            protocol: MappingProtocol::Pcp,
            external,
        });
        assert_eq!(mapping.advertise(&direct), [external, direct[0]]);
    }

    #[test]
    fn failures_name_the_protocol() {
        let failure = MappingFailure {
            protocol: MappingProtocol::NatPmp,
            error: MappingError::Refused("not authorized".to_owned()),
        };
        assert_eq!(
            failure.to_string(),
            "NAT-PMP: the router refused: not authorized"
        );
        let failure = MappingFailure {
            protocol: MappingProtocol::Upnp,
            error: MappingError::NotPublic("100.64.0.1".parse().unwrap()),
        };
        assert_eq!(
            failure.to_string(),
            "UPnP-IGD: the router's external address 100.64.0.1 is not public"
        );
    }
}
