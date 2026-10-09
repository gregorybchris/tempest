//! The `tempest-tracker` binary: runs a tracker and operates its catalog (D-236).

mod tracker;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let args = tracker::TrackerArgs::parse();
    init_logging();
    // Each connection takes a descriptor. Best effort: at the old limit, the server still runs.
    let _ = tempest_net::raise_open_file_limit();
    match tracker::tracker(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Prints log events to stderr: information and above, or what `RUST_LOG` asks for (D-239).
fn init_logging() {
    use std::io::IsTerminal;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,quinn_udp=error"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .init();
}
