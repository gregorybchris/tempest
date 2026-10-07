//! The production driver: `tempest-core` sessions on tokio, over QUIC (D-068, D-085).
//!
//! A [`Node`] owns an [`Endpoint`] and runs one session per bundle. It is a set of actors that
//! talk over bounded channels and share no mutable state:
//!
//! - The node task accepts connections and keeps the table of bundles. Each incoming connection
//!   gets a routing task, which reads Hello from the control stream and hands the connection to
//!   the bundle it names, or closes it with `unknown_bundle`.
//! - A bundle task owns a [`Session`](tempest_core::Session) and the bundle's [`Storage`]. It
//!   feeds the session what happened, carries out its actions, and sleeps until its next timer.
//! - A connection task per connection reads the control stream, writes what the session sends
//!   on it, accepts request streams and reports the round-trip time.
//! - A task per request stream sends one request or answers one, so a slow stream holds up
//!   nothing else.
//!
//! Tasks report to their bundle by awaiting a send on its inbox, which pushes back on reading
//! from the network when the bundle falls behind. The bundle never waits on them: it hands them
//! work with `try_send` or a oneshot, so no cycle of full channels can deadlock.

mod bundle;
mod conn;
mod request;

use std::collections::{HashMap, hash_map};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use quinn::{RecvStream, SendStream};
use tempest_core::{Config, Event};
use tempest_protocol::message::{GoodbyeReason, Hello, PeerRecord};
use tempest_protocol::{Address, BundleId, Manifest, Message};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use self::bundle::{BundleTask, Report, Start};
use crate::endpoint::{Connection, Endpoint, Incoming};
use crate::storage::{Storage, StorageConfig, StorageError};
use crate::stream::read_message;

/// How a [`Node`] and its bundles behave.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeConfig {
    /// Every bundle's session settings. `upload.max_open_requests` should match the endpoint's
    /// [`EndpointConfig::max_open_requests`](crate::EndpointConfig::max_open_requests).
    pub session: Config,
    /// Every bundle's storage settings.
    pub storage: StorageConfig,
    /// The addresses this peer tells others to reach it at, in Hello and Candidates.
    pub addresses: Vec<Address>,
    /// How long a dial may take, per address.
    pub dial_timeout: Duration,
    /// How long an accepted connection may take to send Hello.
    pub hello_timeout: Duration,
    /// How often each connection reports QUIC's round-trip time to its session.
    pub rtt_interval: Duration,
    /// How many reports may wait for a bundle task.
    pub inbox: usize,
    /// How many control messages may wait to be written on one connection. A peer that lets
    /// more pile up by not reading is disconnected.
    pub control_queue: usize,
    /// How many decision events a subscriber may fall behind before it misses some.
    pub events: usize,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            session: Config::default(),
            storage: StorageConfig::default(),
            addresses: Vec::new(),
            dial_timeout: Duration::from_secs(10),
            hello_timeout: Duration::from_secs(10),
            rtt_interval: Duration::from_secs(1),
            inbox: 1024,
            control_queue: 256,
            events: 4096,
        }
    }
}

/// Why a bundle could not be started or stopped early.
#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    /// The settings are invalid, or the stored chunks don't fit the manifest.
    #[error(transparent)]
    Session(#[from] tempest_core::SessionError),
    /// Storage failed.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// The node is already running a session for this bundle.
    #[error("bundle {0} is already running")]
    AlreadyRunning(BundleId),
    /// The node has shut down.
    #[error("the node has shut down")]
    Stopped,
}

/// Where a download's entries are written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    /// Directly in this directory, which may already hold some of them (see
    /// [`Storage::create_download`]).
    Dir(PathBuf),
    /// In a new folder named after the bundle, created inside this directory once the manifest
    /// arrives. The download fails if something by that name is already there.
    NewFolderIn(PathBuf),
}

impl Destination {
    /// The directory to lay the bundle out in, creating it for [`Destination::NewFolderIn`].
    async fn resolve(self, manifest: &Manifest) -> Result<PathBuf, StorageError> {
        match self {
            Self::Dir(dir) => Ok(dir),
            Self::NewFolderIn(parent) => {
                // The name is a valid path segment, as every manifest's is.
                let dir = parent.join(manifest.name());
                let create = {
                    let dir = dir.clone();
                    move || {
                        std::fs::create_dir_all(&parent)?;
                        std::fs::create_dir(&dir)
                    }
                };
                match tokio::task::spawn_blocking(create).await {
                    Ok(Ok(())) => Ok(dir),
                    Ok(Err(source)) => Err(StorageError::Io { path: dir, source }),
                    Err(_) => panic!("creating the download folder panicked"),
                }
            }
        }
    }
}

