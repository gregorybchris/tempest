//! The `tempest-sim` binary: Tempest's simulator (D-214).

mod sim;
mod sim_web;

use std::process::ExitCode;

use clap::Parser;

/// Tempest's simulator: runs scenarios and browses their results.
#[derive(Debug, Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: sim::SimCommand,
}

fn main() -> ExitCode {
    match sim::sim(Cli::parse().command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
