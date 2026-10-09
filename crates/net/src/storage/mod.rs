//! A bundle's chunks on disk (D-086).
//!
//! A [`Storage`] is a handle to one bundle's storage task, which owns the bundle's open files, the
//! known parts of each file's chunk tree and the set of chunks present. Reads, writes and syncs
//! run on tokio's blocking pool, at most [`StorageConfig::parallel_jobs`] at a time, with
//! positioned I/O, so the runtime is never blocked and no file position is shared. Commands queue
//! in a bounded channel (D-085).
//!
//! - A shared bundle's files stay where they are and are only read.
//! - A download is laid out in a destination folder before its first chunk arrives, if the
//!   bundle is within [`StorageConfig::max_bundle_size`] and the folder's file system has room
//!   for it. Writes are verified first, and synced to disk in batches, then once more when the
//!   last chunk arrives. A failed sync stops the storage for good.
//! - Files are opened as they are used, at most [`StorageConfig::open_files`] at a time.
//! - Dropping an opening's future stops its work, such as hashing files, soon after.

mod checkpoint;
mod files;
mod handles;
mod tree;

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tempest_core::ChunkSet;
use tempest_protocol::message::Data;
use tempest_protocol::tree::{InvalidRequest, TreeNode, VerifyError, data_len, verify_data_leaves};
use tempest_protocol::wire::{Decode, Encode};
use tempest_protocol::{BundleId, ChunkLayout, ChunkRange, Hash, Manifest, RangeList};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;
use tokio::time::{Instant, sleep_until};

use checkpoint::FileState;
pub use checkpoint::{Checkpoint, CheckpointError};
pub use files::find_entry;
use files::{Cancel, Opened};
use handles::OpenFiles;
use tree::FileTree;

/// The largest bundle storage takes by default: 1 TiB. Memory grows with a bundle's chunk count,
/// and a manifest can declare any size.
pub const MAX_BUNDLE_SIZE: u64 = 1 << 40;

/// How a bundle's storage behaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageConfig {
    /// The most reads, writes and syncs in progress at once. At least 1.
    pub parallel_jobs: usize,
    /// How many commands may wait for the storage task. At least 1.
    pub queue: usize,
    /// Sync once this many bytes have been written since the last sync.
    pub sync_bytes: u64,
    /// Sync once this long has passed since the first write after the last sync.
    pub sync_interval: Duration,
    /// The most files kept open at once, while no more are in use. At least 1.
    pub open_files: usize,
    /// The largest bundle, in bytes, to store or share.
    pub max_bundle_size: u64,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            parallel_jobs: 4,
            queue: 64,
            sync_bytes: 64 * 1024 * 1024,
            sync_interval: Duration::from_secs(5),
            open_files: 64,
            max_bundle_size: MAX_BUNDLE_SIZE,
        }
    }
}

