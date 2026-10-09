//! The `tempest` binary.

mod daemon;
mod engine;
mod get;
mod handoff;
mod peer;
mod progress;
mod publish;
mod share;
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
    #[command(after_help = get::EXIT_CODES)]
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
    /// List a transfer on a tracker, or change its listing.
    Publish(publish::PublishArgs),
    /// Remove a transfer's listing from a tracker, and stop asking the tracker to list it.
    Unpublish(publish::UnpublishArgs),
    /// Sign a browser in to a tracker's website: sign the code the website shows with this
    /// installation's key.
    SignIn(publish::SignInArgs),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    // The terminal UI draws on the terminal, which log lines would garble.
    if !matches!(cli.command, Command::Tui(_)) {
        init_logging();
    }
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
        Command::Publish(args) => publish::publish(args),
        Command::Unpublish(args) => publish::unpublish(args),
        Command::SignIn(args) => publish::sign_in(args),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) if error.is::<progress::Interrupted>() => {
            eprintln!("{error}");
            ExitCode::from(130)
        }
        Err(error) if error.is::<progress::TimedOut>() => {
            eprintln!("error: {}", message(&error));
            ExitCode::from(progress::TimedOut::EXIT_CODE)
        }
        Err(error) => {
            eprintln!("error: {}", message(&error));
            ExitCode::FAILURE
        }
    }
}

/// Prints log events to stderr: warnings and errors, or what `RUST_LOG` asks for (D-243).
fn init_logging() {
    use std::io::IsTerminal;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,quinn_udp=error"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .init();
}

/// An error and its causes in one line, leaving out a cause its parent already ends with,
/// as errors that quote their source do.
fn message(error: &anyhow::Error) -> String {
    let mut message = error.to_string();
    for cause in error.chain().skip(1) {
        let cause = cause.to_string();
        if !message.ends_with(&cause) {
            message.push_str(": ");
            message.push_str(&cause);
        }
    }
    message
}
