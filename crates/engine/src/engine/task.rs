//! The engine task: every transfer's record, the queue, and what running bundles report.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tempest_net::{
    AnnounceConfig, Announcer, BundleHandle, Checkpoint, Destination, DriverError, Lan, Node,
    Storage, StorageConfig, Summary, TrackerClient,
};
use tempest_protocol::message::PeerRecord;
use tempest_protocol::wire::{Decode, Encode};
use tempest_protocol::{Address, BundleId, ChunkLayout, Link, Manifest, PeerId};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{Instant, MissedTickBehavior};

use super::supervisor::Supervisor;
use super::{
    AddOptions, CHECKPOINT_INTERVAL, Command, EVENT_BUFFER, EngineError, GOODBYE_TIMEOUT, Internal,
    NewShare, Reply, TICK,
};
use crate::TransferInfo;
use crate::config::EngineConfig;
use crate::events::{EngineEvent, EventLog};
use crate::manifest::unix_millis;
use crate::queue::{Candidate, QueueMove, reorder, schedule};
use crate::status::{Phase, Status, StatusInput};
use crate::store::{Kind, Store, TransferRow};

/// What the engine task starts with.
pub(super) struct Parts {
    pub config: EngineConfig,
    pub save_folder: PathBuf,
    pub store: Store,
    pub node: Node,
    pub lan: Option<Lan>,
    pub tracker: TrackerClient,
    pub announce: AnnounceConfig,
    pub peer_id: PeerId,
    pub addresses: Vec<Address>,
    pub internal: mpsc::Sender<Internal>,
}

/// A transfer in the engine task.
struct Transfer {
    row: TransferRow,
    manifest: Option<Arc<Manifest>>,
    link: Link,
    live: Live,
    /// The last snapshot emitted.
    shown: Option<TransferInfo>,
}

/// Whether a transfer is running.
enum Live {
    Idle,
    Opening { generation: u64 },
    Running(Run),
    Stopping(Run),
}

/// A running bundle, as the engine task sees it.
struct Run {
    generation: u64,
    status: watch::Receiver<tempest_net::Status>,
    /// Dropping or setting it stops the bundle.
    stop: watch::Sender<bool>,
    checkpoint: mpsc::Sender<()>,
    /// The transfer's totals before this run.
    base_uploaded: u64,
    base_downloaded: u64,
    /// When it last made progress, and what it had then.
    last_progress: Instant,
    progress_mark: (u32, u64, bool),
    /// The last rate sample: when, and the run's totals then.
    sample: (Instant, u64, u64),
    download_rate: u64,
    upload_rate: u64,
    last_checkpoint: Instant,
}

/// A removed transfer whose bundle is still stopping.
struct Burial {
    generation: u64,
    files: Option<Files>,
    reply: Reply<()>,
}

/// The files to delete for a removed transfer.
struct Files {
    dir: PathBuf,
    manifest: Arc<Manifest>,
    /// The directory itself was made for the bundle, and goes too once empty.
    own_dir: bool,
}

/// The engine task's state.
pub(super) struct EngineTask {
    config: EngineConfig,
    save_folder: PathBuf,
    store: Store,
    node: Node,
    lan: Option<Lan>,
    tracker: TrackerClient,
    announce: AnnounceConfig,
    peer_id: PeerId,
    addresses: Vec<Address>,
    transfers: BTreeMap<BundleId, Transfer>,
    burials: HashMap<BundleId, Burial>,
    events: EventLog,
    internal: mpsc::Sender<Internal>,
    next_generation: u64,
    /// Who waits for shutdown, once it has begun.
    shutting_down: Option<Vec<oneshot::Sender<()>>>,
}

fn now_millis() -> i64 {
    unix_millis(SystemTime::now())
}

/// A path the store can keep: absolute and UTF-8.
fn checked_path(path: &Path) -> Result<PathBuf, EngineError> {
    match path.is_absolute() && path.to_str().is_some() {
        true => Ok(path.to_owned()),
        false => Err(EngineError::InvalidPath(path.to_owned())),
    }
}

fn checked_tags(tags: BTreeSet<String>) -> Result<BTreeSet<String>, EngineError> {
    tags.into_iter()
        .map(|tag| {
            let tag = tag.trim().to_owned();
            match tag.is_empty() {
                true => Err(EngineError::Empty("a tag")),
                false => Ok(tag),
            }
        })
        .collect()
}

