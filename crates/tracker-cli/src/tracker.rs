//! `tempest-tracker`: run a tracker, and operate its catalog.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use tempest_assets as assets;
use tempest_net::FetchLimits;
use tempest_protocol::{BundleId, PeerId, RelayIdentity};
use tempest_tracker::limits::Action;
use tempest_tracker::{
    Catalog, CatalogConfig, CatalogLimits, DEFAULT_PORT, FetcherConfig, Rate, Tracker,
    TrackerConfig,
};
use tempest_tracker_api::Category;
use tokio::net::TcpListener;

/// The tracker website's files, by their path in the page, as `crates/tracker-web/dist` held them at
/// build time (D-201).
static TRACKER_WEB: assets::Assets = include!(concat!(env!("OUT_DIR"), "/tracker_web_assets.rs"));

/// What `/` shows when the binary was built without the tracker website.
const TRACKER_WEB_NOT_BUILT: &str = r#"<!DOCTYPE html>
<html lang="en">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Tempest tracker</title>
<style>
  body { background: #020b05; color: #8af2a4; font: 15px/1.6 ui-monospace, monospace; padding: 8vh 6vw; }
  code { color: #d2ffe0; }
</style>
<h1>Tempest tracker</h1>
<p>The tracker is running, but this <code>tempest-tracker</code> binary was built without its website.</p>
<p>Build the website with <code>trunk build</code> in <code>crates/tracker-web</code>,
then build <code>tempest-tracker</code> again. Meanwhile, its API answers under <code>/tempest/v1</code>, as in
<code>/tempest/v1/listings</code>.</p>
</html>
"#;

/// Tempest's tracker: helps peers in public swarms find each other, and with a catalog, takes
/// listings and serves a website of them.
#[derive(Debug, clap::Parser)]
#[command(
    name = "tempest-tracker",
    version,
    args_conflicts_with_subcommands = true
)]
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
    /// The most peers listed across all swarms from one IP address, or one IPv6 /64.
    #[arg(long, default_value_t = TrackerConfig::default().max_peers_per_ip, value_name = "N")]
    max_peers_per_ip: usize,
    /// The most peers listed in one swarm from one IP address, or one IPv6 /64.
    #[arg(long, default_value_t = TrackerConfig::default().max_swarm_peers_per_ip,
        value_name = "N")]
    max_swarm_peers_per_ip: usize,
    /// A relay to recommend to peers, as `<key>@host[:port]`, which `tempest-relay` prints
    /// when it starts. Repeat for more. The relay runs separately.
    #[arg(long, value_name = "RELAY")]
    relay: Vec<RelayIdentity>,
    /// Serve only this bundle, making the tracker private. Repeat for more.
    #[arg(long, value_name = "BUNDLE")]
    allow: Vec<BundleId>,
    /// Requests arrive through a reverse proxy, such as one that adds HTTPS: take the client's
    /// IP from the last `X-Forwarded-For` entry.
    #[arg(long)]
    behind_proxy: bool,
    /// The URL people reach the tracker at, such as `https://tracker.example.org`. Sign-ins
    /// are signed for it, so no other site can pass on this tracker's sign-in codes. Without
    /// it, only this machine can sign in to the catalog. Required with `--behind-proxy` and
    /// `--catalog`.
    #[arg(long, value_name = "URL")]
    public_url: Option<String>,
    #[command(flatten)]
    catalog: CatalogArgs,
}