/// Why a storage operation failed.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// A file system operation failed.
    #[error("{}: {source}", path.display())]
    Io {
        /// The file or directory.
        path: PathBuf,
        /// What went wrong.
        source: io::Error,
    },
    /// Something on disk isn't what the manifest says should be there.
    #[error("{} {problem}", path.display())]
    Unusable {
        /// The file or directory.
        path: PathBuf,
        /// What is wrong with it.
        problem: &'static str,
    },
    /// A saved manifest doesn't decode, or doesn't match the bundle ID.
    #[error("{}: not this bundle's manifest", path.display())]
    WrongManifest {
        /// The manifest file.
        path: PathBuf,
    },
    /// The range is not a valid Request for this bundle.
    #[error("invalid request: {0}")]
    InvalidRequest(#[from] InvalidRequest),
    /// Some of the requested chunks are not stored yet.
    #[error("chunks {start}..{end} are not all stored")]
    Missing {
        /// The first chunk of the range.
        start: u32,
        /// The bundle index just past the range's last existing chunk.
        end: u32,
    },
    /// Data failed verification, so it was not stored.
    #[error("data failed verification: {0}")]
    Verify(#[from] VerifyError),
    /// The bundle is shared from files that are only read.
    #[error("a shared bundle is read-only")]
    ReadOnly,
    /// The bundle is larger than [`StorageConfig::max_bundle_size`].
    #[error("the bundle is {size} bytes, more than the {max} this client takes")]
    TooLarge {
        /// The bundle's size.
        size: u64,
        /// The most allowed.
        max: u64,
    },
    /// The destination's file system lacks room for the download.
    #[error(
        "not enough disk space in {}: the download needs {needed} more bytes, and {available} are free",
        path.display()
    )]
    NoSpace {
        /// The destination folder.
        path: PathBuf,
        /// The bytes still to write.
        needed: u64,
        /// The bytes free.
        available: u64,
    },
    /// Syncing written data to disk failed, so what was written since the last sync may be
    /// lost. Storage takes no more writes.
    #[error("{}: syncing to disk failed: {source}", path.display())]
    SyncFailed {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        source: Arc<io::Error>,
    },
    /// Opening was stopped before it finished.
    #[error("opening storage was cancelled")]
    Cancelled,
    /// The storage task has stopped.
    #[error("storage has stopped")]
    Stopped,
}

/// The outcome of a successful [`Storage::write`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Written {
    /// Every chunk is now stored and synced to disk.
    pub complete: bool,
}

/// Counts of the storage task's disk operations.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StorageStats {
    /// Data responses read from disk.
    pub reads: u64,
    /// Data written to disk. Data for chunks already stored is verified but not written.
    pub writes: u64,
    /// Files synced to disk, counting each file of each sync.
    pub syncs: u64,
}

/// A handle to one bundle's storage task. Cloning it gives another handle to the same task.
///
/// The task stops once every handle is dropped, after syncing what it has written, or when
/// [`close`](Self::close) is called.
#[derive(Debug, Clone)]
pub struct Storage {
    commands: mpsc::Sender<Command>,
    manifest: Arc<Manifest>,
    manifest_bytes: Arc<[u8]>,
}

impl Storage {
    /// Shares the bundle described by `manifest` from the files under `base`, where each entry's
    /// path is relative to `base`. Every file is hashed, and must match the manifest.
    pub async fn open_shared(
        base: impl Into<PathBuf>,
        manifest: Arc<Manifest>,
        config: StorageConfig,
    ) -> Result<Self, StorageError> {
        let base = base.into();
        Self::start(manifest, config, false, move |manifest, layout, cancel| {
            files::open_shared(&base, manifest, layout, cancel)
        })
        .await
    }

    /// Prepares to download the bundle described by `manifest` into `dir`, where each entry's
    /// path will be relative to `dir`. Returns the storage and the chunks already present: those
    /// of empty files, and of files already in `dir` whose contents match the manifest.
    ///
    /// Directories, files at their full size, and symlinks are created at once. Nothing already
    /// in `dir` is followed or replaced, except files of the bundle, which are resized and
    /// downloaded again unless their contents already match.
    pub async fn create_download(
        dir: impl Into<PathBuf>,
        manifest: Arc<Manifest>,
        config: StorageConfig,
    ) -> Result<(Self, RangeList), StorageError> {
        let dir = dir.into();
        let storage = Self::start(manifest, config, true, move |manifest, layout, cancel| {
            files::create_download(&dir, manifest, layout, cancel)
        })
        .await?;
        let have = storage.have().await?;
        Ok((storage, have))
    }

    /// Shares a bundle as [`open_shared`](Self::open_shared) does, hashing only the files that
    /// changed since `checkpoint` was taken.
    pub async fn resume_shared(
        base: impl Into<PathBuf>,
        manifest: Arc<Manifest>,
        config: StorageConfig,
        checkpoint: Checkpoint,
    ) -> Result<Self, StorageError> {
        let base = base.into();
        Self::start(manifest, config, false, move |manifest, layout, cancel| {
            files::resume_shared(&base, manifest, layout, &checkpoint.files, cancel)
        })
        .await
    }