impl EngineTask {
    pub fn new(parts: Parts) -> Self {
        let Parts {
            config,
            save_folder,
            store,
            node,
            lan,
            tracker,
            announce,
            peer_id,
            addresses,
            internal,
        } = parts;
        Self {
            config,
            save_folder,
            store,
            node,
            lan,
            tracker,
            announce,
            peer_id,
            addresses,
            transfers: BTreeMap::new(),
            burials: HashMap::new(),
            events: EventLog::new(EVENT_BUFFER),
            internal,
            next_generation: 0,
            shutting_down: None,
        }
    }

    /// Loads the saved transfers.
    pub fn load(&mut self) -> Result<(), EngineError> {
        for row in self.store.transfers()? {
            let manifest = row
                .manifest
                .as_deref()
                .and_then(|bytes| Manifest::from_bytes(bytes).ok())
                .filter(|manifest| manifest.bundle_id() == row.id)
                .map(Arc::new);
            let link = row.link.parse().unwrap_or_else(|_| Link::new(row.id));
            self.transfers.insert(
                row.id,
                Transfer {
                    row,
                    manifest,
                    link,
                    live: Live::Idle,
                    shown: None,
                },
            );
        }
        Ok(())
    }

    pub async fn run(
        mut self,
        mut commands: mpsc::Receiver<Command>,
        mut internal: mpsc::Receiver<Internal>,
    ) {
        self.reschedule().await;
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut commands_open = true;
        loop {
            if self.shutting_down.is_some() && self.is_idle() {
                break;
            }
            tokio::select! {
                command = commands.recv(), if commands_open => match command {
                    Some(command) => self.command(command).await,
                    None => {
                        commands_open = false;
                        self.begin_shutdown(None);
                    }
                },
                Some(message) = internal.recv() => self.internal(message).await,
                _ = tick.tick() => self.tick().await,
            }
        }
        let _ = tokio::time::timeout(GOODBYE_TIMEOUT, self.node.shutdown()).await;
        for done in self.shutting_down.take().into_iter().flatten() {
            let _ = done.send(());
        }
    }

    /// Nothing is running, opening, stopping or being buried.
    fn is_idle(&self) -> bool {
        self.burials.is_empty()
            && self
                .transfers
                .values()
                .all(|t| matches!(t.live, Live::Idle))
    }

    fn begin_shutdown(&mut self, done: Option<oneshot::Sender<()>>) {
        self.shutting_down.get_or_insert_with(Vec::new).extend(done);
        let ids: Vec<_> = self.transfers.keys().copied().collect();
        for id in ids {
            self.halt(id);
        }
    }

