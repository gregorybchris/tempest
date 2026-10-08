//! What `share` and `get` have in common: the identity, the endpoint and the node.

use std::net::{IpAddr, SocketAddrV4, SocketAddrV6};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tempest_net::{
    AnnounceConfig, Announcer, BundleHandle, Endpoint, EndpointConfig, Identity, Lan, LanConfig,
    Node, NodeConfig, PortMappingConfig, PortMappingStatus, RelayChoice, RelayStatus,
    TrackerClient,
};
use tempest_protocol::lan::LAN_PORT;
use tempest_protocol::{Address, RelayIdentity};

/// How long a tracker request may take.
pub const TRACKER_TIMEOUT: Duration = Duration::from_secs(30);

/// How long to wait for the home relay and port mapping before going on without them.
const RELAY_TIMEOUT: Duration = Duration::from_secs(10);

/// The environment variable that overrides where the identity is stored.
pub const IDENTITY_ENV: &str = "TEMPEST_IDENTITY";

/// Options for running a peer.
#[derive(Debug, clap::Args)]
pub struct PeerArgs {
    /// The identity key file. Created if missing. Defaults to `identity.pem` in Tempest's folder
    /// in the per-user data directory.
    #[arg(long, env = IDENTITY_ENV, value_name = "FILE")]
    pub identity: Option<PathBuf>,
    /// An address to bind instead of every interface, at most one per address family. Repeat for
    /// both families.
    #[arg(long, value_name = "IP")]
    pub bind: Vec<IpAddr>,
    /// A relay to pick a home relay from, as `<key>@host[:port]`, so peers that can't reach
    /// this one directly can reach it through the relay. Repeat for more; the one that answers
    /// fastest is used. Trackers may recommend more.
    #[arg(long, value_name = "RELAY")]
    pub relay: Vec<RelayIdentity>,
    /// Neither look for peers on the local network nor announce this one there.
    #[arg(long)]
    pub no_lan: bool,
    /// Don't ask the router to forward the peer port (with PCP, NAT-PMP or UPnP-IGD).
    #[arg(long)]
    pub no_port_mapping: bool,
    #[arg(long, value_name = "PORT", help = format!(
        "The UDP port of LAN discovery's multicast groups [default: {LAN_PORT}]"
    ))]
    pub lan_port: Option<u16>,
}

/// A running node and what it needs to tell others about itself.
pub struct Peer {
    pub node: Node,
    /// Its key, which signs sign-ins and listing requests.
    pub identity: Identity,
    /// The addresses it advertises.
    pub addresses: Vec<Address>,
    /// LAN discovery, unless it is off or could not start.
    pub lan: Option<Lan>,
    /// Whether relays were given to pick a home relay from.
    relays_given: bool,
}

impl Peer {
    /// Starts announcing `handle`'s bundle to each of `trackers`, asking those in `publish` to
    /// list it.
    pub fn announce(
        &self,
        trackers: &[String],
        publish: &[String],
        handle: &BundleHandle,
    ) -> Result<Vec<Announcer>> {
        if trackers.is_empty() {
            return Ok(Vec::new());
        }
        let endpoint = self.node.endpoint();
        let port = endpoint
            .local_addrs()
            .first()
            .context("the endpoint has no socket")?
            .port();
        let config = AnnounceConfig {
            addrs: tempest_net::public_addresses(&self.addresses),
            ..AnnounceConfig::new(endpoint.peer_id(), port)
        };
        let client = TrackerClient::new(TRACKER_TIMEOUT).context("cannot set up HTTPS")?;
        let listing = self.identity.listing(&handle.bundle_id());
        let announcers = trackers
            .iter()
            .map(|tracker| {
                let listed = publish.contains(tracker).then_some(listing);
                Announcer::spawn(
                    client.clone(),
                    tracker.clone(),
                    config.clone(),
                    handle,
                    listed,
                )
            })
            .collect();
        Ok(announcers)
    }

    /// Waits for the home relay to register, if relays were given, and returns the relay
    /// address. Warns if no relay would register this peer.
    pub async fn home_relay(&self) -> Option<Address> {
        if !self.relays_given {
            return None;
        }
        let mut status = self.node.endpoint().home_relay().subscribe();
        let settled =
            status.wait_for(|status| !matches!(status, RelayStatus::Off | RelayStatus::Connecting));
        let status = match tokio::time::timeout(RELAY_TIMEOUT, settled).await {
            Ok(Ok(status)) => status.clone(),
            // Still connecting: the link goes out without it.
            _ => return None,
        };
        if let RelayStatus::Unavailable { failures } = &status {
            for failure in failures {
                eprintln!("warning: relay {failure}");
            }
        }
        status.relay_address()
    }