#[derive(Debug, clap::Args)]
struct CatalogArgs {
    /// Take listings, and serve a website of them. The catalog that stores the listings,
    /// accounts, votes and comments is this SQLite file. Created if missing. You are
    /// responsible for what it lists.
    #[arg(long, value_name = "FILE")]
    catalog: Option<PathBuf>,
    /// Let a key join the catalog only with an invite (see `tempest-tracker invite`).
    #[arg(long, requires = "catalog")]
    invites_required: bool,
    /// A category listings can be in, as `slug=Name`. Repeat for more; replaces the defaults.
    #[arg(long, value_name = "SLUG=NAME", value_parser = parse_category, requires = "catalog")]
    category: Vec<Category>,
    /// The most manifests fetched at once, to list bundles.
    #[arg(long, default_value_t = FetcherConfig::default().concurrency, value_name = "N",
        requires = "catalog")]
    fetch_concurrency: usize,
    /// The largest manifest fetched, in bytes. Bundles with larger ones stay pending until
    /// they are dropped.
    #[arg(long, default_value_t = FetcherConfig::default().limits.max_manifest,
        value_name = "BYTES", requires = "catalog")]
    fetch_max_manifest: u32,
    /// Hours a listing may wait for its manifest, from when it was made or last edited, before
    /// it is dropped. Its sharer's next announce asks again.
    #[arg(long, default_value_t = FetcherConfig::default().pending_lifetime.as_secs() / 3600,
        value_name = "HOURS", requires = "catalog")]
    pending_hours: u64,
    /// Change a per-IP rate limit, as `action=BURST/SECONDS` (BURST at once, then one per
    /// SECONDS) or `action=off`. Actions: challenge, sign-in, browse, publish, edit, vote,
    /// comment, report, download. Repeat for more. See specs/tracker.md for the defaults.
    #[arg(long, value_name = "ACTION=RATE", value_parser = parse_limit, requires = "catalog")]
    catalog_limit: Vec<(Action, Option<Rate>)>,
    /// The most sign-in codes outstanding for one IP address.
    #[arg(long, default_value_t = CatalogConfig::default().max_challenges_per_ip,
        value_name = "N", requires = "catalog")]
    challenges_per_ip: usize,
}

#[derive(Debug, clap::Subcommand)]
enum TrackerCommand {
    /// Make an invite to a catalog, and print its code.
    Invite {
        /// The catalog's SQLite file, as given to `tempest-tracker --catalog`.
        #[arg(long, value_name = "FILE")]
        catalog: PathBuf,
    },
    /// Hide a listing from everyone but its publisher.
    Hide {
        /// The bundle ID of the listing.
        bundle: BundleId,
        #[command(flatten)]
        file: CatalogFile,
    },
    /// Show a hidden listing again.
    Unhide {
        /// The bundle ID of the listing.
        bundle: BundleId,
        #[command(flatten)]
        file: CatalogFile,
    },
    /// Ban a key: its listings are hidden, and it can't list, edit, vote, comment or report.
    Ban {
        /// The key, as 64 hexadecimal characters.
        key: PeerId,
        #[command(flatten)]
        file: CatalogFile,
    },
    /// Lift a key's ban, which shows its listings again.
    Unban {
        /// The key, as 64 hexadecimal characters.
        key: PeerId,
        #[command(flatten)]
        file: CatalogFile,
    },
    /// List the reports of listings, newest first.
    Reports {
        /// Only the reports of this bundle.
        #[arg(long, value_name = "BUNDLE")]
        bundle: Option<BundleId>,
        #[command(flatten)]
        file: CatalogFile,
    },
}

/// Which catalog an operator command acts on. The commands change the database itself, so they
/// work whether or not a tracker is running on it.
#[derive(Debug, clap::Args)]
struct CatalogFile {
    /// The catalog's SQLite file, as given to `tempest-tracker --catalog`.
    #[arg(long, value_name = "FILE")]
    catalog: PathBuf,
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

fn parse_limit(text: &str) -> Result<(Action, Option<Rate>), String> {
    let (action, rate) = text
        .split_once('=')
        .ok_or("a limit is `action=BURST/SECONDS` or `action=off`, such as `comment=10/30`")?;
    let action = action.parse()?;
    if rate == "off" {
        return Ok((action, None));
    }
    let numbers = rate
        .split_once('/')
        .and_then(|(burst, every)| Some((burst.parse().ok()?, every.parse().ok()?)));
    let Some((burst, every)) = numbers else {
        return Err(format!(
            "the rate {rate:?} must be BURST/SECONDS, such as 10/30, or off"
        ));
    };
    Ok((action, Some(Rate::new(burst, every))))
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
            max_challenges_per_ip: self.challenges_per_ip,
            ..CatalogConfig::default()
        };
        let catalog = Catalog::open(path, config)
            .with_context(|| format!("cannot open the catalog at {}", path.display()))?;
        Ok(Some(catalog))
    }

    fn limits(&self) -> CatalogLimits {
        let mut limits = CatalogLimits::default();
        for &(action, rate) in &self.catalog_limit {
            match rate {
                Some(rate) => limits.rates.insert(action, rate),
                None => limits.rates.remove(&action),
            };
        }
        limits
    }

    fn fetcher(&self) -> FetcherConfig {
        let defaults = FetcherConfig::default();
        FetcherConfig {
            concurrency: self.fetch_concurrency.max(1),
            limits: FetchLimits {
                max_manifest: self.fetch_max_manifest,
                ..defaults.limits
            },
            pending_lifetime: Duration::from_secs(self.pending_hours.saturating_mul(3600)),
            ..defaults
        }
    }
}