    async fn command(&mut self, command: Command) {
        if self.shutting_down.is_some() {
            // Dropping the reply tells the caller the engine has stopped.
            if let Command::Shutdown(done) = command {
                self.begin_shutdown(Some(done));
            }
            return;
        }
        match command {
            Command::List(reply) => {
                let _ = reply.send(self.list());
            }
            Command::AddLink(link, options, reply) => {
                let result = self.add_link(link, options).await;
                let _ = reply.send(result);
            }
            Command::AddShare(share, reply) => {
                let result = self.add_share(*share).await;
                let _ = reply.send(result);
            }
            Command::Start(id, forced, reply) => {
                let result = self
                    .change(id, |t| {
                        t.row.wanted = true;
                        t.row.forced = forced;
                        t.row.done = false;
                        t.row.error = None;
                        Ok(())
                    })
                    .await;
                let _ = reply.send(result);
            }
            Command::Stop(id, reply) => {
                let result = self
                    .change(id, |t| {
                        t.row.wanted = false;
                        t.row.forced = false;
                        Ok(())
                    })
                    .await;
                let _ = reply.send(result);
            }
            Command::Remove(id, delete_files, reply) => self.remove(id, delete_files, reply),
            Command::Rename(id, name, reply) => {
                let name = name
                    .map(|name| name.trim().to_owned())
                    .filter(|name| !name.is_empty());
                let result = self
                    .change(id, |t| {
                        t.row.name = name;
                        Ok(())
                    })
                    .await;
                let _ = reply.send(result);
            }
            Command::SetTags(id, tags, reply) => {
                let result = match checked_tags(tags) {
                    Ok(tags) => {
                        self.change(id, |t| {
                            t.row.tags = tags;
                            Ok(())
                        })
                        .await
                    }
                    Err(error) => Err(error),
                };
                let _ = reply.send(result);
            }
            Command::SetCategory(id, category, reply) => {
                let result = match self.checked_category(category) {
                    Ok(category) => {
                        self.change(id, |t| {
                            t.row.category = category;
                            Ok(())
                        })
                        .await
                    }
                    Err(error) => Err(error),
                };
                let _ = reply.send(result);
            }
            Command::SetSettings(id, settings, reply) => {
                let result = match self.config.transfer_session(&settings) {
                    Ok(_) => {
                        self.change(id, |t| {
                            t.row.settings = settings;
                            Ok(())
                        })
                        .await
                    }
                    Err(error) => Err(error.into()),
                };
                let _ = reply.send(result);
            }
            Command::Move(id, to, reply) => {
                let result = self.move_in_queue(id, to).await;
                let _ = reply.send(result);
            }
            Command::Categories(reply) => {
                let _ = reply.send(self.store.categories().unwrap_or_default());
            }
            Command::SetCategoryFolder(name, folder, reply) => {
                let name = name.trim().to_owned();
                let result = match (name.is_empty(), checked_path(&folder)) {
                    (true, _) => Err(EngineError::Empty("a category name")),
                    (false, Err(error)) => Err(error),
                    (false, Ok(folder)) => self
                        .store
                        .set_category(&name, &folder)
                        .map_err(EngineError::from),
                };
                let _ = reply.send(result);
            }
            Command::RemoveCategory(name, reply) => {
                let result = self.store.remove_category(&name).map_err(EngineError::from);
                for t in self.transfers.values_mut() {
                    if t.row.category.as_deref() == Some(&name) {
                        t.row.category = None;
                    }
                }
                self.emit_changes();
                let _ = reply.send(result);
            }
            Command::Subscribe(after, reply) => {
                let _ = reply.send(self.events.subscribe(after));
            }
            Command::Shutdown(done) => self.begin_shutdown(Some(done)),
        }
    }

    /// Changes a transfer's record with `edit`, saves it, emits its new state, and lets the
    /// queue react.
    async fn change(
        &mut self,
        id: BundleId,
        edit: impl FnOnce(&mut Transfer) -> Result<(), EngineError>,
    ) -> Result<TransferInfo, EngineError> {
        let t = self
            .transfers
            .get_mut(&id)
            .ok_or(EngineError::NotFound(id))?;
        edit(t)?;
        self.store.save_transfer(&t.row)?;
        self.reschedule().await;
        self.emit_changes();
        Ok(self.info(&id).expect("the transfer exists"))
    }

    fn checked_category(&self, category: Option<String>) -> Result<Option<String>, EngineError> {
        let Some(category) = category else {
            return Ok(None);
        };
        match self.store.categories()?.contains_key(&category) {
            true => Ok(Some(category)),
            false => Err(EngineError::UnknownCategory(category)),
        }
    }

    fn check_new(&self, id: BundleId) -> Result<(), EngineError> {
        if self.transfers.contains_key(&id) {
            return Err(EngineError::AlreadyAdded(id));
        }
        if self.burials.contains_key(&id) {
            return Err(EngineError::Removing(id));
        }
        Ok(())
    }

    fn next_position(&self) -> u32 {
        self.transfers
            .values()
            .map(|t| t.row.queue_position + 1)
            .max()
            .unwrap_or(0)
    }

    async fn add_link(
        &mut self,
        link: Link,
        options: AddOptions,
    ) -> Result<TransferInfo, EngineError> {
        let id = link.bundle;
        self.check_new(id)?;
        let category = self.checked_category(options.category)?;
        let save_folder = match (&options.save_folder, &category) {
            (Some(folder), _) => folder.clone(),
            (None, Some(category)) => self.store.categories()?[category].clone(),
            (None, None) => self.save_folder.clone(),
        };
        let row = TransferRow {
            id,
            kind: Kind::Download,
            link: link.to_string(),
            manifest: None,
            name: None,
            save_folder: checked_path(&save_folder)?,
            content_dir: None,
            wanted: !options.stopped,
            forced: false,
            done: false,
            error: None,
            added: now_millis(),
            completed: None,
            queue_position: self.next_position(),
            tags: checked_tags(options.tags)?,
            category,
            settings: BTreeMap::new(),
            uploaded: 0,
            downloaded: 0,
            have: 0,
            chunks: 0,
        };
        self.store.save_transfer(&row)?;
        if let Some(record) = link_record(&link) {
            self.store.save_peers(&id, &[record])?;
        }
        self.insert(row, None, link).await
    }

