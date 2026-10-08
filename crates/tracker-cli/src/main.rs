//! The `tempest-tracker` binary: runs a tracker and operates its catalog (D-236).

mod tracker;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    // Each connection takes a descriptor. Best effort: at the old limit, the server still runs.
    let _ = tempest_net::raise_open_file_limit();
    match tracker::tracker(tracker::TrackerArgs::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