impl From<PathBuf> for Destination {
    fn from(dir: PathBuf) -> Self {
        Self::Dir(dir)
    }
}

impl From<&Path> for Destination {
    fn from(dir: &Path) -> Self {
        Self::Dir(dir.to_owned())
    }
}

/// A bundle's progress, as of the bundle task's last step.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// The manifest, once known.
    pub manifest: Option<Arc<Manifest>>,
    /// The number of chunks stored.
    pub have: u32,
    /// The bundle's number of chunks, once the manifest is known.
    pub chunks: u32,
    /// Payload bytes of verified Data received, including duplicates.
    pub downloaded: u64,
    /// Payload bytes of Data sent.
    pub uploaded: u64,
    /// The number of connections, open or opening.
    pub connections: usize,
    /// Every chunk is stored and synced to disk.
    pub complete: bool,
}

/// How a bundle's session ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    /// Every chunk was stored.
    pub complete: bool,
    /// Payload bytes of verified Data received, including duplicates.
    pub downloaded: u64,
    /// Payload bytes of Data sent.
    pub uploaded: u64,
}

/// A running bundle. Dropping it leaves the bundle running until the node shuts down.
#[derive(Debug)]
pub struct BundleHandle {
    /// Decision events (D-006), from the start. Call `resubscribe` for another receiver.
    pub events: broadcast::Receiver<Event>,
    /// The bundle's progress. The sender is dropped once the bundle stops.
    pub status: watch::Receiver<Status>,
    bundle_id: BundleId,
    peers: PeerSink,
    shutdown: Arc<watch::Sender<bool>>,
    task: JoinHandle<Result<Summary, DriverError>>,
}

impl BundleHandle {
    /// The bundle's ID.
    pub fn bundle_id(&self) -> BundleId {
        self.bundle_id
    }

    /// Adds peers the session may dial, such as from a link or a tracker.
    pub async fn add_peers(&self, peers: Vec<PeerRecord>) {
        self.peers.add_peers(peers).await;
    }

    /// Somewhere for discovery to deliver peers to, apart from the handle.
    pub fn peer_sink(&self) -> PeerSink {
        self.peers.clone()
    }

    /// Asks the session to close its connections with `shutting_down` and stop.
    pub fn shutdown(&self) {
        self.shutdown.send_replace(true);
    }

    /// Waits for the bundle to stop: once shut down, once it is done lingering, or when storage
    /// fails. Storage is flushed and closed by then.
    pub async fn finished(self) -> Result<Summary, DriverError> {
        self.task.await.expect("the bundle task panicked")
    }
}

/// Where discovery delivers the peers it finds for one bundle.
#[derive(Debug, Clone)]
pub struct PeerSink(mpsc::Sender<Report>);

impl PeerSink {
    /// Adds peers the session may dial. Returns false once the bundle has stopped.
    pub async fn add_peers(&self, peers: Vec<PeerRecord>) -> bool {
        self.0.send(Report::AddPeers(peers)).await.is_ok()
    }
}

/// What the node task is asked to do.
enum Command {
    Register {
        bundle_id: BundleId,
        entry: Entry,
        reply: oneshot::Sender<Result<(), DriverError>>,
    },
    Unregister {
        bundle_id: BundleId,
    },
    Route {
        bundle_id: BundleId,
        reply: oneshot::Sender<Route>,
    },
    Shutdown {
        done: oneshot::Sender<()>,
    },
}

/// A bundle in the node's table.
struct Entry {
    inbox: mpsc::Sender<Report>,
    shutdown: Arc<watch::Sender<bool>>,
}

/// Where an incoming connection goes.
enum Route {
    Bundle(mpsc::Sender<Report>),
    Unknown,
    ShuttingDown,
}

/// A peer on one endpoint, running a session for each bundle it shares or downloads.
#[derive(Debug, Clone)]
pub struct Node {
    endpoint: Endpoint,
    config: Arc<NodeConfig>,
    commands: mpsc::Sender<Command>,
}

