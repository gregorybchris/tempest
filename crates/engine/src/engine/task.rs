//! The engine task: every transfer's record, the queue, and what running bundles report.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::Value;
use tempest_net::{
    AnnounceConfig, Announcer, BundleHandle, Checkpoint, Destination, DriverError, Inspector, Lan,
    Node, SpeedLimits, Storage, StorageConfig, Summary, TrackerClient,
};
use tempest_protocol::message::PeerRecord;
use tempest_protocol::wire::{Decode, Encode};
use tempest_protocol::{
    Address, BundleId, ChunkLayout, ChunkRange, Link, Manifest, PeerId, RangeList,
};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{Instant, MissedTickBehavior};

use super::notify::{self, Notice};
use super::relocate::{self, Item};
use super::supervisor::Supervisor;
use super::{
    AddOptions, CHECKPOINT_INTERVAL, Command, EVENT_BUFFER, EngineError, GOODBYE_TIMEOUT, Inside,
    Internal, NewShare, Reply, SettingsView, TICK,
};
use crate::TransferInfo;
use crate::config::{EngineConfig, SharingAction, SharingLimits};
use crate::events::{EngineEvent, EventLog};
use crate::manifest::unix_millis;
use crate::queue::{Candidate, QueueMove, reorder, schedule};
use crate::schedule::LocalTime;
use crate::status::{Phase, Status, StatusInput};
use crate::store::{Kind, Store, TransferRow};

/// What the engine task starts with.
pub(super) struct Parts {
    /// The config file's settings.
    pub base: EngineConfig,
    /// The settings changed through the engine, by dotted key.
    pub changes: BTreeMap<String, Value>,
    /// The settings in effect: the file's with the changes.
    pub config: EngineConfig,
    /// The settings the engine started with.
    pub running: EngineConfig,
    /// Run no transfers.
    pub hold: bool,
    /// The data directory's lock.
    pub lock: std::fs::File,
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
    /// Its data is to be checked from scratch once it stops.
    recheck: bool,
    /// Its files are to move to this folder once it stops.
    move_to: Option<PathBuf>,
}

