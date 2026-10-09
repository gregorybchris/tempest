//! The production driver: `tempest-core` sessions on tokio, over QUIC (D-068, D-085).
//!
//! A [`Node`] owns an [`Endpoint`] and runs one session per bundle. It is a set of actors that
//! talk over bounded channels and share no mutable state:
//!
//! - The node task accepts connections and keeps the table of bundles. Each incoming connection
//!   gets a routing task, which reads Hello from the control stream and hands the connection to
//!   the bundle it names, or closes it with `unknown_bundle`. Connections beyond the limits on
//!   those still opening are refused before the handshake.
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
mod opening;
mod request;

use std::collections::{HashMap, hash_map};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use quinn::{RecvStream, SendStream};
use tempest_core::{ChunkCell, Config, Event, PeerStatus};
use tempest_protocol::message::{GoodbyeReason, Hello, PeerRecord};
use tempest_protocol::{Address, BundleId, Manifest, Message, RangeList};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use self::bundle::{BundleTask, Outputs, Report, Start};
use self::opening::{Opening, Openings};
use crate::endpoint::{Connection, Endpoint, Incoming};
use crate::home::HomeRelay;
use crate::limit::{SpeedLimits, Throttle};
use crate::portmap::PortMapping;
use crate::storage::{Checkpoint, Storage, StorageConfig, StorageError};
use crate::stream::read_message;

/// How a [`Node`] and its bundles behave.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeConfig {
    /// Every bundle's session settings. `upload.max_open_requests` should match the endpoint's
    /// [`EndpointConfig::max_open_requests`](crate::EndpointConfig::max_open_requests).
    pub session: Config,
    /// Every bundle's storage settings.
    pub storage: StorageConfig,
    /// The direct addresses this peer tells others to reach it at, in Hello and Candidates. The
    /// port mapping's external address and the relay address come from the endpoint.
    pub addresses: Vec<Address>,
    /// How long a dial may take, per address.
    pub dial_timeout: Duration,
    /// How long a hole punch keeps dialing a peer's candidate addresses, to replace a relayed
    /// connection with a direct one. `None` turns hole punching off.
    pub punch_time: Option<Duration>,
    /// How long an accepted connection may take to send Hello.
    pub hello_timeout: Duration,
    /// How many incoming connections may be opening at once: completing the handshake or
    /// waiting for Hello. More are refused without a handshake.
    pub max_opening: usize,
    /// How many of them may come from one IP address, or one IPv6 /64 prefix.
    pub max_opening_per_source: usize,
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
            punch_time: Some(Duration::from_secs(5)),
            hello_timeout: Duration::from_secs(10),
            max_opening: 256,
            max_opening_per_source: 16,
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
    /// The bundle's task panicked: a bug.
    #[error("the bundle stopped unexpectedly: {0}")]
    Crashed(String),
}

/// Where a download's entries are written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    /// Directly in this directory, which may already hold some of them (see
    /// [`Storage::create_download`]).
    Dir(PathBuf),
    /// Inside this directory once the manifest arrives: a bundle of one file is written straight
    /// into it, any other in a new folder named after the bundle. The download fails if
    /// something by that name is already there.
    NewFolderIn(PathBuf),
}

