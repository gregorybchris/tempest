//! The `tempest` binary.

mod daemon;
mod engine;
mod get;
mod handoff;
mod peer;
mod progress;
mod publish;
mod relay;
mod share;
mod tracker;
mod transfers;
mod tui;
mod web;

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
    /// Open the terminal UI, on the daemon if one is running, or else on an engine of its own
    /// that stops when it quits, unless asked to hand its transfers to a daemon.
    Tui(tui::TuiArgs),
    /// Open the web UI, on the daemon if one is running, or else on an engine run here until
    /// Ctrl-C, which asks whether to hand its transfers to a daemon.
    Web(web::WebArgs),
    /// Run the engine, which does every transfer and serves the API that the CLI and UIs use,
    /// and the web UI.
    Daemon(daemon::DaemonArgs),
    /// Moved to the `tempest-sim` binary (D-214).
    #[command(hide = true)]
    Sim(SimArgs),
    /// List a transfer on a tracker, or change its listing.
    Publish(publish::PublishArgs),
    /// Remove a transfer's listing from a tracker, and stop asking the tracker to list it.
    Unpublish(publish::UnpublishArgs),
    /// Sign a browser in to a tracker's website: sign the code the website shows with this
    /// installation's key.
    SignIn(publish::SignInArgs),
    /// Run a tracker, which helps peers in public swarms find each other.
    Tracker(tracker::TrackerArgs),
    /// Run a relay, which lends public addresses to peers behind routers.
    Relay(relay::RelayArgs),
}

/// What `tempest sim` was given; it only points to `tempest-sim`.
#[derive(Debug, clap::Args)]
struct SimArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    _args: Vec<String>,
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
        Command::Tui(args) => tui::tui(args),
        Command::Web(args) => web::web(args),
        Command::Daemon(args) => daemon::daemon(args),
        Command::Sim(_) => Err(anyhow::anyhow!(
            "the simulator is now its own program: run `tempest-sim` with the same arguments"
        )),
        Command::Publish(args) => publish::publish(args),
        Command::Unpublish(args) => publish::unpublish(args),
        Command::SignIn(args) => publish::sign_in(args),
        Command::Tracker(args) => tracker::tracker(args),
        Command::Relay(args) => relay::relay(args),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