/// Whether a transfer is running.
enum Live {
    Idle,
    /// Its storage is opening; with `recheck`, hashing every file, whether or not it is to run.
    Opening {
        generation: u64,
        recheck: bool,
    },
    Running(Run),
    Stopping(Run),
    /// Its files are moving into a folder.
    Moving {
        generation: u64,
        to: PathBuf,
    },
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
    /// When the time spent sharing was last counted.
    last_tick: Instant,
    /// The bundle's own speed limits.
    limits: SpeedLimits,
    /// When it stops sharing.
    sharing: SharingLimits,
    /// For looking into the bundle.
    inspector: Inspector,
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
    base: EngineConfig,
    changes: BTreeMap<String, Value>,
    config: EngineConfig,
    running: EngineConfig,
    hold: bool,
    _lock: std::fs::File,
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
            base,
            changes,
            config,
            running,
            hold,
            lock,
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
            base,
            changes,
            config,
            running,
            hold,
            _lock: lock,
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
                    recheck: false,
                    move_to: None,
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
        let waiting = self.shutting_down.take();
        // Closes the database and releases the data directory before anyone hears it's done.
        drop(self);
        for done in waiting.into_iter().flatten() {
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
            Command::Inside(id, reply) => {
                let _ = reply.send(self.inside(id));
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
            Command::Recheck(id, reply) => {
                let result = self.recheck(id).await;
                let _ = reply.send(result);
            }
            Command::MoveStorage(id, folder, reply) => {
                let result = self.move_storage(id, folder).await;
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
                let result = match self.config.transfer(&settings) {
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
            Command::Settings(reply) => {
                let _ = reply.send(self.settings_view());
            }
            Command::ChangeSettings(changes, reply) => {
                let result = self.change_settings(changes).await;
                let _ = reply.send(result);
            }
            Command::ResetSetting(key, reply) => {
                let result = self.reset_setting(key).await;
                let _ = reply.send(result);
            }
            Command::Subscribe(after, reply) => {
                let _ = reply.send(self.events.subscribe(after));
            }
            Command::Shutdown(done) => self.begin_shutdown(Some(done)),
        }
    }

    fn settings_view(&self) -> SettingsView {
        SettingsView {
            settings: self.config.clone(),
            changed: self.changes.clone(),
            restart_required: (self.config.restart_required(&self.running).iter())
                .map(|group| (*group).to_owned())
                .collect(),
            alternative_limits_active: self.config.limits.alternative_active(LocalTime::now()),
        }
    }

    async fn change_settings(
        &mut self,
        changes: BTreeMap<String, Value>,
    ) -> Result<SettingsView, EngineError> {
        if let Some(key) = changes.keys().find(|key| key.as_str() == "data_dir") {
            return Err(EngineError::Fixed(key.clone()));
        }
        let mut all = self.changes.clone();
        all.extend(changes.clone());
        self.apply_settings(all).await?;
        for (key, value) in &changes {
            self.store.set_setting(key, value)?;
        }
        Ok(self.settings_view())
    }

    async fn reset_setting(&mut self, key: String) -> Result<SettingsView, EngineError> {
        let mut all = self.changes.clone();
        if all.remove(&key).is_some() {
            self.apply_settings(all).await?;
            self.store.remove_setting(&key)?;
        }
        Ok(self.settings_view())
    }

    /// Makes `changes` over the config file the settings in effect, if they are valid.
    async fn apply_settings(
        &mut self,
        changes: BTreeMap<String, Value>,
    ) -> Result<(), EngineError> {
        let config = self.base.with_changes(&changes)?;
        config.validate()?;
        self.save_folder = config.save_folder()?;
        self.config = config;
        self.changes = changes;
        // New queue and speed limits, relays and port mapping apply now, new trackers to shares'
        // links.
        self.apply_speed_limits();
        self.apply_relays();
        self.apply_port_mapping();
        self.refresh_runs();
        self.reschedule().await;
        self.emit_changes();
        Ok(())
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
        self.refresh_runs();
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
            seeding_time: 0,
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
            seeding_time: 0,
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
                recheck: false,
                move_to: None,
            },
        );
        self.reschedule().await;
        self.emit_changes();
        Ok(self.info(&id).expect("just inserted"))
    }

    /// Picks a home relay from the configured relays. A held engine runs no transfers, so
    /// nobody needs to reach it.
    pub(super) fn apply_relays(&self) {
        if !self.hold {
            let endpoint = self.node.endpoint();
            endpoint.set_relays(self.config.relays.choice());
        }
    }

    /// Asks the router to forward the peer port, or stops. A held engine needs no port.
    pub(super) fn apply_port_mapping(&self) {
        if !self.hold {
            let endpoint = self.node.endpoint();
            endpoint.set_port_mapping(self.config.port_mapping.config());
        }
    }

    /// A share's link: the bundle, this peer and its addresses, and the config's trackers.
    fn share_link(&self, link: &Link) -> Link {
        let mut link = link.clone();
        link.peer = Some(self.peer_id);
        link.addrs = self
            .node
            .endpoint()
            .port_mapping()
            .advertise(&self.addresses);
        link.relay = self.node.endpoint().relay_address();
        link.trackers = self.config.trackers.clone();
        link
    }

