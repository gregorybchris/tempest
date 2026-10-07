//! `tempest share`: build a bundle's manifest, print its link, and serve it.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use tempest_net::{DEFAULT_PORT, Storage, StorageConfig};
use tempest_protocol::{Link, Manifest};

use crate::bundle::{BundleInfo, build_manifest, unix_millis};
use crate::peer::PeerArgs;
use crate::progress::{StatusLine, bytes};

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
    /// The bundle's creation time, in milliseconds since the Unix epoch. Defaults to now.
    #[arg(long, allow_hyphen_values = true)]
    created: Option<i64>,
    /// The UDP port to accept peers on, 0 for any.
    #[arg(long, default_value_t = DEFAULT_PORT)]
    port: u16,
    #[command(flatten)]
    peer: PeerArgs,
}

pub fn share(args: ShareArgs) -> Result<()> {
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
    if args.dry_run {
        println!("link      {link}");
        return Ok(());
    }
    let runtime = tokio::runtime::Runtime::new().context("cannot start the async runtime")?;
    runtime.block_on(serve(args, manifest, link))
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
    let peer = args.peer.start(args.port, true)?;
    let mut handle = peer.node.share(storage).await?;

    link.peer = Some(peer.node.endpoint().peer_id());
    link.addrs = peer.addresses.clone();
    println!("peer      {}", peer.node.endpoint().peer_id());
    println!("link      {link}");
    if link.addrs.is_empty() {
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
                    "{} uploaded · {} {}",
                    bytes(status.uploaded),
                    status.connections,
                    if status.connections == 1 { "peer" } else { "peers" },
                ));
            }
        }
    };
    line.clear();
    if !stopped {
        eprintln!("stopping");
    }
    peer.stop().await;
    let summary = handle.finished().await?;
    eprintln!("uploaded {}", bytes(summary.uploaded));
    Ok(())
}

/// The directory the bundle's entry paths are relative to: the folder itself, or the folder
/// holding a single shared file.
fn base_dir(path: &Path) -> Result<PathBuf> {
    let absolute =
        fs::canonicalize(path).with_context(|| format!("cannot read {}", path.display()))?;
    if absolute.is_dir() {
        return Ok(absolute);
    }
    let parent = absolute.parent().context("a file always has a parent")?;
    Ok(parent.to_owned())
}
