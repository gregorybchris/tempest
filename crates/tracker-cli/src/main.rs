//! The `tempest-tracker` binary: runs a tracker and operates its catalog (D-236).

mod tracker;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    match tracker::tracker(tracker::TrackerArgs::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
