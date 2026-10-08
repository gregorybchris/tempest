//! The `tempest-relay` binary: runs a relay (D-236).

mod relay;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    // Each connection takes a descriptor. Best effort: at the old limit, the server still runs.
    let _ = tempest_net::raise_open_file_limit();
    match relay::relay(relay::RelayArgs::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
