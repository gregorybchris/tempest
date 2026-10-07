//! What `share` and `get` have in common: the identity, the endpoint and the node.

use std::net::{IpAddr, SocketAddrV4, SocketAddrV6};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tempest_net::{Endpoint, EndpointConfig, Identity, Node, NodeConfig};
use tempest_protocol::Address;

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
}

/// A running node and what it needs to tell others about itself.
pub struct Peer {
    pub node: Node,
    /// The addresses it advertises.
    pub addresses: Vec<Address>,
}

impl Peer {
    /// Shuts the node down, waiting a moment for its Goodbyes to arrive. Connections that never
    /// finished their handshake, such as dials to an unreachable address, take seconds to
    /// drain, and nobody is waiting for their Goodbye.
    pub async fn stop(&self) {
        let _ = tokio::time::timeout(Duration::from_secs(1), self.node.shutdown()).await;
    }
}

impl PeerArgs {
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
        let endpoint = Endpoint::bind(identity, &endpoint_config)
            .with_context(|| format!("cannot bind UDP port {port}"))?;
        let addresses = if advertise {
            tempest_net::direct_addresses(&endpoint.local_addrs())
                .context("cannot list the network interfaces' addresses")?
        } else {
            Vec::new()
        };
        config.addresses = addresses.clone();
        Ok(Peer {
            node: Node::start(endpoint, config),
            addresses,
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

/// `identity.pem` in Tempest's folder in the per-user data directory: the local application
/// data folder on Windows, Application Support on macOS, and the XDG data directory elsewhere.
fn default_identity_path() -> Result<PathBuf> {
    let missing = || {
        format!("cannot find the per-user data directory; pass --identity or set {IDENTITY_ENV}")
    };
    let absolute = |path: PathBuf| Some(path).filter(|path| path.is_absolute());
    let env = |name: &str| std::env::var_os(name).map(PathBuf::from).and_then(absolute);
    let home = || std::env::home_dir().and_then(absolute);

    let data_dir = if cfg!(windows) {
        env("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        home().map(|home| home.join("Library/Application Support"))
    } else {
        env("XDG_DATA_HOME").or_else(|| home().map(|home| home.join(".local/share")))
    };
    let tempest = if cfg!(any(windows, target_os = "macos")) {
        "Tempest"
    } else {
        "tempest"
    };
    Ok(data_dir
        .with_context(missing)?
        .join(tempest)
        .join("identity.pem"))
}
