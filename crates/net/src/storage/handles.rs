//! A bundle's files and a bounded set of open handles to them.
//!
//! A bundle may hold more files than the process may open, so files are opened when used and
//! the least recently used handle is closed once [`StorageConfig::open_files`] are open. A
//! handle in use by a job is never closed. A handle with writes not yet synced is synced before
//! it closes, since a sync through another handle later might not report a failure that this
//! one would.
//!
//! A failed sync means data the page cache held may never reach the disk, and syncing again
//! would succeed without bringing it back. So the first failure is final: every later write or
//! sync reports it.
//!
//! [`StorageConfig::open_files`]: super::StorageConfig::open_files

use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use super::StorageError;
use super::files::BundleFile;

/// A bundle's files, opened as they are used.
#[derive(Debug)]
pub(super) struct OpenFiles {
    files: Box<[BundleFile]>,
    capacity: usize,
    cache: Mutex<Cache>,
}

#[derive(Debug, Default)]
struct Cache {
    open: HashMap<usize, Handle>,
    /// Counts uses, to find the least recently used handle.
    clock: u64,
    /// The first failed sync.
    failure: Option<SyncFailure>,
}

#[derive(Debug)]
struct Handle {
    file: Arc<File>,
    last_used: u64,
    /// Written since its last sync.
    dirty: bool,
}

/// A failed sync, kept to report again.
#[derive(Debug, Clone)]
struct SyncFailure {
    path: PathBuf,
    source: Arc<io::Error>,
}

impl SyncFailure {
    fn error(&self) -> StorageError {
        StorageError::SyncFailed {
            path: self.path.clone(),
            source: self.source.clone(),
        }
    }
}

impl OpenFiles {
    /// The files, with at most `capacity` handles open at once while none is in use.
    pub fn new(files: Vec<BundleFile>, capacity: usize) -> Self {
        Self {
            files: files.into(),
            capacity: capacity.max(1),
            cache: Mutex::default(),
        }
    }

    /// The bundle's files, in chunk order.
    pub fn files(&self) -> &[BundleFile] {
        &self.files
    }

    /// How many handles are open.
    #[cfg(test)]
    fn open_count(&self) -> usize {
        self.lock().open.len()
    }

    /// Runs `read` with a handle to file `index`.
    pub fn read<T>(
        &self,
        index: usize,
        read: impl FnOnce(&File) -> io::Result<T>,
    ) -> Result<T, StorageError> {
        let file = self.acquire(index)?;
        read(&file).map_err(|source| self.io_error(index, source))
    }

    /// Runs `write` with a handle to file `index`, which then needs a sync. Fails once a sync
    /// has failed.
    pub fn write<T>(
        &self,
        index: usize,
        write: impl FnOnce(&File) -> io::Result<T>,
    ) -> Result<T, StorageError> {
        if let Some(failure) = &self.lock().failure {
            return Err(failure.error());
        }
        let file = self.acquire(index)?;
        let result = write(&file);
        // Even a failed write may have changed the file.
        if let Some(handle) = self.lock().open.get_mut(&index) {
            handle.dirty = true;
        }
        drop(file);
        result.map_err(|source| self.io_error(index, source))
    }

    /// Syncs the data written to each of `which`. A file whose handle has closed was synced as
    /// it closed.
    pub fn sync(&self, which: impl IntoIterator<Item = usize>) -> Result<(), StorageError> {
        for index in which {
            let file = {
                let mut cache = self.lock();
                if let Some(failure) = &cache.failure {
                    return Err(failure.error());
                }
                match cache.open.get_mut(&index) {
                    Some(handle) if handle.dirty => {
                        handle.dirty = false;
                        handle.file.clone()
                    }
                    _ => continue,
                }
            };
            // Holding the handle keeps it open, so writes after this sync mark it again.
            if let Err(source) = file.sync_data() {
                return Err(self.fail(&mut self.lock(), index, source));
            }
        }
        Ok(())
    }

    /// The failed sync that stopped the storage, if one has.
    pub fn failure(&self) -> Option<StorageError> {
        self.lock().failure.as_ref().map(SyncFailure::error)
    }

