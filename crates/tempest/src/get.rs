//! `tempest get`: download a bundle from the peer its link names, the peers its trackers list
//! and the peers on the local network: through the daemon if one is running, otherwise from
//! here.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use tempest_api::client::{ApiClient, ClientError};
use tempest_core::event::CloseCause;
use tempest_core::{Event, EventKind};
use tempest_engine::api::AddRequest;
use tempest_engine::{Status as TransferStatus, TransferInfo};
use tempest_net::{BundleHandle, Destination, Status};
use tempest_protocol::link::ParseLinkError;
use tempest_protocol::message::{GoodbyeReason, PeerRecord};
use tempest_protocol::{Address, BundleId, Link};
use tokio::sync::broadcast::error::RecvError;
use tokio::time::Instant;

use tempest_api::units::{bytes, percent};
use tempest_api::words::peers;

use crate::engine::{EngineArgs, Target};
use crate::peer::{Key, Peer, PeerArgs};
use crate::progress::{Interrupted, StatusLine, TimedOut, duration};

/// `tempest get --help`'s account of its exit codes.
pub const EXIT_CODES: &str = "\
Exit codes:
  0    the bundle downloaded
  1    the download failed
  2    the arguments are wrong
  3    no progress within --timeout
  130  interrupted with Ctrl-C";

#[derive(Debug, clap::Args)]
pub struct GetArgs {
    /// The bundle's tempest: link, as `tempest share` prints it. Quote it: it contains `&`.
    link: String,
    /// The folder to download into. The bundle is written to a new folder inside it, named
    /// after the bundle.
    #[arg(long, short, default_value = ".", value_name = "DIR")]
    output: PathBuf,
    /// Give up after this many seconds without progress, such as when the sharer is
    /// unreachable.
    #[arg(long, default_value_t = 30, value_name = "SECONDS")]
    timeout: u64,
    /// A tracker to find peers through, besides those the link names. Repeat for more.
    #[arg(long, value_name = "URL")]
    tracker: Vec<String>,
    /// Download from here even if a daemon is running, as --port, --identity, --bind, --relay,
    /// --no-lan, --no-port-mapping and --lan-port also do.
    #[arg(long)]
    standalone: bool,
    /// The UDP port to use, 0 for any, which is the default.
    #[arg(long, value_name = "PORT")]
    port: Option<u16>,
    #[command(flatten)]
    peer: PeerArgs,
    #[command(flatten)]
    engine: EngineArgs,
}

pub fn get(args: GetArgs) -> Result<()> {
    let link = parse_link(&args.link)?;
    let record = link
        .peer
        .filter(|_| !link.addrs.is_empty() || link.relay.is_some())
        .map(|peer_id| PeerRecord {
            peer_id,
            addresses: link.addrs.clone(),
            relay: link.relay,
        });
    let mut trackers = link.trackers.clone();
    for tracker in &args.tracker {
        if !trackers.contains(tracker) {
            trackers.push(tracker.clone());
        }
    }
    let standalone = args.standalone || args.port.is_some() || args.peer.any_given();
    let runtime = tokio::runtime::Runtime::new().context("cannot start the async runtime")?;
    if !standalone {
        let config = args.engine.load()?;
        if let Some(client) = runtime.block_on(Target::daemon(&config))? {
            let link = Link { trackers, ..link };
            return runtime.block_on(through_daemon(&client, &args, &link));
        }
    }
    if record.is_none() && trackers.is_empty() && args.peer.no_lan {
        bail!(
            "the link names neither a peer with an address nor a tracker, and LAN discovery is \
             off"
        );
    }
    runtime.block_on(download(args, link.bundle, record, trackers))
}

/// The link `text` holds, or why it isn't one.
pub fn parse_link(text: &str) -> Result<Link> {
    text.parse().map_err(|error| match error {
        ParseLinkError::NotALink => anyhow!(
            "{text:?} is not a link; a link starts with tempest:? and `tempest share` prints one"
        ),
        error => {
            anyhow!("the link is damaged or cut short ({error}); quote it, since it contains &")
        }
    })
}