    async fn add_share(&mut self, share: NewShare) -> Result<TransferInfo, EngineError> {
        let NewShare {
            manifest,
            base,
            options,
        } = share;
        let id = manifest.bundle_id();
        self.check_new(id)?;
        let category = self.checked_category(options.category)?;
        let base = checked_path(&base)?;
        let added = now_millis();
        let chunks = u64::from(ChunkLayout::new(&manifest).chunk_count());
        let mut row = TransferRow {
            id,
            kind: Kind::Share,
            link: String::new(),
            manifest: Some(manifest.to_bytes()),
            name: None,
            save_folder: base.clone(),
            content_dir: Some(base),
            wanted: true,
            forced: false,
            done: false,
            error: None,
            added,
            completed: Some(added),
            queue_position: self.next_position(),
            tags: checked_tags(options.tags)?,
            category,
            settings: BTreeMap::new(),
            uploaded: 0,
            downloaded: 0,
            have: chunks,
            chunks,
        };
        let mut link = Link::new(id);
        link.name = Some(manifest.name().to_owned());
        row.link = self.share_link(&link).to_string();
        self.store.save_transfer(&row)?;
        self.insert(row, Some(Arc::new(manifest)), link).await
    }

    async fn insert(
        &mut self,
        row: TransferRow,
        manifest: Option<Arc<Manifest>>,
        link: Link,
    ) -> Result<TransferInfo, EngineError> {
        let id = row.id;
        self.transfers.insert(
            id,
            Transfer {
                row,
                manifest,
                link,
                live: Live::Idle,
                shown: None,
            },
        );
        self.reschedule().await;
        self.emit_changes();
        Ok(self.info(&id).expect("just inserted"))
    }

    /// A share's link: the bundle, this peer and its addresses, and the config's trackers.
    fn share_link(&self, link: &Link) -> Link {
        let mut link = link.clone();
        link.peer = Some(self.peer_id);
        link.addrs = self.addresses.clone();
        link.trackers = self.config.trackers.clone();
        link
    }

    fn remove(&mut self, id: BundleId, delete_files: bool, reply: Reply<()>) {
        let Some(t) = self.transfers.remove(&id) else {
            let _ = reply.send(Err(EngineError::NotFound(id)));
            return;
        };
        if let Err(error) = self.store.remove_transfer(&id) {
            self.transfers.insert(id, t);
            let _ = reply.send(Err(error.into()));
            return;
        }
        self.events.push(EngineEvent::Removed { id });
        let files = match (&t.row.content_dir, &t.manifest) {
            (Some(dir), Some(manifest)) if delete_files => Some(Files {
                dir: dir.clone(),
                manifest: manifest.clone(),
                own_dir: t.row.kind == Kind::Download,
            }),
            _ => None,
        };
        match t.live {
            Live::Running(run) | Live::Stopping(run) => {
                run.stop.send_replace(true);
                let burial = Burial {
                    generation: run.generation,
                    files,
                    reply,
                };
                self.burials.insert(id, burial);
            }
            // A storage still opening is dropped when it reports back.
            Live::Idle | Live::Opening { .. } => bury(files, reply),
        }
    }

    async fn move_in_queue(
        &mut self,
        id: BundleId,
        to: QueueMove,
    ) -> Result<Vec<TransferInfo>, EngineError> {
        let mut order: Vec<_> = self.transfers.values().map(|t| t.row.clone()).collect();
        order.sort_by_key(|row| row.queue_position);
        let mut ids: Vec<_> = order.iter().map(|row| row.id).collect();
        if !reorder(&mut ids, id, to) {
            return Err(EngineError::NotFound(id));
        }
        for (position, id) in ids.iter().enumerate() {
            let t = self.transfers.get_mut(id).expect("listed above");
            t.row.queue_position = position as u32;
        }
        self.store
            .save_transfers(self.transfers.values().map(|t| &t.row))?;
        self.reschedule().await;
        self.emit_changes();
        Ok(self.list())
    }

    /// Every transfer's state, in queue order.
    fn list(&self) -> Vec<TransferInfo> {
        let mut list: Vec<_> = self
            .transfers
            .keys()
            .filter_map(|id| self.info(id))
            .collect();
        list.sort_by_key(|info| info.queue_position);
        list
    }

