//! The `tempest` binary.

mod bundle;
mod get;
mod peer;
mod progress;
mod share;
mod sim;

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
    /// Share a file or folder: print its link and serve it until Ctrl-C.
    Share(share::ShareArgs),
    /// Download a bundle from a link.
    Get(get::GetArgs),
    /// Simulate swarms.
    #[command(subcommand)]
    Sim(sim::SimCommand),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Share(args) => share::share(args),
        Command::Get(args) => get::get(args),
        Command::Sim(command) => sim::sim(command),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