async fn download(
    args: GetArgs,
    bundle: BundleId,
    record: Option<PeerRecord>,
    trackers: Vec<String>,
) -> Result<()> {
    let peer = (args.peer).start(args.port.unwrap_or(0), false, Key::Fresh, &args.engine)?;
    let sources = Sources {
        addresses: (record.iter())
            .flat_map(|r| r.addresses.iter().chain(&r.relay).copied())
            .collect(),
        trackers: trackers.len(),
        lan: peer.lan.is_some(),
    };
    if !sources.any() {
        bail!(
            "the link names neither a peer with an address nor a tracker, and LAN discovery \
             could not start"
        );
    }
    let destination = Destination::NewFolderIn(args.output.clone());
    let mut handle = peer
        .node
        .download(bundle, destination, record.into_iter().collect())
        .await?;
    let announcers = peer.announce(&trackers, &[], &handle)?;
    if let Some(lan) = &peer.lan {
        lan.watch(&handle).await;
    }
    let started = Instant::now();
    let timeout = Duration::from_secs(args.timeout);
    let mut watch = Watch::new(&sources, timeout);
    let outcome = watch.until_complete(&mut handle).await;

    peer.stop().await;
    Peer::stop_announcing(announcers).await;
    // A storage failure ends the bundle early, and explains more than the timeout it causes.
    handle.finished().await.context("the download failed")?;
    let status = outcome?;
    let manifest = status.manifest.context("complete without a manifest")?;
    let folder = match manifest.single_file() {
        Some(path) => args.output.join(path),
        None => args.output.join(manifest.name()),
    };
    eprintln!(
        "downloaded {} in {}",
        bytes(manifest.total_size()),
        duration(started.elapsed())
    );
    println!("{}", folder.display());
    Ok(())
}

/// Adds the download to the daemon and follows it until it completes. Interrupted, it leaves
/// the download to the daemon.
async fn through_daemon(client: &ApiClient, args: &GetArgs, link: &Link) -> Result<()> {
    let output = std::path::absolute(&args.output).context("cannot find the output folder")?;
    let request = AddRequest {
        link: Some(link.to_string()),
        save_folder: Some(output),
        ..AddRequest::default()
    };
    let added = match client.add(&request).await {
        Ok(added) => added,
        // Already there: follow it, starting it if needed.
        Err(ClientError::Refused(error)) if error.error == "already_added" => {
            let existing = client.transfer(link.bundle).await?;
            match existing.status {
                TransferStatus::Stopped | TransferStatus::Errored { .. } => {
                    client.start(link.bundle).await?
                }
                _ => existing,
            }
        }
        Err(error) => return Err(error.into()),
    };
    eprintln!(
        "downloading through the daemon at {}; Ctrl-C leaves it to the daemon",
        client.url()
    );
    let started = Instant::now();
    let mut line = StatusLine::new();
    let result = follow(client, added, Duration::from_secs(args.timeout), &mut line).await;
    line.clear();
    let done = result?;
    let mut folder = done
        .content_dir
        .context("complete without a content folder")?;
    // A single file lands in the save folder itself; print the file.
    if folder == done.save_folder {
        let files = client.files(link.bundle).await?;
        if let [file] = files.as_slice() {
            folder.push(&file.path);
        }
    }
    eprintln!(
        "downloaded {} in {}",
        bytes(done.size.unwrap_or(0)),
        duration(started.elapsed())
    );
    println!("{}", folder.display());
    Ok(())
}