impl Destination {
    /// The directory to lay the bundle out in, creating it for [`Destination::NewFolderIn`].
    async fn resolve(self, manifest: &Manifest) -> Result<PathBuf, StorageError> {
        match self {
            Self::Dir(dir) => Ok(dir),
            Self::NewFolderIn(parent) => {
                if let Some(path) = manifest.single_file() {
                    let file = parent.join(path);
                    let check = {
                        let parent = parent.clone();
                        move || {
                            std::fs::create_dir_all(&parent)
                                .map_err(|source| (parent.clone(), source))?;
                            // Created here, atomically, so a file that appears by the same name
                            // in the meantime is never taken for this download's and overwritten.
                            // Nothing is followed: an existing symlink fails too.
                            std::fs::OpenOptions::new()
                                .write(true)
                                .create_new(true)
                                .open(&file)
                                .map(drop)
                                .map_err(|source| (file, source))
                        }
                    };
                    return match tokio::task::spawn_blocking(check).await {
                        Ok(Ok(())) => Ok(parent),
                        Ok(Err((path, source))) => Err(StorageError::Io { path, source }),
                        Err(_) => panic!("preparing the download folder panicked"),
                    };
                }
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
    /// How many of them go through a relay.
    pub relayed: usize,
    /// The number of peers known, connected or not.
    pub known_peers: usize,
    /// Every chunk is stored and synced to disk.
    pub complete: bool,
}

/// How a bundle's session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    /// Every chunk was stored.
    pub complete: bool,
    /// Payload bytes of verified Data received, including duplicates.
    pub downloaded: u64,
    /// Payload bytes of Data sent.
    pub uploaded: u64,
    /// What storage held when it closed, unless the manifest never arrived.
    pub checkpoint: Option<Checkpoint>,
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
    limits: SpeedLimits,
    home_relay: HomeRelay,
    port_mapping: PortMapping,
    shutdown: Arc<watch::Sender<bool>>,
    task: JoinHandle<Result<Summary, DriverError>>,
}

impl BundleHandle {
    /// The bundle's ID.
    pub fn bundle_id(&self) -> BundleId {
        self.bundle_id
    }

    /// The bundle's own speed limits, none at first. They apply on top of the node's.
    pub fn limits(&self) -> &SpeedLimits {
        &self.limits
    }

    /// Adds peers the session may dial, such as from a link or a tracker.
    pub async fn add_peers(&self, peers: Vec<PeerRecord>) {
        self.peers.add_peers(peers).await;
    }

    /// The bundle's storage, once laid out: from the start when shared, and once the manifest
    /// arrives when downloaded. `None` before then and once the bundle has stopped.
    pub async fn storage(&self) -> Option<Storage> {
        let (reply, storage) = oneshot::channel();
        self.peers.0.send(Report::Storage(reply)).await.ok()?;
        storage.await.ok().flatten()
    }

    /// The home relay of the node's endpoint, whose relay address trackers are told.
    pub fn home_relay(&self) -> &HomeRelay {
        &self.home_relay
    }

    /// The port mapping of the node's endpoint, whose external address trackers are told.
    pub fn port_mapping(&self) -> &PortMapping {
        &self.port_mapping
    }

    /// Somewhere for discovery to deliver peers to, apart from the handle.
    pub fn peer_sink(&self) -> PeerSink {
        self.peers.clone()
    }

    /// A way to look into the running bundle, apart from the handle.
    pub fn inspector(&self) -> Inspector {
        Inspector(self.peers.0.clone())
    }

    /// Asks the session to close its connections with `shutting_down` and stop.
    pub fn shutdown(&self) {
        self.shutdown.send_replace(true);
    }

    /// Waits for the bundle to stop: once shut down, once it is done lingering, or when storage
    /// fails. Storage is flushed and closed by then.
    pub async fn finished(self) -> Result<Summary, DriverError> {
        // The task only fails to finish if the runtime is shutting down.
        self.task.await.unwrap_or(Err(DriverError::Stopped))
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

/// One connection of a running bundle, as a UI shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct PeerDetail {
    /// The other peer's address, or a virtual address for a connection through this peer's
    /// home relay.
    pub address: SocketAddr,
    /// What the session knows of the connection.
    pub status: PeerStatus,
}

/// Looks into a running bundle for a UI. Every answer is `None` once the bundle has stopped.
#[derive(Debug, Clone)]
pub struct Inspector(mpsc::Sender<Report>);

impl Inspector {
    async fn ask<T>(&self, report: impl FnOnce(oneshot::Sender<T>) -> Report) -> Option<T> {
        let (reply, answer) = oneshot::channel();
        self.0.send(report(reply)).await.ok()?;
        answer.await.ok()
    }

    /// Every connection, open or opening.
    pub async fn peers(&self) -> Option<Vec<PeerDetail>> {
        self.ask(Report::Peers).await
    }

    /// The bundle's chunks in at most `cells` cells (see [`tempest_core::inspect::chunk_map`]),
    /// once the manifest is known.
    pub async fn chunk_map(&self, cells: u32) -> Option<Vec<ChunkCell>> {
        self.ask(|reply| Report::ChunkMap(cells, reply))
            .await
            .flatten()
    }

    /// The chunks stored, once the manifest is known.
    pub async fn have(&self) -> Option<RangeList> {
        self.ask(Report::Have).await.flatten()
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
    limits: SpeedLimits,
    commands: mpsc::Sender<Command>,
}

impl Node {
    /// Starts accepting connections on `endpoint`. Must be called within a tokio runtime.
    pub fn start(endpoint: Endpoint, config: NodeConfig) -> Self {
        let config = Arc::new(config);
        let (commands, receiver) = mpsc::channel(64);
        let task = NodeTask {
            endpoint: endpoint.clone(),
            openings: Openings::new(config.max_opening, config.max_opening_per_source),
            config: config.clone(),
            commands: commands.downgrade(),
            bundles: HashMap::new(),
            shutdown: None,
        };
        tokio::spawn(task.run(receiver));
        Self {
            endpoint,
            config,
            limits: SpeedLimits::default(),
            commands,
        }
    }

    /// The endpoint the node runs on.
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// The speed limits over every bundle together, none at first. Clones of the node share
    /// them.
    pub fn limits(&self) -> &SpeedLimits {
        &self.limits
    }

    /// Runs a bundle from `storage`, with the chunks it holds: shares it, and downloads the rest
    /// if it is incomplete.
    pub async fn share(&self, storage: Storage) -> Result<BundleHandle, DriverError> {
        self.share_with(storage, None).await
    }

    /// [`share`](Self::share), with session settings of its own instead of the node's.
    pub async fn share_with(
        &self,
        storage: Storage,
        session: Option<Config>,
    ) -> Result<BundleHandle, DriverError> {
        let have = storage.have().await?;
        let bundle_id = storage.manifest().bundle_id();
        let start = Start::Stored { storage, have };
        self.run(bundle_id, start, Vec::new(), session).await
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
        self.download_with(bundle_id, destination, peers, None)
            .await
    }

    /// [`download`](Self::download), with session settings of its own instead of the node's.
    pub async fn download_with(
        &self,
        bundle_id: BundleId,
        destination: impl Into<Destination>,
        peers: Vec<PeerRecord>,
        session: Option<Config>,
    ) -> Result<BundleHandle, DriverError> {
        let destination = destination.into();
        self.run(bundle_id, Start::Fetch { destination }, peers, session)
            .await
    }

    async fn run(
        &self,
        bundle_id: BundleId,
        start: Start,
        peers: Vec<PeerRecord>,
        session: Option<Config>,
    ) -> Result<BundleHandle, DriverError> {
        let config = match session {
            Some(session) => Arc::new(NodeConfig {
                session,
                ..NodeConfig::clone(&self.config)
            }),
            None => self.config.clone(),
        };
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

        let (events, events_receiver) = broadcast::channel(config.events.max(1));
        let (status, status_receiver) = watch::channel(Status::default());
        let limits = SpeedLimits::default();
        let throttle = Throttle {
            bundle: limits.clone(),
            node: self.limits.clone(),
        };
        let task = BundleTask::new(
            bundle_id,
            start,
            self.endpoint.clone(),
            config,
            throttle,
            inbox.clone(),
            Outputs { events, status },
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
            // Run apart, so that if it panics, the bundle is still unregistered and its
            // handle hears of it.
            let result = tokio::spawn(task.run(receiver, watcher, peers)).await;
            let _ = commands.send(Command::Unregister { bundle_id }).await;
            result.unwrap_or_else(|error| Err(DriverError::Crashed(error.to_string())))
        });
        Ok(BundleHandle {
            events: events_receiver,
            status: status_receiver,
            bundle_id,
            peers: PeerSink(inbox),
            limits,
            home_relay: self.endpoint.home_relay().clone(),
            port_mapping: self.endpoint.port_mapping().clone(),
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
    /// The incoming connections not yet handed to a bundle.
    openings: Openings,
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
                    let source = opening::source(&self.endpoint, incoming.remote_address());
                    match self.openings.try_open(source) {
                        Some(opening) => {
                            let timeout = self.config.hello_timeout;
                            tokio::spawn(route(incoming, opening, commands, timeout));
                        }
                        None => {
                            tracing::debug!(
                                "refused an incoming connection from {}: too many are opening",
                                incoming.remote_address()
                            );
                            incoming.refuse();
                        }
                    }
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
                        tracing::debug!("{bundle_id} registered with the node");
                        Ok(())
                    }
                };
                let _ = reply.send(result);
            }
            Command::Unregister { bundle_id } => {
                self.bundles.remove(&bundle_id);
                tracing::debug!("{bundle_id} unregistered from the node");
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
        tracing::info!("node shutting down; {} bundles running", self.bundles.len());
        for entry in self.bundles.values() {
            entry.shutdown.send_replace(true);
        }
        self.shutdown.get_or_insert_with(Vec::new).extend(done);
    }
}

/// Completes an incoming connection's handshake, reads its Hello, and hands it to the bundle
/// Hello names. Holds the connection's `place` among those opening until then.
async fn route(
    incoming: Incoming,
    place: Opening,
    commands: mpsc::Sender<Command>,
    hello_timeout: Duration,
) {
    let remote = incoming.remote_address();
    let connection = match incoming.accept().await {
        Ok(connection) => connection,
        Err(error) => {
            tracing::debug!("handshake with incoming {remote} failed: {error}");
            return;
        }
    };
    let opening = tokio::time::timeout(hello_timeout, read_hello(&connection)).await;
    let (send, recv, hello) = match opening {
        Ok(Ok(opening)) => opening,
        Ok(Err(Some(message))) => {
            tracing::debug!("incoming {remote} closed: {message}");
            connection.close(GoodbyeReason::ProtocolViolation, message);
            return;
        }
        // The connection is gone.
        Ok(Err(None)) => {
            tracing::debug!("incoming {remote} went away before its Hello");
            return;
        }
        Err(_) => {
            tracing::debug!("incoming {remote} sent no Hello within {hello_timeout:?}");
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
                tracing::debug!("incoming {remote} for {bundle_id}: the bundle has stopped");
                connection.close(GoodbyeReason::ShuttingDown, "");
            } else {
                tracing::debug!("accepted incoming {remote} for {bundle_id}");
            }
            drop(place);
        }
        Ok(Route::Unknown) => {
            tracing::debug!("incoming {remote} asked for {bundle_id}, which this node doesn't run");
            connection.close(GoodbyeReason::UnknownBundle, "");
        }
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