    fn info(&self, id: &BundleId) -> Option<TransferInfo> {
        let t = self.transfers.get(id)?;
        let row = &t.row;
        let run = match &t.live {
            Live::Running(run) | Live::Stopping(run) => Some(run),
            Live::Idle | Live::Opening { .. } => None,
        };
        let net = run.map(|run| run.status.borrow().clone());
        let (phase, idle) = match &t.live {
            Live::Idle => (Phase::Idle, Duration::ZERO),
            Live::Opening { .. } => (Phase::Opening, Duration::ZERO),
            Live::Running(run) => (Phase::Running, run.last_progress.elapsed()),
            Live::Stopping(_) => (Phase::Stopping, Duration::ZERO),
        };
        let status = Status::of(StatusInput {
            phase,
            wanted: row.wanted,
            done: row.done,
            error: row.error.as_deref(),
            complete: row.is_complete(),
            manifest: t.manifest.is_some(),
            connections: net.as_ref().map_or(0, |s| s.connections),
            known_peers: net.as_ref().map_or(0, |s| s.known_peers),
            idle,
        });
        let name = row
            .name
            .clone()
            .or_else(|| t.manifest.as_ref().map(|m| m.name().to_owned()))
            .or_else(|| t.link.name.clone())
            .unwrap_or_else(|| id.to_string());
        let link = match row.kind {
            Kind::Share => self.share_link(&t.link).to_string(),
            Kind::Download => row.link.clone(),
        };
        Some(TransferInfo {
            id: *id,
            name,
            kind: row.kind,
            status,
            size: t.manifest.as_ref().map(|m| m.total_size()),
            have: row.have,
            chunks: row.chunks,
            downloaded: row.downloaded,
            uploaded: row.uploaded,
            download_rate: run.map_or(0, |run| run.download_rate),
            upload_rate: run.map_or(0, |run| run.upload_rate),
            connections: net.as_ref().map_or(0, |s| s.connections),
            peers: net.as_ref().map_or(0, |s| s.known_peers),
            added: row.added,
            completed: row.completed,
            queue_position: row.queue_position,
            forced: row.forced,
            tags: row.tags.clone(),
            category: row.category.clone(),
            save_folder: row.save_folder.clone(),
            content_dir: row.content_dir.clone(),
            link,
        })
    }

    /// Emits a snapshot of every transfer that changed since its last one.
    fn emit_changes(&mut self) {
        let ids: Vec<_> = self.transfers.keys().copied().collect();
        for id in ids {
            let info = self.info(&id).expect("listed above");
            let t = self.transfers.get_mut(&id).expect("listed above");
            if t.shown.as_ref() != Some(&info) {
                t.shown = Some(info.clone());
                self.events.push(EngineEvent::Transfer(info));
            }
        }
    }

    /// Starts the transfers the queue says should run, and stops those it says shouldn't.
    async fn reschedule(&mut self) {
        if self.shutting_down.is_some() {
            return;
        }
        let candidates: Vec<_> = self
            .transfers
            .values()
            .filter(|t| t.row.wanted && !t.row.done && t.row.error.is_none())
            .map(|t| Candidate {
                id: t.row.id,
                position: t.row.queue_position,
                complete: t.row.is_complete(),
                forced: t.row.forced,
            })
            .collect();
        let running = schedule(&candidates, self.config.queue);
        let ids: Vec<_> = self.transfers.keys().copied().collect();
        for id in ids {
            let live = &self.transfers[&id].live;
            match (running.contains(&id), live) {
                (true, Live::Idle) => self.launch(id).await,
                (false, Live::Running(_) | Live::Opening { .. }) => self.halt(id),
                _ => {}
            }
        }
    }

    fn generation(&mut self) -> u64 {
        self.next_generation += 1;
        self.next_generation
    }

