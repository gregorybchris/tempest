//! `tempest list`, `start`, `stop` and `remove`: transfers in the engine, through the daemon's
//! API or an engine opened here on its state.

use anyhow::{Context, Result};
use tempest_engine::{Status, TransferInfo};

use crate::engine::{EngineArgs, Target, short_id};
use tempest_api::units::{bytes, duration, percent};

/// What to say when no daemon runs the transfers.
const NO_DAEMON: &str =
    "no daemon is running, so nothing is transferring; `tempest daemon` runs it";

#[derive(Debug, clap::Args)]
pub struct ListArgs {
    #[command(flatten)]
    engine: EngineArgs,
    /// Print every field of every transfer as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
pub struct StartArgs {
    #[command(flatten)]
    engine: EngineArgs,
    /// The transfers: bundle IDs, unique prefixes of them, or names.
    #[arg(required = true, value_name = "TRANSFER")]
    transfers: Vec<String>,
    /// Start at once, bypassing the queue.
    #[arg(long)]
    force: bool,
}

#[derive(Debug, clap::Args)]
pub struct StopArgs {
    #[command(flatten)]
    engine: EngineArgs,
    /// The transfers: bundle IDs, unique prefixes of them, or names.
    #[arg(required = true, value_name = "TRANSFER")]
    transfers: Vec<String>,
}

#[derive(Debug, clap::Args)]
pub struct RemoveArgs {
    #[command(flatten)]
    engine: EngineArgs,
    /// The transfers: bundle IDs, unique prefixes of them, or names.
    #[arg(required = true, value_name = "TRANSFER")]
    transfers: Vec<String>,
    /// Delete the transfers' files too.
    #[arg(long)]
    delete_files: bool,
}

/// Runs `command` against the engine, then closes an embedded one, saying that no daemon runs
/// the transfers.
fn with_target<T>(
    engine: &EngineArgs,
    command: impl AsyncFnOnce(&Target) -> Result<T>,
) -> Result<T> {
    let config = engine.load()?;
    let runtime = tokio::runtime::Runtime::new().context("cannot start the async runtime")?;
    runtime.block_on(async {
        let target = Target::connect(config).await?;
        let result = command(&target).await;
        if !target.is_daemon() {
            eprintln!("note: {NO_DAEMON}");
        }
        target.close().await;
        result
    })
}

pub fn list(args: ListArgs) -> Result<()> {
    let transfers = with_target(&args.engine, async |target| target.list().await)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&transfers)?);
    } else if transfers.is_empty() {
        println!("no transfers");
    } else {
        print!("{}", table(&transfers));
    }
    Ok(())
}

pub fn start(args: StartArgs) -> Result<()> {
    with_target(&args.engine, async |target| {
        for query in &args.transfers {
            let transfer = target.find(query).await?;
            let started = target.start(transfer.id, args.force).await?;
            println!(
                "{}  {}  {}",
                short_id(&started.id),
                started.name,
                started.status.name()
            );
        }
        Ok(())
    })
}

pub fn stop(args: StopArgs) -> Result<()> {
    with_target(&args.engine, async |target| {
        for query in &args.transfers {
            let transfer = target.find(query).await?;
            let stopped = target.stop(transfer.id).await?;
            println!(
                "{}  {}  {}",
                short_id(&stopped.id),
                stopped.name,
                stopped.status.name()
            );
        }
        Ok(())
    })
}

pub fn remove(args: RemoveArgs) -> Result<()> {
    with_target(&args.engine, async |target| {
        for query in &args.transfers {
            let transfer = target.find(query).await?;
            target.remove(transfer.id, args.delete_files).await?;
            let what = match args.delete_files {
                true => "removed with its files",
                false => "removed; its files stay",
            };
            println!("{}  {}  {what}", short_id(&transfer.id), transfer.name);
        }
        Ok(())
    })
}

/// The transfers as a table, with a line for each that explains its status.
fn table(transfers: &[TransferInfo]) -> String {
    let header = [
        "ID", "NAME", "STATUS", "DONE", "SIZE", "DOWN", "UP", "PEERS", "ETA", "RATIO",
    ];
    let rate = |rate: u64| match rate {
        0 => "-".to_owned(),
        rate => format!("{}/s", bytes(rate)),
    };
    let rows: Vec<[String; 10]> = transfers
        .iter()
        .map(|t| {
            [
                short_id(&t.id),
                truncate(&t.name, 40),
                t.status.name().to_owned(),
                percent(t.progress),
                t.size.map_or("-".to_owned(), bytes),
                rate(t.download_rate),
                rate(t.upload_rate),
                t.connections.to_string(),
                t.eta.map_or("-".to_owned(), duration),
                t.ratio
                    .map_or("-".to_owned(), |ratio| format!("{ratio:.2}")),
            ]
        })
        .collect();
    let mut widths = header.map(str::len);
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let line = |cells: &[String]| {
        let padded: Vec<_> = cells
            .iter()
            .zip(widths)
            .map(|(cell, width)| format!("{cell:<width$}"))
            .collect();
        format!("{}\n", padded.join("  ").trim_end())
    };
    let mut table = line(&header.map(str::to_owned));
    for (row, transfer) in rows.iter().zip(transfers) {
        table.push_str(&line(row));
        if let Status::Stalled { reason } | Status::Errored { reason } = &transfer.status {
            table.push_str(&format!("          {reason}\n"));
        }
    }
    table
}

fn truncate(text: &str, max: usize) -> String {
    match text.chars().count() > max {
        true => format!("{}…", text.chars().take(max - 1).collect::<String>()),
        false => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_names_are_truncated() {
        assert_eq!(truncate("short", 8), "short");
        assert_eq!(truncate("much too long", 8), "much to…");
    }
}
