//! The `tempest` binary.

mod bundle;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::SystemTime;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use tempest_protocol::Link;

use crate::bundle::{BundleInfo, build_manifest, unix_millis};

/// Tempest: peer-to-peer file sharing.
#[derive(Debug, Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create a bundle from a file or folder and print its link.
    Share(ShareArgs),
}

#[derive(Debug, clap::Args)]
struct ShareArgs {
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
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Share(args) => share(args),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn share(args: ShareArgs) -> Result<()> {
    if !args.dry_run {
        bail!("sharing over the network is not available yet; pass --dry-run");
    }
    let info = BundleInfo {
        name: args.name,
        comment: args.comment,
        created: args
            .created
            .unwrap_or_else(|| unix_millis(SystemTime::now())),
    };
    let manifest = build_manifest(&args.path, info)?;

    let bundle_id = manifest.bundle_id();
    let mut link = Link::new(bundle_id);
    link.name = Some(manifest.name().to_owned());
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
    println!("bundle    {bundle_id}");
    println!("link      {link}");
    Ok(())
}