/// Polls a download in the daemon until it completes, fails, or makes no progress for
/// `timeout`.
async fn follow(
    client: &ApiClient,
    mut transfer: TransferInfo,
    timeout: Duration,
    line: &mut StatusLine,
) -> Result<TransferInfo> {
    let id = transfer.id;
    let mut deadline = Instant::now() + timeout;
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    loop {
        if transfer.completed.is_some() {
            return Ok(transfer);
        }
        if let TransferStatus::Errored { reason } = &transfer.status {
            bail!("the download failed: {reason}");
        }
        if transfer.status == TransferStatus::Stopped {
            bail!("the download was stopped");
        }
        line.show(&format!(
            "{}  {} of {}  {}  {}",
            transfer.name,
            bytes(transfer.downloaded.min(transfer.size.unwrap_or(u64::MAX))),
            transfer.size.map_or("?".to_owned(), bytes),
            percent(transfer.progress),
            peers(transfer.connections, transfer.relayed),
        ));
        tokio::select! {
            _ = tick.tick() => {}
            _ = tokio::time::sleep_until(deadline) => {
                let why = match &transfer.status {
                    TransferStatus::Stalled { reason } => format!(": {reason}"),
                    _ => ".".to_owned(),
                };
                return Err(TimedOut(format!(
                    "no progress for {} s{why} The daemon keeps trying; `tempest list` shows it.",
                    timeout.as_secs()
                ))
                .into());
            }
            interrupted = &mut interrupt => {
                interrupted.context("cannot listen for Ctrl-C")?;
                return Err(Interrupted("the daemon keeps downloading it; `tempest list` shows it").into());
            }
        }
        let next = client.transfer(id).await.map_err(|error| match error {
            ClientError::Refused(e) if e.error == "not_found" => {
                anyhow!("the download was removed")
            }
            other => other.into(),
        })?;
        if (next.have, next.size) != (transfer.have, transfer.size) {
            deadline = Instant::now() + timeout;
        }
        transfer = next;
    }
}

/// Where a download looks for peers, to explain a failure.
struct Sources {
    /// The addresses of the peer the link names.
    addresses: Vec<Address>,
    /// How many trackers it asks.
    trackers: usize,
    /// Whether it looks on the local network.
    lan: bool,
}

impl Sources {
    fn any(&self) -> bool {
        !self.addresses.is_empty() || self.trackers > 0 || self.lan
    }

    fn describe(&self) -> String {
        let addresses: Vec<String> = self.addresses.iter().map(ToString::to_string).collect();
        let trackers = match self.trackers {
            0 => None,
            1 => Some("peers from 1 tracker".to_owned()),
            n => Some(format!("peers from {n} trackers")),
        };
        let addresses = (!addresses.is_empty()).then(|| addresses.join(", "));
        let lan = self.lan.then(|| "peers on the local network".to_owned());
        [addresses, trackers, lan]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" or ")
    }
}

/// Follows a download, showing its progress, until it completes or stalls.
struct Watch<'a> {
    sources: &'a Sources,
    timeout: Duration,
    line: StatusLine,
    /// Why the last attempt to reach the sharer failed, if one did.
    trouble: Option<String>,
}

impl<'a> Watch<'a> {
    fn new(sources: &'a Sources, timeout: Duration) -> Self {
        Self {
            sources,
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
                    return Err(Interrupted("the download stopped").into());
                }
            }
        }
    }

    /// Remembers why a peer couldn't be reached or closed the connection.
    fn note(&mut self, event: Event) {
        match event.kind {
            EventKind::DialFailed { .. } => {
                self.trouble = Some("no connection could be made".to_owned());
            }
            EventKind::Closed {
                cause: CloseCause::Remote { reason, message },
                ..
            } => {
                let mut trouble = format!("a peer closed the connection: {}", why(reason));
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
            format!(
                "could not get the bundle from {} within {seconds} s",
                self.sources.describe()
            )
        } else {
            format!("no progress for {seconds} s")
        };
        if let Some(trouble) = &self.trouble {
            message.push_str(": ");
            message.push_str(trouble);
        }
        TimedOut(message).into()
    }
}

/// One line of progress: what is stored so far, out of what.
fn progress(status: &Status) -> String {
    let Some(manifest) = &status.manifest else {
        return "connecting…".to_owned();
    };
    let total = manifest.total_size();
    let done = match status.chunks {
        0 => 1.0,
        chunks => f64::from(status.have) / f64::from(chunks),
    };
    format!(
        "{}  {} of {}  {}  {}",
        manifest.name(),
        bytes(status.downloaded.min(total)),
        bytes(total),
        percent(done),
        peers(status.connections, status.relayed),
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
