//! `tempest-relay`: run a relay, which lends public addresses to peers behind routers.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use tempest_net::Identity;
use tempest_protocol::PeerId;
use tempest_protocol::relay::RelayHost;
use tempest_relay::{DEFAULT_PORT, DEFAULT_RATE_LIMIT, Relay, RelayConfig};

/// The environment variable that overrides where the relay's identity is stored.
const RELAY_IDENTITY_ENV: &str = "TEMPEST_RELAY_IDENTITY";

/// Runs a relay, which lends public addresses to peers behind routers.
#[derive(Debug, clap::Parser)]
#[command(name = "tempest-relay", version)]
pub struct RelayArgs {
    /// The relay's key file. Created if missing. Defaults to `relay.pem` in Tempest's folder in
    /// the per-user data directory.
    #[arg(long, env = RELAY_IDENTITY_ENV, value_name = "FILE")]
    identity: Option<PathBuf>,
    /// The UDP port relay connections arrive at.
    #[arg(long, default_value_t = DEFAULT_PORT)]
    port: u16,
    /// The address to bind. Defaults to every interface, of both families where possible.
    #[arg(long, value_name = "IP")]
    bind: Option<IpAddr>,
    /// The public IP address in the relay addresses lent to peers. Needed behind a NAT; by
    /// default each peer is lent an address at the IP it reached the relay at.
    #[arg(long, value_name = "IP")]
    public_ip: Option<IpAddr>,
    /// The UDP ports to lend, as `FIRST-LAST`. Defaults to any the system picks.
    #[arg(long, value_name = "FIRST-LAST", value_parser = parse_ports)]
    ports: Option<(u16, u16)>,
    /// Serve only this peer, making the relay private. Repeat for more.
    #[arg(long, value_name = "PEER")]
    allow: Vec<PeerId>,
    /// The most peers registered at once.
    #[arg(long, default_value_t = RelayConfig::localhost().max_registrations)]
    max_peers: usize,
    /// The most peers registered at once from one IP address, or one IPv6 /64.
    #[arg(long, default_value_t = RelayConfig::localhost().max_per_ip)]
    max_per_ip: usize,
    /// The most bytes per second forwarded for each peer in each direction, or 0 for no limit.
    #[arg(long, default_value_t = DEFAULT_RATE_LIMIT, value_name = "BYTES")]
    rate_limit: u64,
}

pub fn relay(args: RelayArgs) -> Result<()> {
    let identity = load_identity(args.identity.as_ref())?;
    let runtime = tokio::runtime::Runtime::new().context("cannot start the async runtime")?;
    runtime.block_on(async {
        let config = RelayConfig {
            public_ip: args.public_ip,
            ports: args.ports.map(|(first, last)| first..=last),
            allowlist: (!args.allow.is_empty()).then(|| args.allow.iter().copied().collect()),
            max_registrations: args.max_peers,
            max_per_ip: args.max_per_ip,
            rate_limit: (args.rate_limit > 0).then_some(args.rate_limit),
            ..RelayConfig::new(SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), args.port))
        };
        let relay = start(&identity, config, args.bind)?;
        relay.serve(tempest_net::signal::interrupted()).await;
        eprintln!("stopped");
        Ok(())
    })
}

/// Loads the relay's identity from `path`, or from its default place.
fn load_identity(path: Option<&PathBuf>) -> Result<Identity> {
    let path = match path {
        Some(path) => path.clone(),
        None => {
            let data_dir = tempest_net::dirs::default_data_dir().with_context(|| {
                format!(
                    "cannot find the per-user data directory; pass --identity or set \
                     {RELAY_IDENTITY_ENV}"
                )
            })?;
            data_dir.join("relay.pem")
        }
    };
    Identity::load_or_create(&path)
        .with_context(|| format!("cannot load the relay identity {}", path.display()))
}

/// Binds a relay on `ip`, or else IPv6's wildcard address, which takes IPv4 too on most
/// systems, falling back to IPv4's. Tells the operator how peers should name it. Must be called
/// within a tokio runtime.
fn start(identity: &Identity, config: RelayConfig, ip: Option<IpAddr>) -> Result<Relay> {
    let port = config.bind.port();
    let candidates = match ip {
        Some(ip) => vec![ip],
        None => vec![Ipv6Addr::UNSPECIFIED.into(), Ipv4Addr::UNSPECIFIED.into()],
    };
    let mut last = None;
    for ip in candidates {
        let config = RelayConfig {
            bind: SocketAddr::new(ip, port),
            ..config.clone()
        };
        let public_ip = config.public_ip;
        match Relay::bind(identity, config) {
            Ok(relay) => {
                let address = relay.local_addr()?;
                eprintln!("relay listening on UDP {address}; press Ctrl-C to stop");
                let host = public_ip.map_or("<host>".into(), |ip| RelayHost::Ip(ip).to_string());
                eprintln!("peers name it {}@{host}:{}", relay.key(), address.port());
                return Ok(relay);
            }
            Err(error) => last = Some(error),
        }
    }
    let error = last.expect("at least one candidate");
    Err(error).with_context(|| format!("cannot bind UDP port {port}"))
}

fn parse_ports(text: &str) -> Result<(u16, u16)> {
    let (first, last) = text.split_once('-').context("expected FIRST-LAST")?;
    let (first, last): (u16, u16) = (first.parse()?, last.parse()?);
    if first == 0 || first > last {
        bail!("expected 1 ≤ FIRST ≤ LAST");
    }
    Ok((first, last))
}