    fn lock(&self) -> MutexGuard<'_, Cache> {
        self.cache.lock().expect("never poisoned")
    }

    fn io_error(&self, index: usize, source: io::Error) -> StorageError {
        StorageError::Io {
            path: self.files[index].path.clone(),
            source,
        }
    }

    /// Records a failed sync of file `index`, the first one being final, and returns the error.
    fn fail(&self, cache: &mut Cache, index: usize, source: io::Error) -> StorageError {
        let failure = cache.failure.get_or_insert_with(|| {
            let path = self.files[index].path.clone();
            tracing::error!("syncing {} failed, so storage stops: {source}", path.display());
            SyncFailure {
                path,
                source: Arc::new(source),
            }
        });
        failure.error()
    }

    /// A handle to file `index`, opening it and closing the least recently used idle handle if
    /// too many are open.
    fn acquire(&self, index: usize) -> Result<Arc<File>, StorageError> {
        let mut cache = self.lock();
        cache.clock += 1;
        let now = cache.clock;
        if let Some(handle) = cache.open.get_mut(&index) {
            handle.last_used = now;
            return Ok(handle.file.clone());
        }
        if cache.open.len() >= self.capacity {
            self.close_least_recent(&mut cache)?;
        }
        let file = Arc::new(self.files[index].reopen()?);
        cache.open.insert(
            index,
            Handle {
                file: file.clone(),
                last_used: now,
                dirty: false,
            },
        );
        Ok(file)
    }

    /// Closes the least recently used handle no job holds, syncing it first if it was written.
    /// Syncing holds the lock, which keeps the file from being opened again meanwhile.
    fn close_least_recent(&self, cache: &mut Cache) -> Result<(), StorageError> {
        let idle = (cache.open.iter())
            .filter(|(_, handle)| Arc::strong_count(&handle.file) == 1)
            .min_by_key(|(_, handle)| handle.last_used)
            .map(|(&index, _)| index);
        // Every handle is in use: go over the limit for now.
        let Some(index) = idle else {
            return Ok(());
        };
        let handle = cache.open.remove(&index).expect("found above");
        if handle.dirty
            && let Err(source) = handle.file.sync_data()
        {
            return Err(self.fail(cache, index, source));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tempest_protocol::{FileInfo, Hash};

    use super::*;

    fn files(dir: &std::path::Path, count: usize) -> OpenFiles {
        let files = (0..count)
            .map(|i| {
                let path = dir.join(format!("{i}"));
                let file = File::create_new(&path).unwrap();
                let info = FileInfo {
                    size: 4,
                    root: Hash::ZERO,
                    executable: false,
                    modified: 0,
                };
                BundleFile::new(path, info, &file, true).unwrap()
            })
            .collect();
        OpenFiles::new(files, 2)
    }

    fn write(files: &OpenFiles, index: usize, bytes: &[u8]) {
        files
            .write(index, |file| super::super::files::write_all_at(file, bytes, 0))
            .unwrap();
    }

    #[test]
    fn handles_are_bounded_and_written_data_survives_closing() {
        let dir = tempfile::tempdir().unwrap();
        let files = files(dir.path(), 5);
        for i in 0..5 {
            write(&files, i, &[i as u8; 4]);
            assert!(files.open_count() <= 2);
        }
        files.sync(0..5).unwrap();
        for i in 0..5 {
            assert_eq!(std::fs::read(dir.path().join(format!("{i}"))).unwrap(), [i as u8; 4]);
        }
    }

    #[test]
    fn a_handle_in_use_stays_open() {
        let dir = tempfile::tempdir().unwrap();
        let files = files(dir.path(), 3);
        let held = files.acquire(0).unwrap();
        files.acquire(1).unwrap();
        files.acquire(2).unwrap();
        // File 0 is the least recently used, but held, so file 1 went instead.
        let cache = files.lock();
        assert!(cache.open.contains_key(&0) && cache.open.contains_key(&2));
        assert!(Arc::ptr_eq(&cache.open[&0].file, &held));
    }

    #[cfg(unix)]
    #[test]
    fn a_file_put_in_the_place_of_another_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let files = files(dir.path(), 3);
        write(&files, 0, b"abcd");
        // Using two other files closes file 0's handle.
        write(&files, 1, b"abcd");
        write(&files, 2, b"abcd");
        let path = dir.path().join("0");
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"else").unwrap();
        let error = files.read(0, |_| Ok(())).unwrap_err();
        assert!(matches!(error, StorageError::Unusable { .. }), "{error}");
    }

    #[test]
    fn a_failed_sync_is_final() {
        let dir = tempfile::tempdir().unwrap();
        let files = files(dir.path(), 1);
        write(&files, 0, b"abcd");
        let error = io::Error::other("injected");
        let reported = files.fail(&mut files.lock(), 0, error);
        assert!(matches!(reported, StorageError::SyncFailed { .. }));
        assert!(matches!(
            files.write(0, |_| Ok(())),
            Err(StorageError::SyncFailed { .. })
        ));
        assert!(matches!(files.sync([0]), Err(StorageError::SyncFailed { .. })));
        assert!(files.failure().is_some());
        // Reads still work: what was synced before is intact.
        files.read(0, |_| Ok(())).unwrap();
    }
}