    fn remove(&mut self, id: BundleId, delete_files: bool, reply: Reply<()>) {
        if let Some(Transfer {
            live: Live::Moving { .. },
            ..
        }) = self.transfers.get(&id)
        {
            let _ = reply.send(Err(EngineError::Moving(id)));
            return;
        }
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
            Live::Idle | Live::Opening { .. } | Live::Moving { .. } => bury(files, reply),
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

    /// What it takes to look inside a transfer, without waiting on its bundle here.
    fn inside(&self, id: BundleId) -> Result<Inside, EngineError> {
        let t = self.transfers.get(&id).ok_or(EngineError::NotFound(id))?;
        // A stopping bundle is left out: it is about to go.
        let running = match &t.live {
            Live::Running(run) => Some(run.inspector.clone()),
            Live::Stopping(_) | Live::Idle | Live::Opening { .. } | Live::Moving { .. } => None,
        };
        let stored = match (&t.manifest, &running) {
            (Some(manifest), None) => Some(self.stored_chunks(id, &t.row, manifest)),
            _ => None,
        };
        Ok(Inside {
            manifest: t.manifest.clone(),
            running,
            stored,
        })
    }

    /// The chunks a transfer that isn't running has stored: as of its checkpoint, or all of
    /// them once complete.
    fn stored_chunks(&self, id: BundleId, row: &TransferRow, manifest: &Manifest) -> RangeList {
        let layout = ChunkLayout::new(manifest);
        let checkpoint = (self.store.checkpoint(&id).ok().flatten())
            .and_then(|bytes| Checkpoint::from_bytes(&bytes).ok());
        match checkpoint {
            Some(checkpoint) => checkpoint.have(&layout),
            None if row.is_complete() && layout.chunk_count() > 0 => {
                let all = ChunkRange {
                    start: 0,
                    end: layout.chunk_count(),
                };
                RangeList::from_ranges(vec![all]).expect("one nonempty range")
            }
            None => RangeList::new(),
        }
    }

    fn info(&self, id: &BundleId) -> Option<TransferInfo> {
        let t = self.transfers.get(id)?;
        let row = &t.row;
        let run = match &t.live {
            Live::Running(run) | Live::Stopping(run) => Some(run),
            Live::Idle | Live::Opening { .. } | Live::Moving { .. } => None,
        };
        let net = run.map(|run| run.status.borrow().clone());
        let (phase, idle) = match &t.live {
            Live::Idle => (Phase::Idle, Duration::ZERO),
            Live::Opening { .. } => (Phase::Opening, Duration::ZERO),
            Live::Running(run) => (Phase::Running, run.last_progress.elapsed()),
            Live::Stopping(_) => (Phase::Stopping, Duration::ZERO),
            Live::Moving { .. } => (Phase::Moving, Duration::ZERO),
        };
        let status = crate::status::of(StatusInput {
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
        // A held engine's addresses are nobody's to dial.
        let link = match row.kind {
            Kind::Share if !self.hold => self.share_link(&t.link).to_string(),
            Kind::Share | Kind::Download => row.link.clone(),
        };
        Some(crate::with_derived(TransferInfo {
            id: *id,
            name,
            kind: row.kind,
            status,
            size: t.manifest.as_ref().map(|m| m.total_size()),
            have: row.have,
            chunks: row.chunks,
            progress: 0.0,
            eta: None,
            ratio: None,
            downloaded: row.downloaded,
            uploaded: row.uploaded,
            seeding_time: row.seeding_time / 1000,
            download_rate: run.map_or(0, |run| run.download_rate),
            upload_rate: run.map_or(0, |run| run.upload_rate),
            connections: net.as_ref().map_or(0, |s| s.connections),
            relayed: net.as_ref().map_or(0, |s| s.relayed),
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
            settings: row.settings.clone(),
        }))
    }

    /// Emits a snapshot of every transfer that changed since its last one, and runs the
    /// notification command for those that just completed or failed.
    fn emit_changes(&mut self) {
        let ids: Vec<_> = self.transfers.keys().copied().collect();
        for id in ids {
            let info = self.info(&id).expect("listed above");
            let t = self.transfers.get_mut(&id).expect("listed above");
            if t.shown.as_ref() == Some(&info) {
                continue;
            }
            let notice = t.shown.as_ref().and_then(|shown| notice(shown, &info));
            t.shown = Some(info.clone());
            if let Some(notice) = notice {
                notify::run(&self.config.notifications.command, notice, &info);
            }
            self.events.push(EngineEvent::Transfer(Box::new(info)));
        }
    }

    /// Starts the transfers the queue says should run, and stops those it says shouldn't.
    async fn reschedule(&mut self) {
        if self.shutting_down.is_some() || self.hold {
            return;
        }
        let running = self.scheduled();
        let ids: Vec<_> = self.transfers.keys().copied().collect();
        for id in ids {
            let live = &self.transfers[&id].live;
            match (running.contains(&id), live) {
                (true, Live::Idle) => self.launch(id, false).await,
                (false, Live::Running(_) | Live::Opening { recheck: false, .. }) => self.halt(id),
                _ => {}
            }
        }
    }

    fn generation(&mut self) -> u64 {
        self.next_generation += 1;
        self.next_generation
    }

    /// Starts a transfer: opens its storage if the manifest is known, or starts fetching it.
    /// With `recheck`, every file is hashed, ignoring the saved checkpoint, and the transfer
    /// runs afterwards only if the queue says it should.
    async fn launch(&mut self, id: BundleId, recheck: bool) {
        let generation = self.generation();
        let t = self.transfers.get_mut(&id).expect("launching a transfer");
        t.recheck = false;
        let Some(manifest) = t.manifest.clone() else {
            let session = match self.config.transfer(&t.row.settings) {
                Ok(transfer) => transfer.session,
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
        let checkpoint = match recheck {
            true => None,
            false => self
                .store
                .checkpoint(&id)
                .ok()
                .flatten()
                .and_then(|bytes| Checkpoint::from_bytes(&bytes).ok()),
        };
        let kind = t.row.kind;
        t.live = Live::Opening {
            generation,
            recheck,
        };
        let internal = self.internal.clone();
        tokio::spawn(async move {
            let config = StorageConfig::default();
            let result = match (kind, checkpoint) {
                (Kind::Share, Some(c)) => Storage::resume_shared(dir, manifest, config, c).await,
                // A recheck finds what is intact even in a damaged share, which then downloads
                // the rest.
                (Kind::Share, None) if !recheck => {
                    Storage::open_shared(dir, manifest, config).await
                }
                (Kind::Download, Some(c)) => Storage::resume_download(dir, manifest, config, c)
                    .await
                    .map(|(storage, _)| storage),
                (Kind::Share | Kind::Download, None) => {
                    Storage::create_download(dir, manifest, config)
                        .await
                        .map(|(storage, _)| storage)
                }
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

    /// Whether the queue would run a transfer now.
    fn should_run(&self, id: BundleId) -> bool {
        self.shutting_down.is_none() && !self.hold && self.scheduled().contains(&id)
    }

    /// The transfers the queue says should run.
    fn scheduled(&self) -> std::collections::HashSet<BundleId> {
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
        schedule(&candidates, self.config.queue)
            .into_iter()
            .collect()
    }

    /// Runs a transfer whose storage just opened, or, after a recheck the queue doesn't want
    /// running, records what storage found and closes it.
    async fn opened(&mut self, id: BundleId, generation: u64, storage: Storage) {
        let Some(t) = self.transfers.get_mut(&id) else {
            return;
        };
        if !matches!(t.live, Live::Opening { generation: g, .. } if g == generation) {
            return;
        }
        t.live = Live::Idle;
        // What storage found may differ from the record, if files changed while stopped.
        if let Ok(checkpoint) = storage.checkpoint().await {
            t.row.have = checkpoint.chunks_stored();
            if t.row.have < t.row.chunks {
                t.row.completed = None;
                t.row.done = false;
            } else {
                t.row.completed.get_or_insert_with(now_millis);
            }
            let _ = self.store.save_checkpoint(&id, &checkpoint.to_bytes());
            let _ = self.store.save_transfer(&t.row);
        }
        if !self.should_run(id) {
            let _ = storage.close().await;
            return;
        }
        let t = self.transfers.get_mut(&id).expect("checked above");
        let session = match self.config.transfer(&t.row.settings) {
            Ok(transfer) => transfer.session,
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
        let limits = handle.limits().clone();
        let sharing = match self.config.transfer(&t.row.settings) {
            Ok(transfer) => {
                limits.download.set_rate(transfer.limits.download);
                limits.upload.set_rate(transfer.limits.upload);
                transfer.sharing
            }
            Err(_) => SharingLimits::default(),
        };
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
            last_tick: now,
            limits,
            sharing,
            inspector: handle.inspector(),
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

    /// Checks a transfer's data from scratch: now if it is idle, or once it has stopped.
    async fn recheck(&mut self, id: BundleId) -> Result<TransferInfo, EngineError> {
        let t = self
            .transfers
            .get_mut(&id)
            .ok_or(EngineError::NotFound(id))?;
        if matches!(t.live, Live::Moving { .. }) || t.move_to.is_some() {
            return Err(EngineError::Moving(id));
        }
        if t.manifest.is_none() {
            return Err(EngineError::NoData(id));
        }
        self.store.clear_checkpoint(&id)?;
        t.recheck = true;
        // A failure that sent the user here may be fixed by what the check finds.
        t.row.error = None;
        match t.live {
            Live::Running(_) | Live::Opening { .. } => self.halt(id),
            Live::Stopping(_) | Live::Idle | Live::Moving { .. } => {}
        }
        self.after_stop(id).await;
        self.emit_changes();
        Ok(self.info(&id).expect("the transfer exists"))
    }

    /// Moves a transfer's files to `folder`: now if it is idle, or once it has stopped.
    async fn move_storage(
        &mut self,
        id: BundleId,
        folder: PathBuf,
    ) -> Result<TransferInfo, EngineError> {
        let folder = checked_path(&folder)?;
        let t = self
            .transfers
            .get_mut(&id)
            .ok_or(EngineError::NotFound(id))?;
        if matches!(t.live, Live::Moving { .. }) || t.move_to.is_some() {
            return Err(EngineError::Moving(id));
        }
        t.move_to = Some(folder);
        if matches!(t.live, Live::Running(_) | Live::Opening { .. }) {
            self.halt(id);
        }
        self.after_stop(id).await;
        self.reschedule().await;
        self.emit_changes();
        Ok(self.info(&id).expect("the transfer exists"))
    }

    /// Starts moving an idle transfer's files to `folder`.
    fn start_move(&mut self, id: BundleId, folder: PathBuf) {
        let generation = self.generation();
        let t = self.transfers.get_mut(&id).expect("moving a transfer");
        let (Some(manifest), Some(dir)) = (&t.manifest, &t.row.content_dir) else {
            return;
        };
        let items: Vec<Item> = match t.row.kind {
            Kind::Download => vec![Item {
                from: dir.clone(),
                to: folder.join(manifest.name()),
            }],
            // A share's entries move one by one: its base folder may hold other things.
            Kind::Share => manifest
                .entries()
                .iter()
                .filter_map(|entry| entry.path.split('/').next())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .map(|top| Item {
                    from: dir.join(top),
                    to: folder.join(top),
                })
                .collect(),
        };
        if items.iter().all(|item| item.from == item.to) {
            return;
        }
        t.live = Live::Moving {
            generation,
            to: folder,
        };
        let internal = self.internal.clone();
        tokio::task::spawn_blocking(move || {
            let result = relocate::move_all(&items);
            let _ = internal.blocking_send(Internal::Moved {
                id,
                generation,
                result,
            });
        });
    }

    /// Records where a transfer's files ended up, and lets it run again.
    async fn moved(&mut self, id: BundleId, generation: u64, result: std::io::Result<()>) {
        let Some(t) = self.transfers.get_mut(&id) else {
            return;
        };
        let folder = match std::mem::replace(&mut t.live, Live::Idle) {
            Live::Moving { generation: g, to } if g == generation => to,
            other => {
                t.live = other;
                return;
            }
        };
        let manifest = t.manifest.clone().expect("a moved transfer has a manifest");
        let new_dir = match t.row.kind {
            Kind::Download => folder.join(manifest.name()),
            Kind::Share => folder.clone(),
        };
        match result {
            Ok(()) => {
                t.row.save_folder = folder;
                t.row.content_dir = Some(new_dir);
            }
            Err(error) => {
                let old_dir = t.row.content_dir.as_deref().unwrap_or(Path::new(""));
                t.row.error = Some(format!(
                    "Couldn't move the files to {}: {error}. They are still in {}.",
                    folder.display(),
                    old_dir.display()
                ));
            }
        }
        let _ = self.store.save_transfer(&t.row);
        self.after_stop(id).await;
        self.reschedule().await;
    }

    /// Applies each running transfer's own speed and sharing limits, after settings change.
    fn refresh_runs(&mut self) {
        for t in self.transfers.values_mut() {
            let (Live::Running(run) | Live::Stopping(run)) = &mut t.live else {
                continue;
            };
            if let Ok(transfer) = self.config.transfer(&t.row.settings) {
                run.limits.download.set_rate(transfer.limits.download);
                run.limits.upload.set_rate(transfer.limits.upload);
                run.sharing = transfer.sharing;
            }
        }
    }

    /// Applies the speed limits over every transfer in effect now: the usual ones, or the
    /// alternative ones while switched on or scheduled.
    fn apply_speed_limits(&self) {
        let limits = self.config.limits.in_effect(LocalTime::now());
        let node = self.node.limits();
        node.download.set_rate(limits.download);
        node.upload.set_rate(limits.upload);
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
            unchanged @ (Live::Stopping(_) | Live::Moving { .. }) => unchanged,
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
                        |t| matches!(t.live, Live::Opening { generation: g, .. } if g == generation),
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
                // A recheck has cleared the checkpoint, and will take its own.
                if current && !t.recheck {
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
            Internal::Moved {
                id,
                generation,
                result,
            } => self.moved(id, generation, result).await,
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
                if let Some(checkpoint) = summary.checkpoint.filter(|_| !t.recheck) {
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
        self.after_stop(id).await;
        self.reschedule().await;
    }

    /// Starts what waited for a transfer to stop: a move, then a recheck.
    async fn after_stop(&mut self, id: BundleId) {
        let Some(t) = self.transfers.get_mut(&id) else {
            return;
        };
        if !matches!(t.live, Live::Idle) {
            return;
        }
        if let Some(folder) = t.move_to.take() {
            match t.manifest {
                Some(_) => return self.start_move(id, folder),
                // Nothing is on disk yet: the download starts again in the new folder.
                None => {
                    t.row.save_folder = folder;
                    let _ = self.store.save_transfer(&t.row);
                }
            }
        }
        if t.recheck {
            self.launch(id, true).await;
        }
    }

    /// Reads every running bundle's status, records what changed, takes checkpoints when due,
    /// stops transfers at their sharing limits, applies the speed limits in effect, and emits
    /// snapshots.
    async fn tick(&mut self) {
        let now = Instant::now();
        let mut queue_changed = false;
        let mut at_limit = Vec::new();
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

            if status.complete {
                let shared = now.duration_since(run.last_tick).as_millis() as u64;
                t.row.seeding_time += shared;
                let size = t.manifest.as_ref().map(|m| m.total_size());
                let ratio = crate::ratio(t.row.uploaded, t.row.downloaded, size);
                let seeding = Duration::from_millis(t.row.seeding_time);
                if run.sharing.reached(ratio, seeding) {
                    at_limit.push((t.row.id, run.sharing.action));
                }
            }
            run.last_tick = now;
            if now.duration_since(run.last_checkpoint) >= CHECKPOINT_INTERVAL {
                run.last_checkpoint = now;
                match status.complete {
                    // What a complete bundle holds doesn't change; its time sharing does.
                    true => {
                        let _ = self.store.save_transfer(&t.row);
                    }
                    false if t.manifest.is_some() => {
                        let _ = run.checkpoint.try_send(());
                    }
                    false => {}
                }
            }
        }
        for (id, action) in at_limit {
            self.sharing_limit_reached(id, action);
            queue_changed = true;
        }
        self.apply_speed_limits();
        if queue_changed {
            self.reschedule().await;
        }
        self.emit_changes();
    }

    /// Stops or removes a transfer that reached a sharing limit.
    fn sharing_limit_reached(&mut self, id: BundleId, action: SharingAction) {
        match action {
            SharingAction::Stop => {
                let t = self.transfers.get_mut(&id).expect("a running transfer");
                // Stopped by a rule, not by the user: completed (D-180).
                t.row.done = true;
                let _ = self.store.save_transfer(&t.row);
            }
            SharingAction::Remove => {
                let (reply, _) = oneshot::channel();
                self.remove(id, false, reply);
            }
        }
    }
}

/// What to notify about a transfer that went from `before` to `after`, if anything.
fn notice(before: &TransferInfo, after: &TransferInfo) -> Option<Notice> {
    let errored = |info: &TransferInfo| matches!(info.status, Status::Errored { .. });
    if errored(after) && !errored(before) {
        Some(Notice::Errored)
    } else if after.completed.is_some() && before.completed.is_none() {
        Some(Notice::Completed)
    } else {
        None
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