    /// Prepares to go on with a download as [`create_download`](Self::create_download) does,
    /// starting from the chunks `checkpoint` recorded. A file's recorded chunks are trusted while
    /// its size and modification time are unchanged since then, and checked against their
    /// recorded hashes otherwise, so only changed files are read.
    pub async fn resume_download(
        dir: impl Into<PathBuf>,
        manifest: Arc<Manifest>,
        config: StorageConfig,
        checkpoint: Checkpoint,
    ) -> Result<(Self, RangeList), StorageError> {
        let dir = dir.into();
        let storage = Self::start(manifest, config, true, move |manifest, layout, cancel| {
            files::resume_download(&dir, manifest, layout, &checkpoint.files, cancel)
        })
        .await?;
        let have = storage.have().await?;
        Ok((storage, have))
    }

    /// Opens the files on the blocking pool and starts the task. Dropping the future cancels
    /// the opening.
    async fn start(
        manifest: Arc<Manifest>,
        config: StorageConfig,
        writable: bool,
        open: impl FnOnce(&Manifest, &ChunkLayout, &Cancel) -> Result<Vec<Opened>, StorageError>
        + Send
        + 'static,
    ) -> Result<Self, StorageError> {
        let size = manifest.total_size();
        if size > config.max_bundle_size {
            return Err(StorageError::TooLarge {
                size,
                max: config.max_bundle_size,
            });
        }
        let manifest_bytes: Arc<[u8]> = manifest.to_bytes().into();
        let task_manifest = manifest.clone();
        let cancel = CancelOnDrop(Cancel::default());
        let job_cancel = cancel.0.clone();
        let started = std::time::Instant::now();
        tracing::debug!("opening storage: {size} bytes, writable {writable}");
        let (layout, opened) = tokio::task::spawn_blocking(move || {
            let layout = ChunkLayout::new(&task_manifest);
            open(&task_manifest, &layout, &job_cancel).map(|opened| (layout, opened))
        })
        .await
        .expect("opening storage panicked")
        .inspect_err(|error| {
            tracing::warn!(
                "opening storage failed after {:?}: {error}",
                started.elapsed()
            );
        })?;
        drop(cancel);
        tracing::debug!(
            "opened storage in {:?}: {} files, {} chunks",
            started.elapsed(),
            opened.len(),
            layout.chunk_count()
        );

        let mut have = ChunkSet::new(layout.chunk_count());
        let mut files = Vec::with_capacity(opened.len());
        let mut trees = Vec::with_capacity(opened.len());
        for (opened, span) in opened.into_iter().zip(layout.files()) {
            for range in opened.have.ranges() {
                have.insert_range(ChunkRange {
                    start: span.first_chunk + range.start,
                    end: span.first_chunk + range.end,
                });
            }
            files.push(opened.file);
            trees.push(opened.tree);
        }

        let (commands, receiver) = mpsc::channel(config.queue.max(1));
        let task = Task {
            files: Arc::new(OpenFiles::new(files, config.open_files)),
            layout,
            trees,
            finished: have.is_full(),
            have,
            writable,
            parallel_jobs: config.parallel_jobs.max(1),
            config,
            jobs: JoinSet::new(),
            unsynced: Unsynced::default(),
            completing: None,
            stats: StorageStats::default(),
        };
        tokio::spawn(task.run(receiver));
        Ok(Self {
            commands,
            manifest,
            manifest_bytes,
        })
    }

    /// The bundle's manifest.
    pub fn manifest(&self) -> &Arc<Manifest> {
        &self.manifest
    }

    /// The manifest's encoding, for answering ManifestRequests.
    pub fn manifest_bytes(&self) -> &Arc<[u8]> {
        &self.manifest_bytes
    }