    /// Starts a transfer: opens its storage if the manifest is known, or starts fetching it.
    async fn launch(&mut self, id: BundleId) {
        let generation = self.generation();
        let t = self.transfers.get_mut(&id).expect("launching a transfer");
        let Some(manifest) = t.manifest.clone() else {
            let session = match self.config.transfer_session(&t.row.settings) {
                Ok(session) => session,
                Err(error) => return self.fail(id, error.to_string()),
            };
            let mut peers = self.store.peers(&id).unwrap_or_default();
            peers.extend(link_record(&t.link));
            let destination = Destination::NewFolderIn(t.row.save_folder.clone());
            let started = self
                .node
                .download_with(id, destination, peers, Some(session))
                .await;
            return match started {
                Ok(handle) => self.attach(id, generation, handle).await,
                Err(error) => self.fail(id, error.to_string()),
            };
        };
        let dir = t
            .row
            .content_dir
            .clone()
            .expect("a transfer with a manifest has a content folder");
        let checkpoint = self
            .store
            .checkpoint(&id)
            .ok()
            .flatten()
            .and_then(|bytes| Checkpoint::from_bytes(&bytes).ok());
        let kind = t.row.kind;
        t.live = Live::Opening { generation };
        let internal = self.internal.clone();
        tokio::spawn(async move {
            let config = StorageConfig::default();
            let result = match (kind, checkpoint) {
                (Kind::Share, Some(c)) => Storage::resume_shared(dir, manifest, config, c).await,
                (Kind::Share, None) => Storage::open_shared(dir, manifest, config).await,
                (Kind::Download, Some(c)) => Storage::resume_download(dir, manifest, config, c)
                    .await
                    .map(|(storage, _)| storage),
                (Kind::Download, None) => Storage::create_download(dir, manifest, config)
                    .await
                    .map(|(storage, _)| storage),
            };
            let _ = internal
                .send(Internal::Opened {
                    id,
                    generation,
                    result,
                })
                .await;
        });
    }

    /// Runs a transfer whose storage just opened.
    async fn opened(&mut self, id: BundleId, generation: u64, storage: Storage) {
        let Some(t) = self.transfers.get_mut(&id) else {
            return;
        };
        if !matches!(t.live, Live::Opening { generation: g } if g == generation) {
            return;
        }
        t.live = Live::Idle;
        // What storage found may differ from the record, if files changed while stopped.
        if let Ok(have) = storage.have().await {
            t.row.have = have.chunk_count();
            if t.row.have < t.row.chunks {
                t.row.completed = None;
                t.row.done = false;
            } else {
                t.row.completed.get_or_insert_with(now_millis);
            }
            let _ = self.store.save_transfer(&t.row);
        }
        let session = match self.config.transfer_session(&t.row.settings) {
            Ok(session) => session,
            Err(error) => return self.fail(id, error.to_string()),
        };
        match self.node.share_with(storage, Some(session)).await {
            Ok(handle) => {
                let peers: Vec<_> = self
                    .store
                    .peers(&id)
                    .unwrap_or_default()
                    .into_iter()
                    .chain(link_record(&t.link))
                    .collect();
                if !peers.is_empty() {
                    handle.add_peers(peers).await;
                }
                self.attach(id, generation, handle).await;
            }
            Err(error) => self.fail(id, error.to_string()),
        }
    }

    /// Starts discovery for a running bundle and hands it to a supervisor.
    async fn attach(&mut self, id: BundleId, generation: u64, handle: BundleHandle) {
        let t = self.transfers.get_mut(&id).expect("attaching a transfer");
        let mut trackers = t.link.trackers.clone();
        for tracker in &self.config.trackers {
            if !trackers.contains(tracker) {
                trackers.push(tracker.clone());
            }
        }
        let announcers = trackers
            .into_iter()
            .map(|url| Announcer::spawn(self.tracker.clone(), url, self.announce.clone(), &handle))
            .collect();
        if let Some(lan) = &self.lan {
            lan.watch(&handle).await;
        }
        let (stop, stop_receiver) = watch::channel(false);
        let (checkpoint, checkpoint_receiver) = mpsc::channel(1);
        let now = Instant::now();
        t.live = Live::Running(Run {
            generation,
            status: handle.status.clone(),
            stop,
            checkpoint,
            base_uploaded: t.row.uploaded,
            base_downloaded: t.row.downloaded,
            last_progress: now,
            progress_mark: (0, 0, t.manifest.is_some()),
            sample: (now, 0, 0),
            download_rate: 0,
            upload_rate: 0,
            last_checkpoint: now,
        });
        let supervisor = Supervisor {
            id,
            generation,
            handle,
            announcers,
            internal: self.internal.clone(),
        };
        tokio::spawn(supervisor.run(stop_receiver, checkpoint_receiver));
    }