impl Node {
    /// Starts accepting connections on `endpoint`. Must be called within a tokio runtime.
    pub fn start(endpoint: Endpoint, config: NodeConfig) -> Self {
        let config = Arc::new(config);
        let (commands, receiver) = mpsc::channel(64);
        let task = NodeTask {
            endpoint: endpoint.clone(),
            config: config.clone(),
            commands: commands.downgrade(),
            bundles: HashMap::new(),
            shutdown: None,
        };
        tokio::spawn(task.run(receiver));
        Self {
            endpoint,
            config,
            commands,
        }
    }

    /// The endpoint the node runs on.
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Shares a bundle from `storage`, with the chunks it holds.
    pub async fn share(&self, storage: Storage) -> Result<BundleHandle, DriverError> {
        let have = storage.have().await?;
        self.run(
            storage.manifest().bundle_id(),
            Start::Stored { storage, have },
            Vec::new(),
        )
        .await
    }

    /// Downloads the bundle `bundle_id` to `destination` from `peers` and whomever they lead
    /// to. The manifest comes first, from one of them; storage is laid out at the destination
    /// once it arrives.
    pub async fn download(
        &self,
        bundle_id: BundleId,
        destination: impl Into<Destination>,
        peers: Vec<PeerRecord>,
    ) -> Result<BundleHandle, DriverError> {
        let destination = destination.into();
        self.run(bundle_id, Start::Fetch { destination }, peers)
            .await
    }

    async fn run(
        &self,
        bundle_id: BundleId,
        start: Start,
        peers: Vec<PeerRecord>,
    ) -> Result<BundleHandle, DriverError> {
        let (inbox, receiver) = mpsc::channel(self.config.inbox.max(1));
        let shutdown = Arc::new(watch::channel(false).0);
        let (reply, registered) = oneshot::channel();
        let entry = Entry {
            inbox: inbox.clone(),
            shutdown: shutdown.clone(),
        };
        let command = Command::Register {
            bundle_id,
            entry,
            reply,
        };
        self.commands
            .send(command)
            .await
            .map_err(|_| DriverError::Stopped)?;
        registered.await.map_err(|_| DriverError::Stopped)??;

        let (events, events_receiver) = broadcast::channel(self.config.events.max(1));
        let (status, status_receiver) = watch::channel(Status::default());
        let task = BundleTask::new(
            bundle_id,
            start,
            self.endpoint.clone(),
            self.config.clone(),
            inbox.clone(),
            events,
            status,
        );
        let task = match task {
            // Discovery reads the first status to learn whether the bundle starts complete.
            Ok(task) => task.with_status_published(),
            Err(error) => {
                let _ = self.commands.send(Command::Unregister { bundle_id }).await;
                return Err(error);
            }
        };
        let commands = self.commands.clone();
        let watcher = shutdown.subscribe();
        let task = tokio::spawn(async move {
            let result = task.run(receiver, watcher, peers).await;
            let _ = commands.send(Command::Unregister { bundle_id }).await;
            result
        });
        Ok(BundleHandle {
            events: events_receiver,
            status: status_receiver,
            bundle_id,
            peers: PeerSink(inbox),
            shutdown,
            task,
        })
    }

    /// Shuts every bundle down, stops accepting, and waits until the Goodbyes have reached the
    /// other peers.
    pub async fn shutdown(&self) {
        let (done, finished) = oneshot::channel();
        if self.commands.send(Command::Shutdown { done }).await.is_ok() {
            let _ = finished.await;
        }
    }
}

/// The node task's state.
struct NodeTask {
    endpoint: Endpoint,
    config: Arc<NodeConfig>,
    /// For routing tasks to reach the node. Weak, so the task ends once every [`Node`] is gone.
    commands: mpsc::WeakSender<Command>,
    bundles: HashMap<BundleId, Entry>,
    /// Who to tell once shutdown is done, once asked to shut down.
    shutdown: Option<Vec<oneshot::Sender<()>>>,
}

