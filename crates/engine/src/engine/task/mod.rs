//! The engine task: every transfer's record, the queue, and what running bundles report.
//!
//! This module holds the task's state, its loop and its commands. [`lifecycle`] runs
//! transfers, [`files`] moves and deletes their files, and [`settings`] applies settings.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::Value;
use tempest_net::storage::find_entry;
use tempest_net::{
    AnnounceConfig, Announcer, BundleHandle, Checkpoint, Destination, DriverError, Identity,
    Inspector, Lan, Node, SpeedLimits, Storage, StorageConfig, Summary, TrackerClient,
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
use super::supervisor::{ListingChange, Supervisor};
use super::{
    AddOptions, CHECKPOINT_INTERVAL, Command, EVENT_BUFFER, EngineError, GOODBYE_TIMEOUT, Inside,
    Internal, NewShare, Reply, SHUTDOWN_TIMEOUT, SettingsView, TICK, Unpublished,
};
use crate::TransferInfo;
use crate::config::{EngineConfig, SharingAction, SharingLimits};
use crate::events::{EngineEvent, EventLog};
use crate::manifest::unix_millis;
use crate::queue::{Candidate, QueueMove, reorder, schedule};
use crate::schedule::local_now;
use crate::status::{Phase, Status, StatusInput};
use crate::store::{Kind, Store, TransferRow};

mod files;
mod lifecycle;
mod settings;

use self::files::{bury, move_items};
use self::lifecycle::{learn_manifest, link_record};

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
    /// Signs announces' listing requests.
    pub identity: Identity,
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
        /// Aborting it stops the opening's work.
        task: tokio::task::AbortHandle,
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
    /// Changes to the trackers it is published on.
    listing: mpsc::UnboundedSender<ListingChange>,
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
    identity: Identity,
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

/// Saves a transfer's record. If the store fails, the error is logged and put on the transfer,
/// which stops it at the next reschedule: the engine can't keep what it does. Returns whether
/// it saved.
fn save_row(store: &mut Store, row: &mut TransferRow) -> bool {
    match store.save_transfer(row) {
        Ok(()) => true,
        Err(error) => {
            tracing::error!("couldn't save transfer {}: {error}", row.id);
            row.error
                .get_or_insert_with(|| format!("Couldn't save the transfer's state: {error}"));
            false
        }
    }
}

/// Saves a transfer's checkpoint. A failure is logged: the transfer would resume from an older
/// checkpoint, checking again what it has stored since.
fn save_checkpoint(store: &mut Store, id: &BundleId, checkpoint: &Checkpoint) {
    if let Err(error) = store.save_checkpoint(id, &checkpoint.to_bytes()) {
        tracing::warn!("couldn't save the checkpoint of transfer {id}: {error}");
    }
}

/// A transfer's saved checkpoint, if there is one that decodes.
fn load_checkpoint(store: &Store, id: &BundleId) -> Option<Checkpoint> {
    let bytes = store
        .checkpoint(id)
        .inspect_err(|error| tracing::warn!("couldn't read the checkpoint of {id}: {error}"))
        .ok()??;
    Checkpoint::from_bytes(&bytes)
        .inspect_err(|error| tracing::warn!("ignoring the checkpoint of {id}: {error}"))
        .ok()
}

/// The peers saved for a transfer.
fn load_peers(store: &Store, id: &BundleId) -> Vec<PeerRecord> {
    store
        .peers(id)
        .inspect_err(|error| tracing::warn!("couldn't read the peers of {id}: {error}"))
        .unwrap_or_default()
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
            identity,
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
            identity,
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
        for (id, folder) in self.store.moves()? {
            self.recover_move(id, &folder);
        }
        Ok(())
    }

    /// Settles a move a crash interrupted: forgets it if the transfer's record already has the
    /// new folder, and otherwise undoes it, since the record has the old one.
    fn recover_move(&mut self, id: BundleId, folder: &Path) {
        let Some(t) = self.transfers.get_mut(&id) else {
            return;
        };
        if let (Some(manifest), Some(dir)) = (&t.manifest, &t.row.content_dir)
            && t.row.save_folder != folder
        {
            let items = move_items(&t.row, manifest, dir, folder, true);
            match relocate::undo_interrupted(&items) {
                Ok(copies) => {
                    tracing::warn!(
                        "undid moving transfer {id} to {}, which was interrupted",
                        folder.display()
                    );
                    for copy in copies {
                        tracing::warn!("left {}, maybe incomplete, in place", copy.display());
                    }
                }
                Err(error) => {
                    t.row.error = Some(format!(
                        "Moving the files to {} was interrupted, and they couldn't all be put \
                         back in {}: {error}",
                        folder.display(),
                        dir.display()
                    ));
                    // Undoing is tried again at the next start.
                    save_row(&mut self.store, &mut t.row);
                    return;
                }
            }
        }
        if let Err(error) = self.store.end_move(&id) {
            tracing::error!("couldn't forget transfer {id}'s move: {error}");
        }
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
        // When shutting down gives up on transfers still stopping.
        let mut give_up = None;
        loop {
            if self.shutting_down.is_some() {
                if self.is_idle() {
                    break;
                }
                give_up.get_or_insert_with(|| Instant::now() + SHUTDOWN_TIMEOUT);
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
                () = tokio::time::sleep_until(give_up.unwrap_or_else(Instant::now)),
                    if give_up.is_some() =>
                {
                    self.abandon_stopping();
                    break;
                }
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

    /// Logs what shutting down leaves unfinished. Each transfer keeps its last saved checkpoint.
    fn abandon_stopping(&self) {
        for (id, t) in &self.transfers {
            if !matches!(t.live, Live::Idle) {
                tracing::warn!(
                    "transfer {id} didn't stop within {SHUTDOWN_TIMEOUT:?}; shutting down anyway"
                );
            }
        }
        for id in self.burials.keys() {
            tracing::warn!("removed transfer {id} didn't stop within {SHUTDOWN_TIMEOUT:?}");
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
                let result = self.add_link(link, *options).await;
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
                let kind = self.transfers.get(&id).map(|t| t.row.kind);
                let result = match kind.map_or(Ok(()), |kind| {
                    self.config.transfer(kind, &settings).map(drop)
                }) {
                    Ok(()) => {
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
            Command::Publish(id, tracker, reply) => {
                let result = self.publish(id, tracker).await;
                let _ = reply.send(result);
            }
            Command::Unpublish(id, tracker, reply) => {
                // Answered before the announces settle, which the caller waits for.
                let result = self.unpublish(id, tracker).await;
                let _ = reply.send(result);
            }
            Command::Categories(reply) => {
                let categories = self.store.categories().unwrap_or_else(|error| {
                    tracing::error!("couldn't read the categories: {error}");
                    BTreeMap::new()
                });
                let _ = reply.send(categories);
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
            publish: BTreeSet::new(),
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
            publish: self.config.publish.iter().cloned().collect(),
        };
        let mut link = Link::new(id);
        link.name = Some(manifest.name().to_owned());
        link.trackers = options.trackers;
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

    /// Lists a transfer on `tracker` from now on: saved, and told to its running
    /// bundle's supervisor.
    async fn publish(
        &mut self,
        id: BundleId,
        tracker: String,
    ) -> Result<TransferInfo, EngineError> {
        if let Some(Transfer {
            live: Live::Running(run),
            ..
        }) = self.transfers.get(&id)
        {
            let _ = run.listing.send(ListingChange::Publish(tracker.clone()));
        }
        self.change(id, |t| {
            t.row.publish.insert(tracker);
            Ok(())
        })
        .await
    }

    /// Stops listing a transfer on `tracker`: forgotten, and told to its running
    /// bundle's supervisor, which stops announcing there unless the transfer has another reason
    /// to. The receivers resolve once no announce asking for the listing can still arrive.
    async fn unpublish(&mut self, id: BundleId, tracker: String) -> Result<Unpublished, EngineError> {
        let t = self.transfers.get(&id).ok_or(EngineError::NotFound(id))?;
        let same = |url: &String| url.trim_end_matches('/') == tracker.trim_end_matches('/');
        let published: Vec<String> = t.row.publish.iter().filter(|u| same(u)).cloned().collect();
        let mut settled = Vec::new();
        if let Live::Running(run) | Live::Stopping(run) = &t.live {
            let keep = t
                .link
                .trackers
                .iter()
                .chain(&self.config.trackers)
                .any(same);
            for tracker in &published {
                let (done, waiting) = oneshot::channel();
                let change = ListingChange::Unpublish {
                    tracker: tracker.clone(),
                    keep,
                    settled: done,
                };
                // A supervisor that is gone announces nothing more.
                if run.listing.send(change).is_ok() {
                    settled.push(waiting);
                }
            }
        }
        let info = self
            .change(id, |t| {
                for tracker in &published {
                    t.row.publish.remove(tracker);
                }
                Ok(())
            })
            .await?;
        Ok(Unpublished { info, settled })
    }

    /// A share's link: the bundle, this peer and its addresses, and its trackers, those it was
    /// shared with and then the config's.
    fn share_link(&self, link: &Link) -> Link {
        let mut link = link.clone();
        link.peer = Some(self.peer_id);
        link.addrs = self
            .node
            .endpoint()
            .port_mapping()
            .advertise(&self.addresses);
        link.relay = self.node.endpoint().relay_address();
        for tracker in &self.config.trackers {
            if !link.trackers.contains(tracker) {
                link.trackers.push(tracker.clone());
            }
        }
        link
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
        match load_checkpoint(&self.store, &id) {
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
            published: row.publish.clone(),
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
                    save_checkpoint(&mut self.store, &id, &checkpoint);
                    if !save_row(&mut self.store, &mut t.row) {
                        self.reschedule().await;
                    }
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
                queue_changed |= !save_row(&mut self.store, &mut t.row);
            }
            t.row.have = u64::from(status.have);
            t.row.uploaded = run.base_uploaded + status.uploaded;
            t.row.downloaded = run.base_downloaded + status.downloaded;
            if status.complete && t.row.completed.is_none() {
                t.row.completed = Some(now_millis());
                save_row(&mut self.store, &mut t.row);
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
                    true => queue_changed |= !save_row(&mut self.store, &mut t.row),
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
