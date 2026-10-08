//! `tempest share`: build a bundle's manifest, print its link, and serve it: through the
//! daemon if one is running, otherwise from here. With `--publish`, list it on a tracker too.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use tempest_engine::api::AddRequest;
use tempest_engine::api::client::ApiClient;
use tempest_engine::manifest::{BundleInfo, base_dir, build_manifest, unix_millis};
use tempest_net::{DEFAULT_PORT, Storage, StorageConfig, TrackerClient};
use tempest_protocol::{BundleId, Link, Manifest};

use crate::engine::{EngineArgs, Target};
use crate::peer::{Peer, PeerArgs, TRACKER_TIMEOUT};
use tempest_api::units::bytes;
use tempest_api::words::peers;

use crate::progress::StatusLine;
use crate::publish::{ListingArgs, print_listing};

#[derive(Debug, clap::Args)]
pub struct ShareArgs {
    /// The file or folder to share.
    path: PathBuf,
    /// Build the manifest and print the bundle ID and link without sharing.
    #[arg(long)]
    dry_run: bool,
    /// The bundle's name. Defaults to the file or folder's name.
    #[arg(long)]
    name: Option<String>,
    /// A description of the bundle.
    #[arg(long, default_value = "")]
    comment: String,
    /// Serve it from here even if a daemon is running, as --port, --identity, --bind, --relay,
    /// --no-lan and --lan-port also do.
    #[arg(long)]
    standalone: bool,
    /// The bundle's creation time, in milliseconds since the Unix epoch. Defaults to now.
    #[arg(long, allow_hyphen_values = true)]
    created: Option<i64>,
    /// A tracker to announce the bundle to, and to name in the link, such as
    /// `https://tracker.example.org`. Repeat for more. A daemon also uses its config's
    /// trackers.
    #[arg(long, value_name = "URL")]
    tracker: Vec<String>,
    /// List the bundle on the tracker at this URL, signing in with this
    /// installation's key. The tracker lists it once it has fetched the manifest from the
    /// swarm.
    #[arg(long, value_name = "URL", conflicts_with = "dry_run")]
    publish: Option<String>,
    #[command(flatten)]
    listing: ListingArgs,
    #[arg(long, value_name = "PORT", help = format!(
        "The UDP port to accept peers on, 0 for any [default: {DEFAULT_PORT}]"
    ))]
    port: Option<u16>,
    #[command(flatten)]
    peer: PeerArgs,
    #[command(flatten)]
    engine: EngineArgs,
}

impl ShareArgs {
    /// The trackers to announce to and name in the link: those given, and the one to publish
    /// on.
    fn trackers(&self) -> Vec<String> {
        let mut trackers = self.tracker.clone();
        let publish = self.publish.iter().filter(|t| !self.tracker.contains(t));
        trackers.extend(publish.cloned());
        trackers
    }

    /// Whether to serve from here rather than through a daemon.
    fn standalone(&self) -> bool {
        self.standalone || self.port.is_some() || self.peer.any_given()
    }
}

pub fn share(args: ShareArgs) -> Result<()> {
    if args.publish.is_none() && args.listing.edit() != Default::default() {
        bail!("--title, --description, --category and --tag describe a listing; add --publish");
    }
    if !args.dry_run && !args.standalone() {
        let config = args.engine.load()?;
        let runtime = tokio::runtime::Runtime::new().context("cannot start the async runtime")?;
        if let Some(client) = runtime.block_on(Target::daemon(&config))? {
            return runtime.block_on(through_daemon(&client, &args));
        }
    }
    let info = BundleInfo {
        name: args.name.clone(),
        comment: args.comment.clone(),
        created: args
            .created
            .unwrap_or_else(|| unix_millis(SystemTime::now())),
    };
    let manifest = build_manifest(&args.path, info)?;
    print_summary(&manifest);

    let mut link = Link::new(manifest.bundle_id());
    link.name = Some(manifest.name().to_owned());
    link.trackers = args.trackers();
    if args.dry_run {
        println!("link      {link}");
        return Ok(());
    }
    let runtime = tokio::runtime::Runtime::new().context("cannot start the async runtime")?;
    runtime.block_on(serve(args, manifest, link))
}