    /// Records that a transfer failed to start.
    fn fail(&mut self, id: BundleId, reason: String) {
        let Some(t) = self.transfers.get_mut(&id) else {
            return;
        };
        t.live = Live::Idle;
        t.row.error = Some(reason);
        let _ = self.store.save_transfer(&t.row);
    }

    /// Asks a running transfer to stop, or forgets a storage still opening.
    fn halt(&mut self, id: BundleId) {
        let t = self.transfers.get_mut(&id).expect("halting a transfer");
        t.live = match std::mem::replace(&mut t.live, Live::Idle) {
            Live::Running(run) => {
                run.stop.send_replace(true);
                if learn_manifest(&mut t.manifest, &mut t.row, &run.status.borrow()) {
                    let _ = self.store.save_transfer(&t.row);
                }
                Live::Stopping(run)
            }
            Live::Opening { .. } | Live::Idle => Live::Idle,
            stopping @ Live::Stopping(_) => stopping,
        };
    }

    async fn internal(&mut self, message: Internal) {
        match message {
            Internal::Opened {
                id,
                generation,
                result,
            } => match result {
                Ok(storage) => self.opened(id, generation, storage).await,
                Err(error) => {
                    let current = self.transfers.get(&id).is_some_and(
                        |t| matches!(t.live, Live::Opening { generation: g } if g == generation),
                    );
                    if current {
                        self.fail(id, error.to_string());
                    }
                }
            },
            Internal::Decisions { id, events, missed } => {
                if !self.transfers.contains_key(&id) {
                    return;
                }
                if missed > 0 {
                    self.events
                        .push(EngineEvent::DecisionsMissed { id, count: missed });
                }
                for event in events {
                    self.events.push(EngineEvent::Decision { id, event });
                }
                // Decisions change no transfer's state.
                return;
            }
            Internal::Checkpoint {
                id,
                generation,
                checkpoint,
            } => {
                let Some(t) = self.transfers.get_mut(&id) else {
                    return;
                };
                let current = match &t.live {
                    Live::Running(run) | Live::Stopping(run) => run.generation == generation,
                    _ => false,
                };
                if current {
                    t.row.have = checkpoint.chunks_stored();
                    let _ = self.store.save_checkpoint(&id, &checkpoint.to_bytes());
                    let _ = self.store.save_transfer(&t.row);
                }
            }
            Internal::Finished {
                id,
                generation,
                result,
            } => self.finished(id, generation, result).await,
        }
        self.emit_changes();
    }

    /// Records how a bundle ended.
    async fn finished(
        &mut self,
        id: BundleId,
        generation: u64,
        result: Result<Summary, DriverError>,
    ) {
        if self
            .burials
            .get(&id)
            .is_some_and(|burial| burial.generation == generation)
        {
            let burial = self.burials.remove(&id).expect("checked above");
            bury(burial.files, burial.reply);
            return;
        }
        let Some(t) = self.transfers.get_mut(&id) else {
            return;
        };
        let (run, asked) = match std::mem::replace(&mut t.live, Live::Idle) {
            Live::Running(run) if run.generation == generation => (run, false),
            Live::Stopping(run) if run.generation == generation => (run, true),
            other => {
                t.live = other;
                return;
            }
        };
        // The manifest may have arrived since the last tick.
        learn_manifest(&mut t.manifest, &mut t.row, &run.status.borrow());
        match result {
            Ok(summary) => {
                t.row.uploaded = run.base_uploaded + summary.uploaded;
                t.row.downloaded = run.base_downloaded + summary.downloaded;
                if let Some(checkpoint) = summary.checkpoint {
                    t.row.have = checkpoint.chunks_stored();
                    let _ = self.store.save_checkpoint(&id, &checkpoint.to_bytes());
                }
                if summary.complete {
                    t.row.completed.get_or_insert_with(now_millis);
                    // It stopped sharing by itself: lingering is over (D-065).
                    t.row.done |= !asked;
                }
            }
            Err(error) => t.row.error = Some(error.to_string()),
        }
        let _ = self.store.save_transfer(&t.row);
        self.reschedule().await;
    }

