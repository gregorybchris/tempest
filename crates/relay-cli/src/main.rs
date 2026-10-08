//! The `tempest-relay` binary: runs a relay (D-236).

mod relay;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    match relay::relay(relay::RelayArgs::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