    /// Builds the Data answering a Request for the chunks `[start, start + count)`: the chunks
    /// that exist and their proof. Fails with [`StorageError::Missing`] unless every one of them
    /// is stored.
    pub async fn read(&self, start: u32, count: u32) -> Result<Data, StorageError> {
        self.call(|reply| Command::Read {
            start,
            count,
            reply,
        })
        .await?
    }

    /// Verifies `data`, received for the Request `[start, start + count)`, against the manifest
    /// and, if it passes, writes its chunks. Fails with [`StorageError::Verify`] if it doesn't.
    ///
    /// Returns once the chunks are written, but not necessarily synced; the write that completes
    /// the bundle returns once everything is synced.
    pub async fn write(&self, start: u32, count: u32, data: Data) -> Result<Written, StorageError> {
        self.call(|reply| Command::Write {
            start,
            count,
            data,
            reply,
        })
        .await?
    }

    /// The chunks stored so far.
    pub async fn have(&self) -> Result<RangeList, StorageError> {
        self.call(|reply| Command::Have { reply }).await
    }

    /// Waits for the operations in progress, then syncs everything written to disk.
    pub async fn flush(&self) -> Result<(), StorageError> {
        self.call(|reply| Command::Flush { reply }).await?
    }

    /// Counts of the disk operations so far.
    pub async fn stats(&self) -> Result<StorageStats, StorageError> {
        self.call(|reply| Command::Stats { reply }).await
    }

    /// Waits for the operations in progress, syncs everything written, and returns what is
    /// stored, to be saved for [`resume_download`](Self::resume_download) or
    /// [`resume_shared`](Self::resume_shared).
    pub async fn checkpoint(&self) -> Result<Checkpoint, StorageError> {
        self.call(|reply| Command::Checkpoint { reply }).await?
    }

    /// Flushes and stops the storage task, returning its final checkpoint. Other handles to it
    /// fail with [`StorageError::Stopped`] from then on.
    pub async fn close(self) -> Result<Checkpoint, StorageError> {
        self.call(|reply| Command::Close { reply }).await?
    }

    async fn call<T>(
        &self,
        command: impl FnOnce(oneshot::Sender<T>) -> Command,
    ) -> Result<T, StorageError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(command(reply))
            .await
            .map_err(|_| StorageError::Stopped)?;
        response.await.map_err(|_| StorageError::Stopped)
    }
}

/// Saves an encoded manifest to `path`, replacing any file there only once the new one is
/// complete and synced. This blocks, so call it from the blocking pool.
pub fn save_manifest(path: &Path, bytes: &[u8]) -> Result<(), StorageError> {
    let io_error = |source| StorageError::Io {
        path: path.to_owned(),
        source,
    };
    // A name of its own, so saves of the same manifest at once don't write into one file.
    static SAVES: AtomicU64 = AtomicU64::new(0);
    let mut temporary = path.as_os_str().to_owned();
    let save = SAVES.fetch_add(1, Ordering::Relaxed);
    temporary.push(format!(".{}-{save}.partial", std::process::id()));
    let temporary = PathBuf::from(temporary);
    let write = || -> io::Result<()> {
        let mut file = std::fs::File::create_new(&temporary)?;
        io::Write::write_all(&mut file, bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        // The rename itself lasts only once its directory is synced.
        #[cfg(unix)]
        if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            std::fs::File::open(dir)?.sync_all()?;
        }
        Ok(())
    };
    write().map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        io_error(error)
    })
}

/// Loads the manifest saved at `path`, checking that it is the manifest of `bundle_id`. This
/// blocks, so call it from the blocking pool.
pub fn load_manifest(path: &Path, bundle_id: &BundleId) -> Result<Manifest, StorageError> {
    let bytes = std::fs::read(path).map_err(|source| StorageError::Io {
        path: path.to_owned(),
        source,
    })?;
    let wrong = || StorageError::WrongManifest {
        path: path.to_owned(),
    };
    if tempest_protocol::hash::manifest_hash(&bytes) != *bundle_id {
        return Err(wrong());
    }
    Manifest::from_bytes(&bytes).map_err(|_| wrong())
}

