//! The `tempest-sim` binary: Tempest's simulator (D-214).

mod sim;
mod sim_web;

use std::process::ExitCode;

use clap::Parser;

/// Tempest's simulator: runs scenarios and browses their results.
#[derive(Debug, Parser)]
#[command(name = "tempest-sim", version)]
struct Cli {
    #[command(subcommand)]
    command: sim::SimCommand,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_logging();
    match sim::sim(cli.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Prints log events to stderr: warnings and errors, or what `RUST_LOG` asks for (D-243).
fn init_logging() {
    use std::io::IsTerminal;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .init();
}
