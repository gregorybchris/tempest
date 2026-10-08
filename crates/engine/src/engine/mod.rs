//! The engine task and its handle.
//!
//! The engine task owns the node, the state store and every transfer's record, and is the only
//! place they change (D-085). [`Engine`] sends it commands. Work that takes time runs in tasks
//! of its own, which report back through a second inbox:
//!
//! - opening a transfer's storage, which may hash files (the transfer is "checking");
//! - a supervisor per running bundle, which owns its [`BundleHandle`] and its tracker
//!   announcers, forwards its decision events, takes checkpoints when asked, stops it, and
//!   reports how it ended;
//! - deleting a removed transfer's files;
//! - moving a transfer's files to another folder ("moving").
//!
//! Requests to trackers' catalogs, signing in with this peer's key and listing bundles, run on
//! the handle; only the trackers a transfer is listed on pass through the task.
//!
//! Every half second the task reads each running bundle's status, records what changed (the
//! manifest's arrival, completion, progress, rates, time spent sharing), emits snapshots of
//! transfers that changed, asks downloads for a checkpoint of their verified chunks every half
//! minute, applies the speed limits in effect, and stops transfers that reached a sharing
//! limit. Transfers that complete or fail run the notification command.

mod notify;
mod relocate;
mod supervisor;
mod task;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tempest_catalog_api::account::Account;
use tempest_catalog_api::listing::{Listing, ListingEdit};
use tempest_core::Event;
use tempest_net::{
    AnnounceConfig, Checkpoint, DriverError, Endpoint, EndpointConfig, HomeRelay, Identity,
    IdentityError, Inspector, Lan, LanConfig, Node, NodeConfig, PortMapping, PortMappingStatus,
    RelayStatus, Storage, StorageError, Summary, TrackerClient, TrackerError,
};
use tempest_protocol::link::ParseLinkError;
use tempest_protocol::{Address, BundleId, ChunkLayout, Link, Manifest, PeerId, RangeList};
use tokio::sync::{mpsc, oneshot};

use self::task::EngineTask;
use crate::TransferInfo;
use crate::config::{ConfigError, EngineConfig};
use crate::events::Subscription;
use crate::inspect::{self, ChunkMapInfo, FileInfo, PeerInfo};
use crate::manifest::{BuildError, BundleInfo, base_dir, build_manifest, unix_millis};
use crate::queue::QueueMove;
use crate::store::{Store, StoreError};

/// How often running transfers' statuses are read.
const TICK: Duration = Duration::from_millis(500);
/// How often a running download's verified chunks are recorded.
const CHECKPOINT_INTERVAL: Duration = Duration::from_secs(30);
/// How many events are kept for clients that resume.
const EVENT_BUFFER: usize = 4096;
/// How long a tracker request may take.
const TRACKER_TIMEOUT: Duration = Duration::from_secs(30);
/// How long stopping waits for trackers to hear `stopped`.
const ANNOUNCE_STOP_TIMEOUT: Duration = Duration::from_secs(2);
/// How long shutting down waits for Goodbyes to reach other peers.
const GOODBYE_TIMEOUT: Duration = Duration::from_secs(1);