impl NodeTask {
    async fn run(mut self, mut commands: mpsc::Receiver<Command>) {
        loop {
            if self.shutdown.is_some() && self.bundles.is_empty() {
                break;
            }
            tokio::select! {
                command = commands.recv() => match command {
                    Some(command) => self.handle(command),
                    // Every handle is gone: nobody can add or stop bundles any more.
                    None if self.bundles.is_empty() => break,
                    None => self.begin_shutdown(None),
                },
                incoming = self.endpoint.accept(), if self.shutdown.is_none() => {
                    let Some(incoming) = incoming else { break };
                    let Some(commands) = self.commands.upgrade() else { break };
                    tokio::spawn(route(incoming, commands, self.config.hello_timeout));
                }
            }
        }
        self.endpoint.close(GoodbyeReason::ShuttingDown, "");
        self.endpoint.wait_idle().await;
        for done in self.shutdown.into_iter().flatten() {
            let _ = done.send(());
        }
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Register {
                bundle_id,
                entry,
                reply,
            } => {
                let result = match self.bundles.entry(bundle_id) {
                    _ if self.shutdown.is_some() => Err(DriverError::Stopped),
                    hash_map::Entry::Occupied(_) => Err(DriverError::AlreadyRunning(bundle_id)),
                    hash_map::Entry::Vacant(vacant) => {
                        vacant.insert(entry);
                        Ok(())
                    }
                };
                let _ = reply.send(result);
            }
            Command::Unregister { bundle_id } => {
                self.bundles.remove(&bundle_id);
            }
            Command::Route { bundle_id, reply } => {
                let route = match self.bundles.get(&bundle_id) {
                    _ if self.shutdown.is_some() => Route::ShuttingDown,
                    Some(entry) => Route::Bundle(entry.inbox.clone()),
                    None => Route::Unknown,
                };
                let _ = reply.send(route);
            }
            Command::Shutdown { done } => self.begin_shutdown(Some(done)),
        }
    }

    fn begin_shutdown(&mut self, done: Option<oneshot::Sender<()>>) {
        for entry in self.bundles.values() {
            entry.shutdown.send_replace(true);
        }
        self.shutdown.get_or_insert_with(Vec::new).extend(done);
    }
}

/// Completes an incoming connection's handshake, reads its Hello, and hands it to the bundle
/// Hello names.
async fn route(incoming: Incoming, commands: mpsc::Sender<Command>, hello_timeout: Duration) {
    let Ok(connection) = incoming.accept().await else {
        return;
    };
    let opening = tokio::time::timeout(hello_timeout, read_hello(&connection)).await;
    let (send, recv, hello) = match opening {
        Ok(Ok(opening)) => opening,
        Ok(Err(Some(message))) => {
            connection.close(GoodbyeReason::ProtocolViolation, message);
            return;
        }
        // The connection is gone.
        Ok(Err(None)) => return,
        Err(_) => {
            connection.close(GoodbyeReason::ProtocolViolation, "no Hello in time");
            return;
        }
    };

    let (reply, route) = oneshot::channel();
    let bundle_id = hello.bundle_id;
    if commands
        .send(Command::Route { bundle_id, reply })
        .await
        .is_err()
    {
        connection.close(GoodbyeReason::ShuttingDown, "");
        return;
    }
    match route.await {
        Ok(Route::Bundle(inbox)) => {
            let accepted = Report::Accepted {
                connection: connection.clone(),
                send,
                recv,
                hello,
            };
            if inbox.send(accepted).await.is_err() {
                connection.close(GoodbyeReason::ShuttingDown, "");
            }
        }
        Ok(Route::Unknown) => connection.close(GoodbyeReason::UnknownBundle, ""),
        Ok(Route::ShuttingDown) | Err(_) => connection.close(GoodbyeReason::ShuttingDown, ""),
    }
}

/// Accepts the control stream and reads Hello from it. Fails with the reason for a
/// `protocol_violation`, or `None` if the connection failed.
async fn read_hello(
    connection: &Connection,
) -> Result<(SendStream, RecvStream, Hello), Option<&'static str>> {
    let (send, mut recv) = connection.quic().accept_bi().await.map_err(|_| None)?;
    match read_message(&mut recv).await {
        Ok(Some(Message::Hello(hello))) => Ok((send, recv, hello)),
        Ok(Some(_)) => Err(Some("the first control message was not Hello")),
        Ok(None) => Err(Some("the control stream ended before Hello")),
        Err(crate::ReadMessageError::Read(_)) => Err(None),
        Err(_) => Err(Some("the first control message did not decode")),
    }
}