enum Command {
    Read {
        start: u32,
        count: u32,
        reply: oneshot::Sender<Result<Data, StorageError>>,
    },
    Write {
        start: u32,
        count: u32,
        data: Data,
        reply: oneshot::Sender<Result<Written, StorageError>>,
    },
    Have {
        reply: oneshot::Sender<RangeList>,
    },
    Flush {
        reply: oneshot::Sender<Result<(), StorageError>>,
    },
    Stats {
        reply: oneshot::Sender<StorageStats>,
    },
    Checkpoint {
        reply: oneshot::Sender<Result<Checkpoint, StorageError>>,
    },
    Close {
        reply: oneshot::Sender<Result<Checkpoint, StorageError>>,
    },
}

/// A finished blocking job, for the task to account for.
enum Done {
    /// A read, already answered.
    Read,
    /// A write: on success, what the tree learns from it. `stored` is whether the chunks were
    /// written, or only verified because they were already present.
    Write {
        file: usize,
        node: TreeNode,
        span: ChunkRange,
        stored: bool,
        result: Result<Learned, StorageError>,
        reply: oneshot::Sender<Result<Written, StorageError>>,
    },
    /// A batched sync.
    Sync,
}

/// What a verified Data adds to a file's chunk tree.
struct Learned {
    leaves: Vec<Hash>,
    proof: Vec<Hash>,
}

/// Writes not yet synced to disk.
#[derive(Debug, Default)]
struct Unsynced {
    files: BTreeSet<usize>,
    bytes: u64,
    since: Option<Instant>,
}

/// Cancels an opening when dropped, as when its future is.
struct CancelOnDrop(Cancel);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// The storage task's state.
struct Task {
    files: Arc<OpenFiles>,
    layout: ChunkLayout,
    trees: Vec<FileTree>,
    have: ChunkSet,
    writable: bool,
    config: StorageConfig,
    parallel_jobs: usize,
    jobs: JoinSet<Done>,
    unsynced: Unsynced,
    /// The write that stored the last missing chunk, answered once everything is synced.
    completing: Option<oneshot::Sender<Result<Written, StorageError>>>,
    /// Every chunk is present and has been synced, or was present from the start.
    finished: bool,
    stats: StorageStats,
}

impl Task {
    async fn run(mut self, mut commands: mpsc::Receiver<Command>) {
        loop {
            let sync_at = self.sync_deadline();
            tokio::select! {
                command = commands.recv(), if self.jobs.len() < self.parallel_jobs => {
                    match command {
                        Some(Command::Close { reply }) => {
                            let result = self.checkpoint().await;
                            let _ = reply.send(result);
                            return;
                        }
                        Some(command) => self.handle(command).await,
                        None => break,
                    }
                }
                Some(done) = self.jobs.join_next() => {
                    self.account(done.expect("a storage job panicked"));
                }
                () = sleep_until(sync_at.unwrap_or_else(Instant::now)), if sync_at.is_some() => {
                    self.start_sync();
                }
            }
            self.settle().await;
        }
        // Every handle is gone: keep what was written.
        let _ = self.flush().await;
        self.settle().await;
    }