    /// Reads every running bundle's status, records what changed, takes checkpoints when due,
    /// and emits snapshots.
    async fn tick(&mut self) {
        let now = Instant::now();
        let mut queue_changed = false;
        for t in self.transfers.values_mut() {
            let Live::Running(run) = &mut t.live else {
                continue;
            };
            let status = run.status.borrow().clone();
            if learn_manifest(&mut t.manifest, &mut t.row, &status) {
                let _ = self.store.save_transfer(&t.row);
            }
            t.row.have = u64::from(status.have);
            t.row.uploaded = run.base_uploaded + status.uploaded;
            t.row.downloaded = run.base_downloaded + status.downloaded;
            if status.complete && t.row.completed.is_none() {
                t.row.completed = Some(now_millis());
                let _ = self.store.save_transfer(&t.row);
                queue_changed = true;
            }

            let mark = (status.have, status.downloaded, status.manifest.is_some());
            if mark != run.progress_mark {
                run.progress_mark = mark;
                run.last_progress = now;
            }
            let (then, downloaded, uploaded) = run.sample;
            let elapsed = now.duration_since(then).as_secs_f64();
            if elapsed > 0.0 {
                let rate = |bytes: u64| (bytes as f64 / elapsed) as u64;
                // Half the latest sample and half the previous rate: steady, yet quick to react.
                run.download_rate =
                    (run.download_rate + rate(status.downloaded.saturating_sub(downloaded))) / 2;
                run.upload_rate =
                    (run.upload_rate + rate(status.uploaded.saturating_sub(uploaded))) / 2;
                run.sample = (now, status.downloaded, status.uploaded);
            }
            if !status.complete
                && t.manifest.is_some()
                && now.duration_since(run.last_checkpoint) >= CHECKPOINT_INTERVAL
            {
                run.last_checkpoint = now;
                let _ = run.checkpoint.try_send(());
            }
        }
        if queue_changed {
            self.reschedule().await;
        }
        self.emit_changes();
    }
}

/// Records a download's manifest once its status shows it. Returns whether it was new.
fn learn_manifest(
    known: &mut Option<Arc<Manifest>>,
    row: &mut TransferRow,
    status: &tempest_net::Status,
) -> bool {
    let (None, Some(manifest)) = (&known, &status.manifest) else {
        return false;
    };
    row.manifest = Some(manifest.to_bytes());
    row.chunks = u64::from(ChunkLayout::new(manifest).chunk_count());
    row.content_dir = Some(row.save_folder.join(manifest.name()));
    *known = Some(manifest.clone());
    true
}

/// The peer a link names, if it gives an address to reach it at.
fn link_record(link: &Link) -> Option<PeerRecord> {
    let peer_id = link.peer?;
    (!link.addrs.is_empty() || link.relay.is_some()).then(|| PeerRecord {
        peer_id,
        addresses: link.addrs.clone(),
        relay: link.relay,
    })
}

/// Deletes a removed transfer's files, if asked to, then answers the removal.
fn bury(files: Option<Files>, reply: Reply<()>) {
    let Some(files) = files else {
        let _ = reply.send(Ok(()));
        return;
    };
    tokio::task::spawn_blocking(move || {
        let _ = reply.send(delete_files(&files));
    });
}

/// Deletes a bundle's files, symlinks and then directories, leaving anything else in them. A
/// file already gone is fine.
fn delete_files(files: &Files) -> Result<(), EngineError> {
    use tempest_protocol::EntryKind;

    let fs_path = |path: &str| -> PathBuf {
        let mut fs_path = files.dir.clone();
        fs_path.extend(path.split('/'));
        fs_path
    };
    let io_error = |path: &Path| {
        let path = path.to_owned();
        move |source| EngineError::Io { path, source }
    };
    let mut dirs = BTreeSet::new();
    for entry in files.manifest.entries() {
        let path = fs_path(&entry.path);
        for ancestor in tempest_protocol::path::ancestors(&entry.path) {
            dirs.insert(ancestor.to_owned());
        }
        match entry.kind {
            EntryKind::Directory => {
                dirs.insert(entry.path.clone());
            }
            EntryKind::File(_) | EntryKind::Symlink { .. } => match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    return Err(io_error(&path)(e));
                }
                _ => {}
            },
        }
    }
    // Deepest first. A directory that still holds something else stays.
    let mut dirs: Vec<_> = dirs.into_iter().collect();
    dirs.sort_by_key(|dir| std::cmp::Reverse(dir.matches('/').count()));
    for dir in dirs {
        let _ = std::fs::remove_dir(fs_path(&dir));
    }
    if files.own_dir {
        let _ = std::fs::remove_dir(&files.dir);
    }
    Ok(())
}
