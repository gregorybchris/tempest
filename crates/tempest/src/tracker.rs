//! `tempest tracker`: run a tracker's peer-list service.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use anyhow::{Context, Result};
use tempest_protocol::{BundleId, RelayIdentity};
use tempest_tracker::{DEFAULT_PORT, Tracker, TrackerConfig};
use tokio::net::TcpListener;

#[derive(Debug, clap::Args)]
pub struct TrackerArgs {
    /// The TCP port to serve on.
    #[arg(long, default_value_t = DEFAULT_PORT)]
    port: u16,
    /// The address to bind. Defaults to every interface, of both families where possible.
    #[arg(long, value_name = "IP")]
    bind: Option<IpAddr>,
    /// Seconds peers should wait between announces.
    #[arg(long, default_value_t = 300, value_name = "SECONDS")]
    interval: u64,
    /// The fewest seconds between a peer's updates before it is told to slow down.
    #[arg(long, default_value_t = 60, value_name = "SECONDS")]
    min_interval: u64,
    /// The most peers listed across all swarms.
    #[arg(long, default_value_t = TrackerConfig::default().max_peers)]
    max_peers: usize,
    /// A relay to recommend to peers, as `<key>@host[:port]`. Repeat for more.
    #[arg(long, value_name = "RELAY")]
    relay: Vec<RelayIdentity>,
    /// Serve only this bundle, making the tracker private. Repeat for more.
    #[arg(long, value_name = "BUNDLE")]
    allow: Vec<BundleId>,
    /// Requests arrive through a reverse proxy, such as one that adds HTTPS: take the client's
    /// IP from the last `X-Forwarded-For` entry.
    #[arg(long)]
    behind_proxy: bool,
}

pub fn tracker(args: TrackerArgs) -> Result<()> {
    let config = TrackerConfig {
        interval: Duration::from_secs(args.interval.max(1)),
        min_interval: Duration::from_secs(args.min_interval),
        max_peers: args.max_peers,
        allowed: (!args.allow.is_empty())
            .then(|| args.allow.iter().copied().collect::<HashSet<_>>()),
        relays: args.relay.clone(),
        ..TrackerConfig::default()
    };
    let runtime = tokio::runtime::Runtime::new().context("cannot start the async runtime")?;
    runtime.block_on(async {
        let listener = bind(args.bind, args.port).await?;
        let address = listener.local_addr()?;
        eprintln!("tracker listening on http://{address}; press Ctrl-C to stop");
        let tracker = Tracker::new(config)?.behind_proxy(args.behind_proxy);
        let shutdown = async {
            let _ = tokio::signal::ctrl_c().await;
        };
        tracker.serve(listener, shutdown).await?;
        eprintln!("stopped");
        Ok(())
    })
}

/// Binds `ip`, or else IPv6's wildcard address, which takes IPv4 too on most systems, falling
/// back to IPv4's where IPv6 is unavailable.
async fn bind(ip: Option<IpAddr>, port: u16) -> Result<TcpListener> {
    let candidates = match ip {
        Some(ip) => vec![ip],
        None => vec![Ipv6Addr::UNSPECIFIED.into(), Ipv4Addr::UNSPECIFIED.into()],
    };
    let mut last = None;
    for ip in candidates {
        match TcpListener::bind(SocketAddr::new(ip, port)).await {
            Ok(listener) => return Ok(listener),
            Err(error) => last = Some(error),
        }
    }
    let error = last.expect("at least one candidate");
    Err(error).with_context(|| format!("cannot bind TCP port {port}"))
}