    async fn handle(&mut self, command: Command) {
        match command {
            Command::Read {
                start,
                count,
                reply,
            } => match self.prepare_read(start, count) {
                Ok((file, position, proof)) => {
                    let files = self.files.clone();
                    self.stats.reads += 1;
                    self.jobs.spawn_blocking(move || {
                        let info = &files.files()[file].info;
                        let result = files
                            .read(file, |handle| {
                                files::read_chunks(handle, info, position, count)
                            })
                            .map(|chunks| Data { proof, chunks });
                        let _ = reply.send(result);
                        Done::Read
                    });
                }
                Err(error) => {
                    let _ = reply.send(Err(error));
                }
            },
            Command::Write {
                start,
                count,
                data,
                reply,
            } => self.start_write(start, count, data, reply),
            Command::Have { reply } => {
                let _ = reply.send(self.have.to_ranges());
            }
            Command::Flush { reply } => {
                let _ = reply.send(self.flush().await);
            }
            Command::Stats { reply } => {
                let _ = reply.send(self.stats);
            }
            Command::Checkpoint { reply } => {
                let _ = reply.send(self.checkpoint().await);
            }
            Command::Close { .. } => unreachable!("handled by run"),
        }
    }

    /// The file, the range's position in it and the proof for a Read, if every chunk is present.
    fn prepare_read(
        &self,
        start: u32,
        count: u32,
    ) -> Result<(usize, u32, Vec<Hash>), StorageError> {
        let (file, node) = self.layout.request_node(start, count)?;
        let span = self.span(file, node);
        let missing = StorageError::Missing {
            start: span.start,
            end: span.end,
        };
        if !self.have.contains_range(span) {
            return Err(missing);
        }
        let proof = self.trees[file].proof(node).ok_or(missing)?;
        Ok((file, position(node), proof))
    }

    /// The bundle chunks a Request for `node` of `file` covers: the node's chunks that exist.
    fn span(&self, file: usize, node: TreeNode) -> ChunkRange {
        let chunks = self.layout.files()[file];
        let start = chunks.first_chunk + position(node);
        let end = (u64::from(start) + (1u64 << node.level)).min(u64::from(chunks.end()));
        ChunkRange {
            start,
            end: end as u32,
        }
    }

    fn start_write(
        &mut self,
        start: u32,
        count: u32,
        data: Data,
        reply: oneshot::Sender<Result<Written, StorageError>>,
    ) {
        if !self.writable {
            let _ = reply.send(Err(StorageError::ReadOnly));
            return;
        }
        let (file, node) = match self.layout.request_node(start, count) {
            Ok(found) => found,
            Err(error) => {
                let _ = reply.send(Err(error.into()));
                return;
            }
        };
        let span = self.span(file, node);
        // Chunks already stored are only verified, so a finished file is never touched again.
        let store = !self.have.contains_range(span);
        let files = self.files.clone();
        self.jobs.spawn_blocking(move || {
            let result = verify_and_write(&files, file, position(node), count, data, store);
            Done::Write {
                file,
                node,
                span,
                stored: store,
                result,
                reply,
            }
        });
    }

    /// Takes account of a finished job.
    fn account(&mut self, done: Done) {
        match done {
            Done::Read => {}
            Done::Write {
                file,
                node,
                span,
                stored,
                result,
                reply,
            } => {
                let learned = match result {
                    Ok(learned) => learned,
                    Err(error) => {
                        let _ = reply.send(Err(error));
                        return;
                    }
                };
                self.trees[file].insert(node, &learned.leaves, &learned.proof);
                if stored {
                    self.stats.writes += 1;
                    self.have.insert_range(span);
                    self.unsynced.files.insert(file);
                    let size = self.files.files()[file].info.size;
                    self.unsynced.bytes += data_len(size, position(node), 1 << node.level);
                    self.unsynced.since.get_or_insert_with(Instant::now);
                    if self.unsynced.bytes >= self.config.sync_bytes {
                        self.start_sync();
                    }
                }
                // Only the first write to complete the bundle waits for the final sync; a
                // duplicate of the last chunk finishing alongside it adds nothing.
                if !self.finished && self.have.is_full() && self.completing.is_none() {
                    self.completing = Some(reply);
                } else {
                    let _ = reply.send(Ok(Written { complete: false }));
                }
            }
            // A failure is final, and the next flush reports it.
            Done::Sync => {}
        }
    }

