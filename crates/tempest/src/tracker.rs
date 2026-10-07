//! `tempest tracker`: run a tracker's peer-list service.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use tempest_catalog_api::Category;
use tempest_protocol::{BundleId, RelayIdentity};
use tempest_relay::RelayConfig;
use tempest_tracker::{Catalog, CatalogConfig, DEFAULT_PORT, Tracker, TrackerConfig};
use tokio::net::TcpListener;
use tokio::sync::watch;

use crate::relay;

#[derive(Debug, clap::Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct TrackerArgs {
    #[command(subcommand)]
    command: Option<TrackerCommand>,
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
    /// Also run a relay, with the default limits of `tempest relay`.
    #[arg(long)]
    run_relay: bool,
    /// The relay's UDP port.
    #[arg(long, default_value_t = tempest_relay::DEFAULT_PORT, requires = "run_relay")]
    relay_port: u16,
    /// The relay's public IP address. With it, the tracker recommends its own relay to peers.
    #[arg(long, value_name = "IP", requires = "run_relay")]
    relay_public_ip: Option<IpAddr>,
    /// The relay's key file. Created if missing. Defaults to that of `tempest relay`.
    #[arg(long, value_name = "FILE", requires = "run_relay")]
    relay_identity: Option<PathBuf>,
    /// Requests arrive through a reverse proxy, such as one that adds HTTPS: take the client's
    /// IP from the last `X-Forwarded-For` entry.
    #[arg(long)]
    behind_proxy: bool,
    #[command(flatten)]
    catalog: CatalogArgs,
}

#[derive(Debug, clap::Args)]
struct CatalogArgs {
    /// Run a catalog, a website of the bundles their sharers chose to list, saved in this
    /// SQLite file. Created if missing. You are responsible for what it lists.
    #[arg(long, value_name = "FILE")]
    catalog: Option<PathBuf>,
    /// Let a key join the catalog only with an invite (see `tempest tracker invite`).
    #[arg(long, requires = "catalog")]
    invites_required: bool,
    /// A category listings can be in, as `slug=Name`. Repeat for more; replaces the defaults.
    #[arg(long, value_name = "SLUG=NAME", value_parser = parse_category, requires = "catalog")]
    category: Vec<Category>,
}

#[derive(Debug, clap::Subcommand)]
enum TrackerCommand {
    /// Make an invite to a catalog, and print its code.
    Invite {
        /// The catalog's SQLite file, as given to `tempest tracker --catalog`.
        #[arg(long, value_name = "FILE")]
        catalog: PathBuf,
    },
}

fn parse_category(text: &str) -> Result<Category, String> {
    let (slug, name) = text
        .split_once('=')
        .ok_or("a category is `slug=Name`, such as `datasets=Datasets`")?;
    let valid = !slug.is_empty()
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid {
        return Err(format!(
            "the slug {slug:?} must be lowercase letters, digits and -"
        ));
    }
    if name.trim().is_empty() {
        return Err("the category's name is empty".into());
    }
    Ok(Category::new(slug, name.trim()))
}

impl CatalogArgs {
    fn open(&self) -> Result<Option<Catalog>> {
        let Some(path) = &self.catalog else {
            return Ok(None);
        };
        let config = CatalogConfig {
            invites_required: self.invites_required,
            categories: match self.category.is_empty() {
                true => CatalogConfig::default().categories,
                false => self.category.clone(),
            },
            ..CatalogConfig::default()
        };
        let catalog = Catalog::open(path, config)
            .with_context(|| format!("cannot open the catalog at {}", path.display()))?;
        Ok(Some(catalog))
    }
}

pub fn tracker(args: TrackerArgs) -> Result<()> {
    if let Some(TrackerCommand::Invite { catalog }) = &args.command {
        return invite(catalog);
    }
    let catalog = args.catalog.open()?;
    let relay_identity = args
        .run_relay
        .then(|| relay::load_identity(args.relay_identity.as_ref()))
        .transpose()?;
    let mut config = TrackerConfig {
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
        // Relaying is one of the tracker's roles (D-028).
        let relay = match &relay_identity {
            Some(identity) => {
                let relay_config = RelayConfig {
                    public_ip: args.relay_public_ip,
                    ..RelayConfig::new(SocketAddr::new(
                        Ipv4Addr::UNSPECIFIED.into(),
                        args.relay_port,
                    ))
                };
                let relay = relay::start(identity, relay_config, args.bind)?;
                config
                    .relays
                    .extend(relay::identity_of(&relay, args.relay_public_ip)?);
                Some(relay)
            }
            None => None,
        };

        let listener = bind(args.bind, args.port).await?;
        let address = listener.local_addr()?;
        eprintln!("tracker listening on http://{address}; press Ctrl-C to stop");
        let mut tracker = Tracker::new(config)?.behind_proxy(args.behind_proxy);
        if let Some(catalog) = catalog {
            eprintln!("catalog at http://{address}/catalog/v1/info");
            tracker = tracker.with_catalog(catalog);
        }
        let (stop, stopped) = watch::channel(false);
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            let _ = stop.send(true);
        });
        let shutdown = |mut stopped: watch::Receiver<bool>| async move {
            let _ = stopped.wait_for(|stop| *stop).await;
        };
        let relaying = async {
            if let Some(relay) = &relay {
                relay.serve(shutdown(stopped.clone())).await;
            }
        };
        let (served, ()) =
            tokio::join!(tracker.serve(listener, shutdown(stopped.clone())), relaying);
        served?;
        eprintln!("stopped");
        Ok(())
    })
}

fn invite(path: &Path) -> Result<()> {
    let mut catalog = Catalog::open(path, CatalogConfig::default())
        .with_context(|| format!("cannot open the catalog at {}", path.display()))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let code = catalog
        .create_invite(now)
        .map_err(|refusal| anyhow::anyhow!(refusal.body.message))?;
    println!("{code}");
    Ok(())
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
