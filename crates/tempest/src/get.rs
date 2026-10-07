//! `tempest get`: download a bundle from the peer its link names.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use tempest_core::event::CloseCause;
use tempest_core::{Event, EventKind};
use tempest_net::{BundleHandle, Destination, Status};
use tempest_protocol::message::{GoodbyeReason, PeerRecord};
use tempest_protocol::{Address, Link};
use tokio::sync::broadcast::error::RecvError;
use tokio::time::Instant;

use crate::peer::PeerArgs;
use crate::progress::{StatusLine, bytes, duration};

#[derive(Debug, clap::Args)]
pub struct GetArgs {
    /// The bundle's tempest: link.
    link: String,
    /// The folder to download into. The bundle is written to a new folder inside it, named
    /// after the bundle.
    #[arg(long, short, default_value = ".")]
    output: PathBuf,
    /// Give up after this many seconds without progress, such as when the sharer is
    /// unreachable.
    #[arg(long, default_value_t = 30, value_name = "SECONDS")]
    timeout: u64,
    /// The UDP port to use, 0 for any.
    #[arg(long, default_value_t = 0)]
    port: u16,
    #[command(flatten)]
    peer: PeerArgs,
}

pub fn get(args: GetArgs) -> Result<()> {
    let link: Link = args
        .link
        .parse()
        .with_context(|| format!("not a valid tempest: link: {}", args.link))?;
    let Some(peer_id) = link.peer else {
        bail!(
            "the link names no peer to download from \
             (finding peers through trackers is not supported yet)"
        );
    };
    if link.addrs.is_empty() {
        bail!(
            "the link has no address for its peer \
             (reaching peers through relays is not supported yet)"
        );
    }
    let record = PeerRecord {
        peer_id,
        addresses: link.addrs.clone(),
        relay: None,
    };
    let runtime = tokio::runtime::Runtime::new().context("cannot start the async runtime")?;
    runtime.block_on(download(args, link, record))
}

async fn download(args: GetArgs, link: Link, record: PeerRecord) -> Result<()> {
    let addresses = record.addresses.clone();
    let peer = args.peer.start(args.port, false)?;
    let destination = Destination::NewFolderIn(args.output.clone());
    let mut handle = peer
        .node
        .download(link.bundle, destination, vec![record])
        .await?;
    let started = Instant::now();
    let timeout = Duration::from_secs(args.timeout);
    let mut watch = Watch::new(&addresses, timeout);
    let outcome = watch.until_complete(&mut handle).await;

    peer.stop().await;
    // A storage failure ends the bundle early, and explains more than the timeout it causes.
    handle.finished().await.context("the download failed")?;
    let status = outcome?;
    let manifest = status.manifest.context("complete without a manifest")?;
    let folder = args.output.join(manifest.name());
    eprintln!(
        "downloaded {} in {}",
        bytes(manifest.total_size()),
        duration(started.elapsed())
    );
    println!("{}", folder.display());
    Ok(())
}

/// Follows a download, showing its progress, until it completes or stalls.
struct Watch<'a> {
    addresses: &'a [Address],
    timeout: Duration,
    line: StatusLine,
    /// Why the last attempt to reach the sharer failed, if one did.
    trouble: Option<String>,
}

impl<'a> Watch<'a> {
    fn new(addresses: &'a [Address], timeout: Duration) -> Self {
        Self {
            addresses,
            timeout,
            line: StatusLine::new(),
            trouble: None,
        }
    }

    /// Waits for the download to complete, and returns its last status.
    async fn until_complete(&mut self, handle: &mut BundleHandle) -> Result<Status> {
        let result = self.follow(handle).await;
        self.line.clear();
        result
    }

    async fn follow(&mut self, handle: &mut BundleHandle) -> Result<Status> {
        let mut deadline = Instant::now() + self.timeout;
        let mut last = Status::default();
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        let interrupt = tokio::signal::ctrl_c();
        tokio::pin!(interrupt);
        let mut events_open = true;
        loop {
            tokio::select! {
                changed = handle.status.changed() => {
                    if changed.is_err() {
                        bail!("the download stopped");
                    }
                    let status = handle.status.borrow_and_update().clone();
                    if status.complete {
                        return Ok(status);
                    }
                    if status.manifest.is_some() != last.manifest.is_some()
                        || status.have != last.have
                    {
                        deadline = Instant::now() + self.timeout;
                    }
                    last = status;
                }
                event = handle.events.recv(), if events_open => match event {
                    Ok(event) => self.note(event),
                    Err(RecvError::Lagged(_)) => {}
                    // Only the bundle stopping closes the channel, which the status shows too.
                    Err(RecvError::Closed) => events_open = false,
                },
                _ = tick.tick() => self.line.show(&progress(&last)),
                _ = tokio::time::sleep_until(deadline) => return Err(self.stalled(&last)),
                interrupted = &mut interrupt => {
                    interrupted.context("cannot listen for Ctrl-C")?;
                    bail!("interrupted");
                }
            }
        }
    }

    /// Remembers why the sharer couldn't be reached or closed the connection.
    fn note(&mut self, event: Event) {
        match event.kind {
            EventKind::DialFailed { .. } => {
                self.trouble = Some("no connection could be made".to_owned());
            }
            EventKind::Closed {
                cause: CloseCause::Remote { reason, message },
                ..
            } => {
                let mut trouble = format!("the sharer closed the connection: {}", why(reason));
                if !message.is_empty() {
                    trouble.push_str(&format!(" ({message})"));
                }
                self.trouble = Some(trouble);
            }
            EventKind::Closed {
                cause: CloseCause::Lost,
                ..
            } => self.trouble = Some("the connection was lost".to_owned()),
            _ => {}
        }
    }

    fn stalled(&self, status: &Status) -> anyhow::Error {
        let seconds = self.timeout.as_secs();
        let mut message = if status.manifest.is_none() {
            let addresses: Vec<String> = self.addresses.iter().map(ToString::to_string).collect();
            format!(
                "could not get the bundle from {} within {seconds} s",
                addresses.join(", ")
            )
        } else {
            format!("no progress for {seconds} s")
        };
        if let Some(trouble) = &self.trouble {
            message.push_str(": ");
            message.push_str(trouble);
        }
        anyhow!(message)
    }
}

/// One line of progress: what is stored so far, out of what.
fn progress(status: &Status) -> String {
    let Some(manifest) = &status.manifest else {
        return "connecting…".to_owned();
    };
    let total = manifest.total_size();
    let percent = match status.chunks {
        0 => 100,
        chunks => u64::from(status.have) * 100 / u64::from(chunks),
    };
    format!(
        "{}  {} of {}  {percent}%  {} {}",
        manifest.name(),
        bytes(status.downloaded.min(total)),
        bytes(total),
        status.connections,
        if status.connections == 1 {
            "peer"
        } else {
            "peers"
        },
    )
}

/// A Goodbye reason in words.
fn why(reason: GoodbyeReason) -> &'static str {
    match reason {
        GoodbyeReason::Done => "done",
        GoodbyeReason::ShuttingDown => "it is shutting down",
        GoodbyeReason::UnknownBundle => "it does not have this bundle",
        GoodbyeReason::Duplicate => "duplicate connection",
        GoodbyeReason::ProtocolViolation => "protocol violation",
        GoodbyeReason::BadData => "bad data",
        GoodbyeReason::TooManyConnections => "it has too many connections",
    }
}
