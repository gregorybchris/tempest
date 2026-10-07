//! The `tempest` binary.

mod daemon;
mod engine;
mod get;
mod peer;
mod progress;
mod share;
mod sim;
mod tracker;
mod transfers;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// Tempest: peer-to-peer file sharing.
#[derive(Debug, Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Share a file or folder and print its link. A running daemon shares it; otherwise it is
    /// served from here until Ctrl-C.
    Share(share::ShareArgs),
    /// Download a bundle from a link, through the daemon if one is running.
    Get(get::GetArgs),
    /// Show the engine's transfers.
    List(transfers::ListArgs),
    /// Start transfers.
    Start(transfers::StartArgs),
    /// Stop transfers.
    Stop(transfers::StopArgs),
    /// Remove transfers, keeping their files unless asked not to.
    Remove(transfers::RemoveArgs),
    /// Run the engine, which does every transfer and serves the API that the CLI and UIs use.
    Daemon(daemon::DaemonArgs),
    /// Simulate swarms.
    #[command(subcommand)]
    Sim(sim::SimCommand),
    /// Run a tracker, which helps peers in public swarms find each other.
    Tracker(tracker::TrackerArgs),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Share(args) => share::share(args),
        Command::Get(args) => get::get(args),
        Command::List(args) => transfers::list(args),
        Command::Start(args) => transfers::start(args),
        Command::Stop(args) => transfers::stop(args),
        Command::Remove(args) => transfers::remove(args),
        Command::Daemon(args) => daemon::daemon(args),
        Command::Sim(command) => sim::sim(command),
        Command::Tracker(args) => tracker::tracker(args),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