    /// When the next batched sync is due, if one is waiting.
    fn sync_deadline(&self) -> Option<Instant> {
        self.unsynced
            .since
            .map(|since| since + self.config.sync_interval)
    }

    /// Starts syncing every file written since the last sync.
    fn start_sync(&mut self) {
        let unsynced = std::mem::take(&mut self.unsynced);
        if unsynced.files.is_empty() {
            return;
        }
        self.stats.syncs += unsynced.files.len() as u64;
        let files = self.files.clone();
        self.jobs.spawn_blocking(move || {
            if let Err(error) = files.sync(unsynced.files) {
                tracing::warn!("cannot sync downloaded files to disk: {error}");
            }
            Done::Sync
        });
    }

    /// Waits for every job, then syncs everything written.
    async fn flush(&mut self) -> Result<(), StorageError> {
        while let Some(done) = self.jobs.join_next().await {
            self.account(done.expect("a storage job panicked"));
        }
        let unsynced = std::mem::take(&mut self.unsynced);
        self.stats.syncs += unsynced.files.len() as u64;
        let files = self.files.clone();
        tokio::task::spawn_blocking(move || files.sync(unsynced.files))
            .await
            .expect("syncing panicked")?;
        // A periodic sync may have failed.
        match self.files.failure() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Flushes, then records what is stored and synced, with each file's size and modification
    /// time as of now.
    async fn checkpoint(&mut self) -> Result<Checkpoint, StorageError> {
        self.flush().await?;
        // Answering the write that completed the bundle restores modification times, which the
        // checkpoint must see.
        self.settle().await;
        let have = self.have.to_ranges();
        let files = self.files.files();
        let mut states = Vec::with_capacity(files.len());
        for ((file, tree), span) in files.iter().zip(&self.trees).zip(self.layout.files()) {
            let metadata =
                std::fs::symlink_metadata(&file.path).map_err(|source| StorageError::Io {
                    path: file.path.clone(),
                    source,
                })?;
            let span = ChunkRange {
                start: span.first_chunk,
                end: span.end(),
            };
            states.push(FileState {
                size: metadata.len(),
                modified: checkpoint::modified(&metadata),
                have: checkpoint::file_ranges(&have, span),
                nodes: tree.known_nodes(),
            });
        }
        Ok(Checkpoint { files: states })
    }

    /// If the last chunk has been written, syncs everything, restores the files' modification
    /// times and answers the write that completed the bundle.
    async fn settle(&mut self) {
        let Some(reply) = self.completing.take() else {
            return;
        };
        self.finished = true;
        let mut result = self.flush().await;
        if result.is_ok() {
            let files = self.files.clone();
            result = tokio::task::spawn_blocking(move || {
                (0..files.files().len()).try_for_each(|i| {
                    let modified = files.files()[i].info.modified;
                    files.read(i, |file| files::restore_modified(file, modified))
                })
            })
            .await
            .expect("restoring modification times panicked");
        }
        let _ = reply.send(result.map(|()| Written { complete: true }));
    }
}

/// A node's first chunk position within its file.
fn position(node: TreeNode) -> u32 {
    // A valid Request's node starts within the file, so this fits.
    (node.index << node.level) as u32
}

/// Verifies `data` against file `file`'s manifest entry and, if `store`, writes its chunks.
fn verify_and_write(
    files: &OpenFiles,
    file: usize,
    position: u32,
    count: u32,
    data: Data,
    store: bool,
) -> Result<Learned, StorageError> {
    let info = &files.files()[file].info;
    let leaves = verify_data_leaves(
        info.size,
        &info.root,
        position,
        count,
        &data.proof,
        &data.chunks,
    )?;
    // Verification passed, so the chunks lie within the file.
    if store {
        files.write(file, |handle| {
            files::write_chunks(handle, position, &data.chunks)
        })?;
    }
    Ok(Learned {
        leaves,
        proof: data.proof,
    })
}