/// Why the engine could not do something.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// The config is invalid.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// The state database failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The identity could not be loaded or created.
    #[error("identity: {0}")]
    Identity(#[from] IdentityError),
    /// A file system operation failed.
    #[error("{}: {source}", path.display())]
    Io {
        /// The file or directory.
        path: PathBuf,
        /// What went wrong.
        source: std::io::Error,
    },
    /// The peer port could not be bound.
    #[error("cannot bind UDP port {port}: {source}")]
    Bind {
        /// The port.
        port: u16,
        /// What went wrong.
        source: std::io::Error,
    },
    /// The text is not a `tempest:` link.
    #[error("not a valid tempest: link: {0}")]
    InvalidLink(#[from] ParseLinkError),
    /// The file or folder can't be shared.
    #[error(transparent)]
    Build(#[from] BuildError),
    /// Storage for the share could not be opened.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// The bundle is already a transfer.
    #[error("bundle {0} is already added")]
    AlreadyAdded(BundleId),
    /// No transfer has this ID.
    #[error("no transfer {0}")]
    NotFound(BundleId),
    /// The transfer is being removed.
    #[error("transfer {0} is still being removed")]
    Removing(BundleId),
    /// No category has this name.
    #[error("no category {0:?}")]
    UnknownCategory(String),
    /// A name, tag or category is empty or only whitespace.
    #[error("{0} must not be empty")]
    Empty(&'static str),
    /// A path must be absolute and valid UTF-8.
    #[error("{} must be an absolute path in UTF-8", .0.display())]
    InvalidPath(PathBuf),
    /// Another engine is using the data directory.
    #[error("another Tempest engine is using {}", .0.display())]
    Busy(PathBuf),
    /// The setting can't be changed while the engine runs.
    #[error("{0} can only be set in the config file")]
    Fixed(String),
    /// The transfer's files are being moved.
    #[error("transfer {0} is being moved")]
    Moving(BundleId),
    /// The transfer has no data on disk to check yet.
    #[error("transfer {0} has no data yet: its manifest hasn't arrived")]
    NoData(BundleId),
    /// A tracker URL is not an `http://` or `https://` URL.
    #[error("{0:?} is not an http:// or https:// URL")]
    InvalidTracker(String),
    /// A request to a tracker failed, such as signing in to its catalog.
    #[error("tracker {tracker}: {source}")]
    Tracker {
        /// The tracker's URL.
        tracker: String,
        /// What went wrong.
        source: TrackerError,
    },
    /// The engine has shut down.
    #[error("the engine has shut down")]
    Stopped,
}

/// The engine's settings: the config file's, with the changes made through the engine applied.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SettingsView {
    /// Every setting, as in effect.
    pub settings: EngineConfig,
    /// The settings changed through the engine rather than the config file, by dotted key.
    /// They are saved in the state database and apply over the file.
    pub changed: BTreeMap<String, serde_json::Value>,
    /// The groups of settings changed since the engine started that apply only once it
    /// restarts.
    pub restart_required: Vec<String>,
    /// Whether the alternative speed limits apply now, switched on or by their schedule.
    pub alternative_limits_active: bool,
}

/// How to add a transfer from a link.
#[derive(Debug, Clone, Default)]
pub struct AddOptions {
    /// Where to make the bundle's folder. Defaults to the category's save folder, then the
    /// config's.
    pub save_folder: Option<PathBuf>,
    /// The transfer's category.
    pub category: Option<String>,
    /// The transfer's tags.
    pub tags: BTreeSet<String>,
    /// Add it stopped rather than started.
    pub stopped: bool,
}

/// How to share a file or folder.
#[derive(Debug, Clone, Default)]
pub struct ShareOptions {
    /// The bundle's name. Defaults to the file or folder's name.
    pub name: Option<String>,
    /// A description of the bundle.
    pub comment: String,
    /// The transfer's category. Its save folder is not used: a share stays where it is.
    pub category: Option<String>,
    /// The transfer's tags.
    pub tags: BTreeSet<String>,
}

/// A handle to the engine. Clones share it. Once every handle is dropped the engine shuts down
/// as [`shutdown`](Self::shutdown) does, without anyone waiting.
#[derive(Debug, Clone)]
pub struct Engine {
    commands: mpsc::Sender<Command>,
    identity: Identity,
    tracker: TrackerClient,
    peer_id: PeerId,
    addresses: Vec<Address>,
    home_relay: HomeRelay,
    port_mapping: PortMapping,
}

type Reply<T> = oneshot::Sender<Result<T, EngineError>>;

/// What the engine is asked to do.
enum Command {
    List(oneshot::Sender<Vec<TransferInfo>>),
    Inside(BundleId, Reply<Inside>),
    AddLink(Link, AddOptions, Reply<TransferInfo>),
    AddShare(Box<NewShare>, Reply<TransferInfo>),
    Start(BundleId, bool, Reply<TransferInfo>),
    Stop(BundleId, Reply<TransferInfo>),
    Recheck(BundleId, Reply<TransferInfo>),
    MoveStorage(BundleId, PathBuf, Reply<TransferInfo>),
    Remove(BundleId, bool, Reply<()>),
    Rename(BundleId, Option<String>, Reply<TransferInfo>),
    SetTags(BundleId, BTreeSet<String>, Reply<TransferInfo>),
    SetCategory(BundleId, Option<String>, Reply<TransferInfo>),
    SetSettings(
        BundleId,
        BTreeMap<String, serde_json::Value>,
        Reply<TransferInfo>,
    ),
    Move(BundleId, QueueMove, Reply<Vec<TransferInfo>>),
    Publish(BundleId, String, Reply<TransferInfo>),
    Unpublish(BundleId, String, Reply<TransferInfo>),
    Categories(oneshot::Sender<BTreeMap<String, PathBuf>>),
    SetCategoryFolder(String, PathBuf, Reply<()>),
    RemoveCategory(String, Reply<()>),
    Settings(oneshot::Sender<SettingsView>),
    ChangeSettings(BTreeMap<String, serde_json::Value>, Reply<SettingsView>),
    ResetSetting(String, Reply<SettingsView>),
    Subscribe(Option<u64>, oneshot::Sender<Subscription>),
    Shutdown(oneshot::Sender<()>),
}

/// What it takes to look inside a transfer. The engine task hands it out, so that waiting on
/// the bundle happens outside the task.
struct Inside {
    manifest: Option<Arc<Manifest>>,
    /// The running bundle.
    running: Option<Inspector>,
    /// The chunks stored, while it isn't running and its manifest is known.
    stored: Option<RangeList>,
}

/// A share whose manifest has been built.
struct NewShare {
    manifest: Manifest,
    base: PathBuf,
    options: ShareOptions,
}

impl Engine {
    /// Starts the engine: opens its state, binds its peer port, and resumes the transfers that
    /// were running. Must be called within a tokio runtime.
    pub async fn open(config: EngineConfig) -> Result<Self, EngineError> {
        Self::launch(config, false).await
    }

    /// Opens the engine's state without running any transfer, to read and change it, as the
    /// CLI does when no daemon is running. It binds a loopback peer port the system picks, and
    /// leaves LAN discovery off. Must be called within a tokio runtime.
    pub async fn open_held(config: EngineConfig) -> Result<Self, EngineError> {
        Self::launch(config, true).await
    }

    async fn launch(base: EngineConfig, hold: bool) -> Result<Self, EngineError> {
        base.validate()?;
        let data_dir = base.data_dir()?;
        std::fs::create_dir_all(&data_dir).map_err(|source| EngineError::Io {
            path: data_dir.clone(),
            source,
        })?;
        let lock = lock(&data_dir)?;
        let mut store = Store::open(&data_dir.join("state.db"))?;
        let (config, changes) = apply_saved_changes(&base, &mut store)?;
        let identity = Identity::load_or_create(&data_dir.join("identity.pem"))?;
        let signer = identity.clone();
        let session = config.session()?;

        let endpoint_config = EndpointConfig {
            max_open_requests: session.upload.max_open_requests,
            ..match hold {
                false => endpoint_config(&config),
                true => EndpointConfig {
                    ipv4: Some(std::net::SocketAddrV4::new(
                        std::net::Ipv4Addr::LOCALHOST,
                        0,
                    )),
                    ipv6: None,
                    ..EndpointConfig::default()
                },
            }
        };
        let port = if hold { 0 } else { config.peers.port };
        let endpoint = Endpoint::bind(identity, &endpoint_config)
            .map_err(|source| EngineError::Bind { port, source })?;
        let addresses =
            tempest_net::direct_addresses(&endpoint.local_addrs()).map_err(|source| {
                EngineError::Io {
                    path: PathBuf::from("network interfaces"),
                    source,
                }
            })?;
        let home_relay = endpoint.home_relay().clone();
        let port_mapping = endpoint.port_mapping().clone();
        if !hold {
            endpoint.set_relays(config.relays.choice());
            endpoint.set_port_mapping(config.port_mapping.config());
        }
        let lan = match config.lan.enabled && !hold {
            true => {
                let lan_config = LanConfig {
                    port: config.lan.port,
                    ..LanConfig::default()
                };
                // Without a network to discover peers on, transfers still work.
                Lan::start(&endpoint, lan_config).ok()
            }
            false => None,
        };
        let announce = AnnounceConfig {
            addrs: tempest_net::public_addresses(&addresses),
            ..AnnounceConfig::new(
                endpoint.peer_id(),
                endpoint.local_addrs().first().map_or(0, |a| a.port()),
            )
        };
        let tracker = TrackerClient::new(TRACKER_TIMEOUT).map_err(|e| EngineError::Io {
            path: PathBuf::from("tracker client"),
            source: std::io::Error::other(e.to_string()),
        })?;
        let peer_id = endpoint.peer_id();
        let node = Node::start(
            endpoint,
            NodeConfig {
                session,
                addresses: addresses.clone(),
                ..NodeConfig::default()
            },
        );

        let (internal, internal_receiver) = mpsc::channel(256);
        let mut task = EngineTask::new(task::Parts {
            save_folder: config.save_folder()?,
            running: config.clone(),
            base,
            changes,
            config,
            hold,
            lock,
            store,
            node,
            lan,
            tracker: tracker.clone(),
            announce,
            identity: signer.clone(),
            peer_id,
            addresses: addresses.clone(),
            internal,
        });
        task.load()?;
        let (commands, receiver) = mpsc::channel(64);
        tokio::spawn(task.run(receiver, internal_receiver));
        Ok(Self {
            commands,
            identity: signer,
            tracker,
            peer_id,
            addresses,
            home_relay,
            port_mapping,
        })
    }

    /// This peer's ID.
    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    /// The direct addresses of this peer's network interfaces. Links also carry the port
    /// mapping's external address.
    pub fn addresses(&self) -> &[Address] {
        &self.addresses
    }

    /// What port mapping is doing, and the external address it gives this peer while mapped.
    pub fn port_mapping(&self) -> PortMappingStatus {
        self.port_mapping.status()
    }

    /// What the home relay is doing, and the relay address it lends while registered.
    pub fn relay(&self) -> RelayStatus {
        self.home_relay.status()
    }

    async fn ask<T>(&self, command: impl FnOnce(oneshot::Sender<T>) -> Command) -> Option<T> {
        let (reply, response) = oneshot::channel();
        self.commands.send(command(reply)).await.ok()?;
        response.await.ok()
    }

    async fn call<T>(&self, command: impl FnOnce(Reply<T>) -> Command) -> Result<T, EngineError> {
        self.ask(command).await.unwrap_or(Err(EngineError::Stopped))
    }

    /// Lists a transfer on the catalog of the tracker at `tracker` (D-129): signs in with this
    /// peer's key, using `invite` for a first sign-in to a catalog that needs one, sets the
    /// listing's fields from `edit`, and from then on announces the transfer there with
    /// `listing`. The tracker lists it once it has fetched the manifest from the swarm.
    pub async fn publish(
        &self,
        id: BundleId,
        tracker: &str,
        edit: &ListingEdit,
        invite: Option<&str>,
    ) -> Result<Listing, EngineError> {
        if !crate::config::is_http_url(tracker) {
            return Err(EngineError::InvalidTracker(tracker.to_owned()));
        }
        self.get(id).await.ok_or(EngineError::NotFound(id))?;
        let failed = |source| EngineError::Tracker {
            tracker: tracker.to_owned(),
            source,
        };
        let client = &self.tracker;
        let session = client
            .catalog_sign_in(tracker, &self.identity, invite)
            .await
            .map_err(failed)?;
        let listing = client
            .edit_listing(tracker, &session.token, &id, edit)
            .await;
        // The session was only for this; it would expire on its own.
        let _ = client.catalog_sign_out(tracker, &session.token).await;
        let listing = listing.map_err(failed)?;
        let tracker = tracker.to_owned();
        self.call(|reply| Command::Publish(id, tracker, reply))
            .await?;
        Ok(listing)
    }

    /// Withdraws a transfer's listing from the catalog of the tracker at `tracker`: signs in
    /// with this peer's key, withdraws the listing, signs out, and from then on announces the
    /// transfer there without `listing`, or not at all if only publishing put it there. A
    /// listing the catalog doesn't have counts as withdrawn.
    pub async fn unpublish(
        &self,
        id: BundleId,
        tracker: &str,
    ) -> Result<TransferInfo, EngineError> {
        if !crate::config::is_http_url(tracker) {
            return Err(EngineError::InvalidTracker(tracker.to_owned()));
        }
        self.get(id).await.ok_or(EngineError::NotFound(id))?;
        let failed = |source| EngineError::Tracker {
            tracker: tracker.to_owned(),
            source,
        };
        let client = &self.tracker;
        let session = client
            .catalog_sign_in(tracker, &self.identity, None)
            .await
            .map_err(failed)?;
        let withdrawn = client.withdraw_listing(tracker, &session.token, &id).await;
        let _ = client.catalog_sign_out(tracker, &session.token).await;
        match withdrawn {
            Ok(()) | Err(TrackerError::Refused { status: 404, .. }) => {}
            Err(error) => return Err(failed(error)),
        }
        let tracker = tracker.to_owned();
        self.call(|reply| Command::Unpublish(id, tracker, reply))
            .await
    }

    /// Signs this peer's key in to the catalog of the tracker at `tracker` for whoever asked
    /// for the sign-in `code`, such as a browser showing the catalog, using `invite` for a first
    /// sign-in to a catalog that needs one.
    pub async fn catalog_sign_in(
        &self,
        tracker: &str,
        code: &str,
        invite: Option<&str>,
    ) -> Result<Account, EngineError> {
        if !crate::config::is_http_url(tracker) {
            return Err(EngineError::InvalidTracker(tracker.to_owned()));
        }
        let session = self
            .tracker
            .catalog_sign_code(tracker, &self.identity, code, invite)
            .await
            .map_err(|source| EngineError::Tracker {
                tracker: tracker.to_owned(),
                source,
            })?;
        Ok(session.account)
    }

    /// Every transfer, in queue order.
    pub async fn list(&self) -> Vec<TransferInfo> {
        self.ask(Command::List).await.unwrap_or_default()
    }

    /// One transfer.
    pub async fn get(&self, id: BundleId) -> Option<TransferInfo> {
        self.list().await.into_iter().find(|t| t.id == id)
    }

    /// A transfer's files and how much of each is stored; none before the manifest is known.
    pub async fn files(&self, id: BundleId) -> Result<Vec<FileInfo>, EngineError> {
        let inside = self.call(|reply| Command::Inside(id, reply)).await?;
        let Some(manifest) = inside.manifest else {
            return Ok(Vec::new());
        };
        let running = match &inside.running {
            Some(inspector) => inspector.have().await,
            None => None,
        };
        let have = running.or(inside.stored).unwrap_or_default();
        Ok(inspect::files(&manifest, &have))
    }

    /// A running transfer's connections, open or opening; none while it isn't running.
    pub async fn peers(&self, id: BundleId) -> Result<Vec<PeerInfo>, EngineError> {
        let inside = self.call(|reply| Command::Inside(id, reply)).await?;
        let Some(inspector) = inside.running else {
            return Ok(Vec::new());
        };
        let peers = inspector.peers().await.unwrap_or_default();
        Ok(peers.into_iter().map(inspect::peer).collect())
    }

    /// A transfer's chunks in at most `cells` cells, and no more than
    /// [`MAX_CELLS`](inspect::MAX_CELLS); none before the manifest is known.
    pub async fn chunk_map(&self, id: BundleId, cells: u32) -> Result<ChunkMapInfo, EngineError> {
        let inside = self.call(|reply| Command::Inside(id, reply)).await?;
        let cells = cells.min(inspect::MAX_CELLS);
        let chunks = (inside.manifest.as_ref())
            .map_or(0, |manifest| ChunkLayout::new(manifest).chunk_count());
        if let Some(inspector) = &inside.running {
            let map = inspector.chunk_map(cells).await.unwrap_or_default();
            // The session may know the manifest a moment before the engine task does.
            let mapped = map.iter().map(|cell| cell.chunks()).sum();
            return Ok(ChunkMapInfo {
                chunks: chunks.max(mapped),
                running: true,
                cells: map.into_iter().map(crate::inspect::cell).collect(),
            });
        }
        let stored = inside.stored.unwrap_or_default();
        Ok(inspect::stored_chunk_map(chunks, &stored, cells))
    }

    /// Adds a download from a `tempest:` link. It starts once the queue lets it, unless added
    /// stopped. Its peers come from the link, its trackers and the config's, the local network
    /// and peer exchange, all at once.
    pub async fn add_link(
        &self,
        link: &str,
        options: AddOptions,
    ) -> Result<TransferInfo, EngineError> {
        let link: Link = link.parse()?;
        self.call(|reply| Command::AddLink(link, options, reply))
            .await
    }

    /// Makes a bundle from the file or folder at `path` and shares it from there. The returned
    /// transfer's `link` is the bundle's link.
    pub async fn add_path(
        &self,
        path: &Path,
        options: ShareOptions,
    ) -> Result<TransferInfo, EngineError> {
        let info = BundleInfo {
            name: options.name.clone(),
            comment: options.comment.clone(),
            created: unix_millis(SystemTime::now()),
        };
        let path = path.to_owned();
        let built = tokio::task::spawn_blocking(move || {
            let manifest = build_manifest(&path, info)?;
            Ok::<_, BuildError>((manifest, base_dir(&path)?))
        })
        .await
        .expect("building a manifest panicked");
        let (manifest, base) = built?;
        let share = Box::new(NewShare {
            manifest,
            base,
            options,
        });
        self.call(|reply| Command::AddShare(share, reply)).await
    }

    /// Starts a transfer, through the queue. Clears an error, and makes a completed transfer
    /// share again.
    pub async fn start(&self, id: BundleId) -> Result<TransferInfo, EngineError> {
        self.call(|reply| Command::Start(id, false, reply)).await
    }

    /// Starts a transfer at once, bypassing the queue.
    pub async fn force_start(&self, id: BundleId) -> Result<TransferInfo, EngineError> {
        self.call(|reply| Command::Start(id, true, reply)).await
    }

    /// Stops a transfer.
    pub async fn stop(&self, id: BundleId) -> Result<TransferInfo, EngineError> {
        self.call(|reply| Command::Stop(id, reply)).await
    }

    /// Checks a transfer's data on disk again from scratch, as "checking", records the chunks
    /// found, and resumes it if it was running. Missing or damaged chunks are downloaded again.
    pub async fn recheck(&self, id: BundleId) -> Result<TransferInfo, EngineError> {
        self.call(|reply| Command::Recheck(id, reply)).await
    }

    /// Moves a transfer's files into `folder`, as "moving", stopping it first if it runs and
    /// resuming it after. A download's bundle folder moves into `folder`; a share's entries
    /// move to `folder` as their new base. If the move fails, the files stay where they were
    /// and the transfer shows the error.
    pub async fn move_storage(
        &self,
        id: BundleId,
        folder: &Path,
    ) -> Result<TransferInfo, EngineError> {
        let folder = folder.to_owned();
        self.call(|reply| Command::MoveStorage(id, folder, reply))
            .await
    }

    /// Removes a transfer once it has stopped, deleting its files too if `delete_files` is set:
    /// the bundle's entries, and a download's bundle folder once empty.
    pub async fn remove(&self, id: BundleId, delete_files: bool) -> Result<(), EngineError> {
        self.call(|reply| Command::Remove(id, delete_files, reply))
            .await
    }

    /// Shows a transfer under `name`, or under the bundle's own name again with `None`.
    pub async fn rename(
        &self,
        id: BundleId,
        name: Option<String>,
    ) -> Result<TransferInfo, EngineError> {
        self.call(|reply| Command::Rename(id, name, reply)).await
    }

    /// Replaces a transfer's tags.
    pub async fn set_tags(
        &self,
        id: BundleId,
        tags: BTreeSet<String>,
    ) -> Result<TransferInfo, EngineError> {
        self.call(|reply| Command::SetTags(id, tags, reply)).await
    }

    /// Puts a transfer in a category, or none. Its data stays where it is: a category's save
    /// folder applies only to downloads added in it.
    pub async fn set_category(
        &self,
        id: BundleId,
        category: Option<String>,
    ) -> Result<TransferInfo, EngineError> {
        self.call(|reply| Command::SetCategory(id, category, reply))
            .await
    }

    /// Replaces a transfer's own settings, by dotted key, applied over the config's: session
    /// settings and `sharing.idle_timeout`, which take effect the next time it starts, and
    /// `limits.download`, `limits.upload` and the other `sharing.*` settings, which take effect
    /// at once (see
    /// [`EngineConfig::transfer`]).
    pub async fn set_transfer_settings(
        &self,
        id: BundleId,
        settings: BTreeMap<String, serde_json::Value>,
    ) -> Result<TransferInfo, EngineError> {
        self.call(|reply| Command::SetSettings(id, settings, reply))
            .await
    }

    /// Moves a transfer in the queue, and returns every transfer in the new order.
    pub async fn move_in_queue(
        &self,
        id: BundleId,
        to: QueueMove,
    ) -> Result<Vec<TransferInfo>, EngineError> {
        self.call(|reply| Command::Move(id, to, reply)).await
    }

    /// Every category and its save folder.
    pub async fn categories(&self) -> BTreeMap<String, PathBuf> {
        self.ask(Command::Categories).await.unwrap_or_default()
    }

    /// Adds a category, or changes its save folder.
    pub async fn set_category_folder(
        &self,
        name: &str,
        save_folder: &Path,
    ) -> Result<(), EngineError> {
        let (name, folder) = (name.to_owned(), save_folder.to_owned());
        self.call(|reply| Command::SetCategoryFolder(name, folder, reply))
            .await
    }

    /// Removes a category, taking it off every transfer.
    pub async fn remove_category(&self, name: &str) -> Result<(), EngineError> {
        let name = name.to_owned();
        self.call(|reply| Command::RemoveCategory(name, reply))
            .await
    }

    /// The settings in effect, and which were changed through the engine.
    pub async fn settings(&self) -> Result<SettingsView, EngineError> {
        self.ask(Command::Settings)
            .await
            .ok_or(EngineError::Stopped)
    }

    /// Changes settings by dotted key, as [`EngineConfig::set`] does, all or none. The changes
    /// are saved in the state database and apply over the config file, now where they can:
    /// session settings when a transfer next starts, and `peers`, `lan` and `api` when the
    /// engine restarts. `data_dir` can only be set in the file.
    pub async fn change_settings(
        &self,
        changes: BTreeMap<String, serde_json::Value>,
    ) -> Result<SettingsView, EngineError> {
        self.call(|reply| Command::ChangeSettings(changes, reply))
            .await
    }

    /// Forgets the change to a setting made through the engine, so the config file's value
    /// applies again.
    pub async fn reset_setting(&self, key: &str) -> Result<SettingsView, EngineError> {
        let key = key.to_owned();
        self.call(|reply| Command::ResetSetting(key, reply)).await
    }

    /// Subscribes to the engine's events: those after event `after` that are still buffered,
    /// then new ones. With `None`, only new ones.
    pub async fn subscribe(&self, after: Option<u64>) -> Result<Subscription, EngineError> {
        self.ask(|reply| Command::Subscribe(after, reply))
            .await
            .ok_or(EngineError::Stopped)
    }

    /// Stops every transfer, records where each one got to, and shuts the node down. Running
    /// transfers start again when the engine next opens.
    pub async fn shutdown(&self) {
        let _ = self.ask(Command::Shutdown).await;
    }
}

/// Takes the data directory's lock, held while the engine runs, so that only one engine uses
/// its state.
fn lock(data_dir: &Path) -> Result<std::fs::File, EngineError> {
    let path = data_dir.join("engine.lock");
    let io_error = |source| EngineError::Io {
        path: path.clone(),
        source,
    };
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(io_error)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(EngineError::Busy(data_dir.to_owned())),
        Err(std::fs::TryLockError::Error(source)) => Err(io_error(source)),
    }
}

/// The config with the changes saved in `store` applied. A saved change that no longer fits
/// the config file, which may have changed since, is dropped.
fn apply_saved_changes(
    base: &EngineConfig,
    store: &mut Store,
) -> Result<(EngineConfig, BTreeMap<String, serde_json::Value>), EngineError> {
    let mut config = base.clone();
    let mut changes = BTreeMap::new();
    for (key, value) in store.settings()? {
        let mut changed = config.clone();
        match key != "data_dir" && changed.set(&key, value.clone()).is_ok() {
            true => {
                config = changed;
                changes.insert(key, value);
            }
            false => store.remove_setting(&key)?,
        }
    }
    Ok((config, changes))
}

fn endpoint_config(config: &EngineConfig) -> EndpointConfig {
    let port = config.peers.port;
    if config.peers.bind.is_empty() {
        return EndpointConfig::any(port);
    }
    let mut endpoint = EndpointConfig {
        ipv4: None,
        ipv6: None,
        ..EndpointConfig::default()
    };
    for ip in &config.peers.bind {
        match ip {
            std::net::IpAddr::V4(v4) => {
                endpoint.ipv4 = Some(std::net::SocketAddrV4::new(*v4, port));
            }
            std::net::IpAddr::V6(v6) => {
                endpoint.ipv6 = Some(std::net::SocketAddrV6::new(*v6, port, 0, 0));
            }
        }
    }
    endpoint
}

/// What spawned tasks report to the engine task.
enum Internal {
    /// A transfer's storage was opened, or failed to open.
    Opened {
        id: BundleId,
        generation: u64,
        result: Result<Storage, StorageError>,
    },
    /// Decision events from a running bundle, and how many were dropped before them.
    Decisions {
        id: BundleId,
        events: Vec<Event>,
        missed: u64,
    },
    /// A running bundle's checkpoint.
    Checkpoint {
        id: BundleId,
        generation: u64,
        checkpoint: Checkpoint,
    },
    /// A bundle stopped.
    Finished {
        id: BundleId,
        generation: u64,
        result: Result<Summary, DriverError>,
    },
    /// A transfer's files were moved, or put back after failing to move.
    Moved {
        id: BundleId,
        generation: u64,
        result: std::io::Result<()>,
    },
}