    /// Waits for the router to map the peer port, if port mapping is on, and returns the
    /// external address. Warns if no router would map it.
    pub async fn port_mapping(&self) -> Option<Address> {
        let mut status = self.node.endpoint().port_mapping().subscribe();
        let settled = status.wait_for(|status| *status != PortMappingStatus::Mapping);
        let status = match tokio::time::timeout(RELAY_TIMEOUT, settled).await {
            Ok(Ok(status)) => status.clone(),
            // Still asking: the link goes out without it.
            _ => return None,
        };
        if let PortMappingStatus::Unavailable { failures } = &status {
            for failure in failures {
                eprintln!("warning: no port mapping: {failure}");
            }
        }
        status.address()
    }

    /// Shuts the node down, waiting a moment for its Goodbyes to arrive. Connections that never
    /// finished their handshake, such as dials to an unreachable address, take seconds to
    /// drain, and nobody is waiting for their Goodbye.
    pub async fn stop(&self) {
        let _ = tokio::time::timeout(Duration::from_secs(1), self.node.shutdown()).await;
    }

    /// Waits a moment for `announcers` to tell their trackers the bundle stopped.
    pub async fn stop_announcing(announcers: Vec<Announcer>) {
        let stopping: tokio::task::JoinSet<()> =
            announcers.into_iter().map(Announcer::stop).collect();
        let _ = tokio::time::timeout(Duration::from_secs(2), stopping.join_all()).await;
    }
}

impl PeerArgs {
    /// Whether any option is given, each of which asks to run standalone rather than through
    /// the daemon.
    pub fn any_given(&self) -> bool {
        self.identity.is_some()
            || !self.bind.is_empty()
            || !self.relay.is_empty()
            || self.no_lan
            || self.no_port_mapping
            || self.lan_port.is_some()
    }

    /// Loads the identity and starts a node on `port` (0 for any), advertising its direct
    /// addresses if `advertise` is set. Must be called within a tokio runtime.
    pub fn start(&self, port: u16, advertise: bool) -> Result<Peer> {
        let identity_path = match &self.identity {
            Some(path) => path.clone(),
            None => default_identity_path()?,
        };
        let identity = Identity::load_or_create(&identity_path)
            .with_context(|| format!("cannot load the identity {}", identity_path.display()))?;

        let mut config = NodeConfig::default();
        let endpoint_config = EndpointConfig {
            max_open_requests: config.session.upload.max_open_requests,
            ..self.endpoint_config(port)?
        };
        let endpoint = Endpoint::bind(identity.clone(), &endpoint_config)
            .with_context(|| format!("cannot bind UDP port {port}"))?;
        let addresses = if advertise {
            tempest_net::direct_addresses(&endpoint.local_addrs())
                .context("cannot list the network interfaces' addresses")?
        } else {
            Vec::new()
        };
        config.addresses = addresses.clone();
        endpoint.set_relays(RelayChoice {
            known: self.relay.clone(),
            from_trackers: true,
        });
        endpoint.set_port_mapping(match self.no_port_mapping {
            true => PortMappingConfig::off(),
            false => PortMappingConfig::default(),
        });
        let lan = if self.no_lan {
            None
        } else {
            let config = LanConfig {
                port: self.lan_port.unwrap_or(LAN_PORT),
                ..LanConfig::default()
            };
            Lan::start(&endpoint, config)
                .inspect_err(|error| eprintln!("warning: LAN discovery is off: {error}"))
                .ok()
        };
        Ok(Peer {
            node: Node::start(endpoint, config),
            identity,
            addresses,
            lan,
            relays_given: !self.relay.is_empty(),
        })
    }

    /// Both families' wildcard addresses, or the addresses given with `--bind`.
    fn endpoint_config(&self, port: u16) -> Result<EndpointConfig> {
        if self.bind.is_empty() {
            return Ok(EndpointConfig::any(port));
        }
        let mut config = EndpointConfig {
            ipv4: None,
            ipv6: None,
            ..EndpointConfig::default()
        };
        for &ip in &self.bind {
            let repeated = match ip {
                IpAddr::V4(v4) => config.ipv4.replace(SocketAddrV4::new(v4, port)).is_some(),
                IpAddr::V6(v6) => config
                    .ipv6
                    .replace(SocketAddrV6::new(v6, port, 0, 0))
                    .is_some(),
            };
            if repeated {
                bail!("--bind takes at most one address per address family");
            }
        }
        Ok(config)
    }
}

/// `identity.pem` in Tempest's folder in the per-user data directory, which the engine uses
/// too.
fn default_identity_path() -> Result<PathBuf> {
    let data_dir = tempest_engine::config::default_data_dir().with_context(|| {
        format!("cannot find the per-user data directory; pass --identity or set {IDENTITY_ENV}")
    })?;
    Ok(data_dir.join("identity.pem"))
}