pub fn tracker(args: TrackerArgs) -> Result<()> {
    match &args.command {
        Some(TrackerCommand::Invite { catalog }) => return invite(catalog),
        Some(command) => return operate(command, &mut std::io::stdout().lock()),
        None => {}
    }
    // Otherwise peers that announce on schedule are told to slow down.
    if args.min_interval > args.interval {
        anyhow::bail!("--min-interval must not be longer than --interval");
    }
    // Behind a proxy, clients never reach the tracker at a loopback address.
    if args.behind_proxy && args.catalog.catalog.is_some() && args.public_url.is_none() {
        anyhow::bail!(
            "--behind-proxy with --catalog needs --public-url, the URL people reach the tracker at"
        );
    }
    let catalog = args.catalog.open()?;
    tracing::info!(
        "tracker config: interval {}s, min interval {}s, max peers {} ({} per IP, {} per swarm \
         and IP), {} relays, {}, behind proxy {}, public URL {}",
        args.interval,
        args.min_interval,
        args.max_peers,
        args.max_peers_per_ip,
        args.max_swarm_peers_per_ip,
        args.relay.len(),
        match args.allow.len() {
            0 => "public".to_string(),
            n => format!("private ({n} allowed bundles)"),
        },
        args.behind_proxy,
        args.public_url.as_deref().unwrap_or("none"),
    );
    if let Some(path) = &args.catalog.catalog {
        tracing::info!(
            "catalog config: {}, invites required {}, categories {}, fetch concurrency {}, \
             max manifest {} bytes, pending lifetime {}h, rate limit overrides {}, \
             {} sign-in codes per IP",
            path.display(),
            args.catalog.invites_required,
            args.catalog.category.len(),
            args.catalog.fetch_concurrency,
            args.catalog.fetch_max_manifest,
            args.catalog.pending_hours,
            args.catalog.catalog_limit.len(),
            args.catalog.challenges_per_ip,
        );
    }
    let config = TrackerConfig {
        interval: Duration::from_secs(args.interval.max(1)),
        min_interval: Duration::from_secs(args.min_interval),
        max_peers: args.max_peers,
        max_peers_per_ip: args.max_peers_per_ip,
        max_swarm_peers_per_ip: args.max_swarm_peers_per_ip,
        allowed: (!args.allow.is_empty())
            .then(|| args.allow.iter().copied().collect::<HashSet<_>>()),
        relays: args.relay.clone(),
        ..TrackerConfig::default()
    };
    let runtime = tokio::runtime::Runtime::new().context("cannot start the async runtime")?;
    runtime.block_on(async {
        let listener = bind(args.bind, args.port).await?;
        let address = listener.local_addr()?;
        tracing::info!("listening on {address}");
        eprintln!("tracker listening on http://{address}; press Ctrl-C to stop");
        let mut tracker = Tracker::new(config)?.behind_proxy(args.behind_proxy);
        if let Some(url) = &args.public_url {
            tracker = tracker.public_url(url)?;
        }
        if let Some(catalog) = catalog {
            match &args.public_url {
                Some(url) => eprintln!("website at {url}"),
                None => {
                    eprintln!("website at http://{address}/");
                    eprintln!("note: only this machine can sign in; pass --public-url to let others");
                }
            }
            if TRACKER_WEB.is_empty() {
                eprintln!("note: this binary was built without the tracker website; the page says how to add it");
            }
            tracker = tracker
                .with_catalog(catalog)
                .catalog_limits(args.catalog.limits())
                .fetcher(args.catalog.fetcher())
                .with_pages(assets::router(TRACKER_WEB, TRACKER_WEB_NOT_BUILT));
        }
        let served = tracker
            .serve(listener, tempest_net::signal::interrupted())
            .await;
        if let Err(error) = &served {
            tracing::error!("the tracker stopped: {error}");
        }
        served?;
        tracing::info!("stopped");
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
    tracing::info!("operator made an invite");
    println!("{code}");
    Ok(())
}

/// The time now, in seconds since the Unix epoch.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// An existing catalog, for the operator's commands: they never create one.
fn open_existing(path: &Path) -> Result<Catalog> {
    anyhow::ensure!(path.is_file(), "there is no catalog at {}", path.display());
    Catalog::open(path, CatalogConfig::default())
        .with_context(|| format!("cannot open the catalog at {}", path.display()))
}

/// Runs an operator command (see [`TrackerCommand`]) on its catalog, and says what it did.
fn operate(command: &TrackerCommand, out: &mut impl std::io::Write) -> Result<()> {
    let refused = |refusal: tempest_tracker::Refusal| anyhow::anyhow!(refusal.body.message);
    match command {
        TrackerCommand::Invite { .. } => unreachable!("handled by the caller"),
        TrackerCommand::Hide { bundle, file } | TrackerCommand::Unhide { bundle, file } => {
            let hide = matches!(command, TrackerCommand::Hide { .. });
            open_existing(&file.catalog)?
                .set_hidden(bundle, hide)
                .map_err(refused)?;
            let done = if hide { "hid" } else { "showed" };
            tracing::info!("operator {done} {bundle}");
            writeln!(out, "{done} {bundle}")?;
        }
        TrackerCommand::Ban { key, file } | TrackerCommand::Unban { key, file } => {
            let ban = matches!(command, TrackerCommand::Ban { .. });
            open_existing(&file.catalog)?
                .set_banned(key, ban)
                .map_err(refused)?;
            let done = if ban { "banned" } else { "unbanned" };
            tracing::info!("operator {done} {key}");
            writeln!(out, "{done} {key}")?;
        }
        TrackerCommand::Reports { bundle, file } => {
            let catalog = open_existing(&file.catalog)?;
            let reports = catalog.reports(bundle.as_ref())?;
            let now = unix_now();
            for report in &reports {
                let taken_down = if report.blocked { "  (taken down)" } else { "" };
                writeln!(out, "bundle     {}{taken_down}", report.bundle)?;
                writeln!(out, "publisher  {}", report.publisher)?;
                writeln!(out, "reporter   {}", report.reporter)?;
                writeln!(
                    out,
                    "reported   {} ago",
                    ago(now.saturating_sub(report.created))
                )?;
                for line in report.reason.lines() {
                    writeln!(out, "  {line}")?;
                }
                writeln!(out)?;
            }
            if reports.is_empty() {
                writeln!(out, "no reports")?;
            }
        }
    }
    Ok(())
}

/// A span of `secs` seconds as its largest whole unit.
fn ago(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        3600..86_400 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
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
            Err(error) => {
                tracing::warn!("cannot bind {ip} port {port}: {error}");
                last = Some(error);
            }
        }
    }
    let error = last.expect("at least one candidate");
    Err(error).with_context(|| format!("cannot bind TCP port {port}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_show_their_largest_unit() {
        assert_eq!(ago(5), "5s");
        assert_eq!(ago(150), "2m");
        assert_eq!(ago(7_300), "2h");
        assert_eq!(ago(200_000), "2d");
    }

    #[test]
    fn limits_parse() {
        assert_eq!(
            parse_limit("comment=5/60"),
            Ok((Action::Comment, Some(Rate::new(5, 60))))
        );
        assert_eq!(parse_limit("sign-in=off"), Ok((Action::SignIn, None)));
        for bad in ["comment", "comment=5", "comment=a/b", "talk=1/1"] {
            assert!(parse_limit(bad).is_err(), "{bad}");
        }
    }
}