/// Has the daemon share the file or folder.
async fn through_daemon(client: &ApiClient, args: &ShareArgs) -> Result<()> {
    let path = std::fs::canonicalize(&args.path)
        .with_context(|| format!("cannot read {}", args.path.display()))?;
    let request = AddRequest {
        path: Some(path),
        name: args.name.clone(),
        comment: args.comment.clone(),
        trackers: args.trackers(),
        created: args.created,
        ..AddRequest::default()
    };
    let added = client.add(&request).await?;
    println!("name      {}", added.name);
    if let Some(size) = added.size {
        println!("size      {} bytes", size);
    }
    println!("bundle    {}", added.id);
    println!("link      {}", added.link);
    if let Some(tracker) = &args.publish {
        let listing = client
            .publish(added.id, &args.listing.request(tracker))
            .await?;
        print_listing(tracker, &listing);
    }
    eprintln!(
        "the daemon at {} is sharing it; `tempest list` shows it, and `tempest remove` stops it",
        client.url()
    );
    Ok(())
}

/// Signs `peer`'s key in to `tracker` and sets `bundle`'s listing there.
async fn publish(
    peer: &Peer,
    tracker: &str,
    listing: &ListingArgs,
    bundle: BundleId,
) -> Result<()> {
    let client = TrackerClient::new(TRACKER_TIMEOUT).context("cannot set up HTTPS")?;
    let invite = listing.invite.as_deref();
    let session = client.sign_in(tracker, &peer.identity, invite).await?;
    let edited = client
        .edit_listing(tracker, &session.token, &bundle, &listing.edit())
        .await;
    // The session was only for this; it would expire on its own.
    let _ = client.sign_out(tracker, &session.token).await;
    print_listing(tracker, &edited?);
    Ok(())
}

fn print_summary(manifest: &Manifest) {
    let files = manifest
        .entries()
        .iter()
        .filter(|entry| entry.as_file().is_some())
        .count();
    println!("name      {}", manifest.name());
    println!(
        "entries   {} ({files} files, {} bytes)",
        manifest.entries().len(),
        manifest.total_size()
    );
    println!("manifest  {} bytes", manifest.encoded_len());
    println!("bundle    {}", manifest.bundle_id());
}

/// Serves the bundle until Ctrl-C.
async fn serve(args: ShareArgs, manifest: Manifest, mut link: Link) -> Result<()> {
    let base = base_dir(&args.path)?;
    let storage = Storage::open_shared(base, Arc::new(manifest), StorageConfig::default())
        .await
        .context("cannot open the shared files")?;
    let peer = args.peer.start(args.port.unwrap_or(DEFAULT_PORT), true)?;
    if let Some(tracker) = &args.publish {
        let published = publish(&peer, tracker, &args.listing, link.bundle).await;
        if let Err(error) = published {
            peer.stop().await;
            return Err(error.context(format!("cannot publish to {tracker}")));
        }
    }
    let mut handle = peer.node.share(storage).await?;
    let publish: Vec<String> = args.publish.iter().cloned().collect();
    let announcers = peer.announce(&link.trackers, &publish, &handle)?;

    let (relay, mapped) = tokio::join!(peer.home_relay(), peer.port_mapping());
    link.peer = Some(peer.node.endpoint().peer_id());
    link.addrs = peer
        .node
        .endpoint()
        .port_mapping()
        .advertise(&peer.addresses);
    link.relay = relay;
    println!("peer      {}", peer.node.endpoint().peer_id());
    if let Some(mapped) = mapped {
        println!("mapped    {mapped}");
    }
    if let Some(relay) = link.relay {
        println!("relay     {relay}");
    }
    println!("link      {link}");
    if link.addrs.is_empty() && link.relay.is_none() {
        eprintln!("warning: no network address to put in the link; pass --bind to choose one");
    }
    eprintln!("sharing; press Ctrl-C to stop");

    let mut line = StatusLine::new();
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    let stopped = loop {
        tokio::select! {
            interrupted = &mut interrupt => {
                interrupted.context("cannot listen for Ctrl-C")?;
                break false;
            }
            changed = handle.status.changed() => if changed.is_err() {
                // The bundle stopped by itself, which only a storage failure does.
                break true;
            },
            _ = tick.tick() => {
                let status = handle.status.borrow().clone();
                line.show(&format!(
                    "{} uploaded · {}",
                    bytes(status.uploaded),
                    peers(status.connections, status.relayed),
                ));
            }
        }
    };
    line.clear();
    if !stopped {
        eprintln!("stopping");
    }
    peer.stop().await;
    Peer::stop_announcing(announcers).await;
    let summary = handle.finished().await?;
    eprintln!("uploaded {}", bytes(summary.uploaded));
    Ok(())
}
