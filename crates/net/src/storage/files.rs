//! A bundle's files on disk: opening a shared folder, laying out a download, and positioned reads
//! and writes. Everything here blocks, so it runs on the blocking pool.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, UNIX_EPOCH};

use tempest_protocol::hash::leaf_hash;
use tempest_protocol::path::{ancestors, parent};
use tempest_protocol::tree::{CHUNK_SIZE, chunk_count, data_len, leaf_hashes};
use tempest_protocol::{ChunkLayout, ChunkRange, EntryKind, FileInfo, Hash, Manifest, RangeList};
use unicode_normalization::UnicodeNormalization;

use super::StorageError;
use super::checkpoint::FileState;
use super::tree::FileTree;

/// How many chunks [`hash_leaves`] reads at a time.
const HASH_BATCH_CHUNKS: usize = 64;

/// One file of a bundle.
#[derive(Debug)]
pub(super) struct BundleFile {
    /// Where the file is on disk.
    pub path: PathBuf,
    /// The file's size, root and other fields from the manifest.
    pub info: FileInfo,
    /// Opened for writing too, when downloading.
    writable: bool,
    /// The file system's identity for the file, to tell it from one put in its place.
    identity: Option<(u64, u64)>,
}

impl BundleFile {
    /// The file at `path`, open as `file`.
    pub fn new(path: PathBuf, info: FileInfo, file: &File, writable: bool) -> io::Result<Self> {
        Ok(Self {
            identity: identity(&file.metadata()?),
            path,
            info,
            writable,
        })
    }

    /// Opens the file again, checking it is the same file as when it was first opened.
    pub fn reopen(&self) -> Result<File, StorageError> {
        let mut options = OpenOptions::new();
        options.read(true).write(self.writable);
        no_follow(&mut options);
        let file = options.open(&self.path).map_err(io_error(&self.path))?;
        let metadata = file.metadata().map_err(io_error(&self.path))?;
        if !metadata.is_file() || identity(&metadata) != self.identity {
            return Err(unusable(&self.path, "was replaced while in use"));
        }
        Ok(file)
    }
}

/// A file's device and inode numbers, where the platform has them.
#[cfg(unix)]
fn identity(metadata: &fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn identity(_metadata: &fs::Metadata) -> Option<(u64, u64)> {
    None
}

/// A file just opened, with what is known of its chunk tree and the chunks present, as positions
/// within the file.
#[derive(Debug)]
pub(super) struct Opened {
    pub file: BundleFile,
    pub tree: FileTree,
    pub have: RangeList,
}

/// Set to stop opening storage early: by dropping [`Storage`](super::Storage)'s opening future.
#[derive(Debug, Clone, Default)]
pub(super) struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    fn check(&self) -> Result<(), StorageError> {
        match self.0.load(Ordering::Relaxed) {
            true => Err(StorageError::Cancelled),
            false => Ok(()),
        }
    }
}

/// Every chunk of a file of `size` bytes.
fn all_chunks(size: u64) -> RangeList {
    RangeList::from_ranges(vec![ChunkRange {
        start: 0,
        end: chunk_count(size) as u32,
    }])
    .expect("one non-empty range")
}

/// Where the entry at manifest path `path` lies under `base`. The manifest's path rules make
/// every segment a plain name on every platform, so this never leaves `base`.
fn fs_path(base: &Path, path: &str) -> PathBuf {
    let mut fs_path = base.to_path_buf();
    fs_path.extend(path.split('/'));
    fs_path
}

/// Where the entry at manifest path `path` lies under `base`, for files that were there before
/// the bundle was made. Manifest paths are in Unicode's NFC form, which a file system may not
/// keep: a name that isn't found as it is is looked for among the names in its directory whose
/// NFC form matches. Returns the path as it would be if nothing matches.
pub fn find_entry(base: &Path, path: &str) -> PathBuf {
    let exact = fs_path(base, path);
    if fs::symlink_metadata(&exact).is_ok() {
        return exact;
    }
    let mut found = base.to_path_buf();
    for segment in path.split('/') {
        let as_is = found.join(segment);
        found = match fs::symlink_metadata(&as_is) {
            Ok(_) => as_is,
            Err(_) => same_name_in(&found, segment).unwrap_or(as_is),
        };
    }
    found
}

/// The one entry of `dir` whose name, in NFC form, is `name`.
fn same_name_in(dir: &Path, name: &str) -> Option<PathBuf> {
    let mut matches = fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| {
            (entry.file_name().to_str()).is_some_and(|other| other.nfc().eq(name.chars()))
        });
    let entry = matches.next()?;
    matches.next().is_none().then(|| entry.path())
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> StorageError + '_ {
    move |source| StorageError::Io {
        path: path.to_owned(),
        source,
    }
}

fn unusable(path: &Path, problem: &'static str) -> StorageError {
    StorageError::Unusable {
        path: path.to_owned(),
        problem,
    }
}

/// The files of `manifest`, in chunk order, each with its manifest fields.
fn manifest_files<'a>(
    manifest: &'a Manifest,
    layout: &'a ChunkLayout,
) -> impl Iterator<Item = (&'a str, &'a FileInfo)> {
    layout.files().iter().map(|f| {
        let entry = &manifest.entries()[f.entry];
        (
            entry.path.as_str(),
            entry.as_file().expect("layout lists files"),
        )
    })
}

/// Opens the files of a bundle shared from `base`, read-only, and hashes each to build its chunk
/// tree. Fails if any file is missing or has changed since the manifest was made.
pub(super) fn open_shared(
    base: &Path,
    manifest: &Manifest,
    layout: &ChunkLayout,
    cancel: &Cancel,
) -> Result<Vec<Opened>, StorageError> {
    resume_shared(base, manifest, layout, &[], cancel)
}

/// Opens the files of a bundle shared from `base`, as [`open_shared`] does, using `saved` to
/// avoid hashing files unchanged since it was taken.
pub(super) fn resume_shared(
    base: &Path,
    manifest: &Manifest,
    layout: &ChunkLayout,
    saved: &[FileState],
    cancel: &Cancel,
) -> Result<Vec<Opened>, StorageError> {
    manifest_files(manifest, layout)
        .enumerate()
        .map(|(i, (path, info))| {
            cancel.check()?;
            open_shared_file(find_entry(base, path), info, saved.get(i), cancel)
        })
        .collect()
}

/// Opens one file of a shared bundle, read-only. With the state a checkpoint saved, the file is
/// hashed only if its size or modification time changed since, and then checked chunk by chunk
/// against the saved leaf hashes. Otherwise it is hashed to build its tree.
fn open_shared_file(
    path: PathBuf,
    info: &FileInfo,
    saved: Option<&FileState>,
    cancel: &Cancel,
) -> Result<Opened, StorageError> {
    let metadata = fs::symlink_metadata(&path).map_err(io_error(&path))?;
    if !metadata.is_file() {
        return Err(unusable(&path, "is not a regular file"));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    no_follow(&mut options);
    let file = options.open(&path).map_err(io_error(&path))?;
    let changed = || unusable(&path, "has changed since the bundle was made");
    let metadata = file.metadata().map_err(io_error(&path))?;
    if metadata.len() != info.size {
        return Err(changed());
    }
    let all = all_chunks(info.size);
    let saved_tree = saved
        .filter(|saved| saved.have == all)
        .and_then(|saved| Some((saved, saved_tree(info, saved)?)));
    let tree = match saved_tree {
        Some((saved, tree)) => {
            let unchanged = saved.size == info.size
                && saved.modified.is_some()
                && saved.modified == super::checkpoint::modified(&metadata);
            if !unchanged && matching_chunks(&file, &path, info.size, &tree, &all, cancel)? != all
            {
                return Err(changed());
            }
            tree
        }
        None => {
            let tree = FileTree::from_leaves(&hash_leaves(&file, &path, info.size, cancel)?);
            if tree.root() != Some(info.root) {
                return Err(changed());
            }
            tree
        }
    };
    Ok(Opened {
        file: BundleFile::new(path.clone(), info.clone(), &file, false)
            .map_err(io_error(&path))?,
        tree,
        have: all,
    })
}

/// The tree a checkpoint saved for a file, if it is consistent with the file's root and knows
/// the leaf of every chunk it says is present.
fn saved_tree(info: &FileInfo, saved: &FileState) -> Option<FileTree> {
    let count = chunk_count(info.size);
    if !saved.have.fits(count as u32) {
        return None;
    }
    let tree = FileTree::from_known(count, info.root, &saved.nodes)?;
    let known = saved
        .have
        .chunks()
        .all(|position| tree.leaf(position.into()).is_some());
    known.then_some(tree)
}

/// The chunks of `candidates` whose contents on disk hash to the leaf `tree` knows for them.
fn matching_chunks(
    file: &File,
    path: &Path,
    size: u64,
    tree: &FileTree,
    candidates: &RangeList,
    cancel: &Cancel,
) -> Result<RangeList, StorageError> {
    let mut matching = Vec::new();
    let mut buffer = vec![0; CHUNK_SIZE];
    for position in candidates.chunks() {
        cancel.check()?;
        let offset = u64::from(position) * CHUNK_SIZE as u64;
        let len = (size - offset).min(CHUNK_SIZE as u64) as usize;
        read_exact_at(file, &mut buffer[..len], offset).map_err(io_error(path))?;
        if tree.leaf(position.into()) == Some(leaf_hash(&buffer[..len])) {
            matching.push(position);
        }
    }
    Ok(RangeList::from_chunks(matching))
}

/// Lays out a download of `manifest` in `dir`: creates `dir` if needed, then every directory,
/// every file at its full size, and finally every symlink. Fails first if the file system lacks
/// room for the bytes the files don't hold yet.
///
/// Nothing already in `dir` is followed: a symlink, or anything else that isn't what the manifest
/// says should be at a path, is an error. A file that already exists with the right size and
/// root is kept and counts as complete; any other existing file is resized and downloaded again.
pub(super) fn create_download(
    dir: &Path,
    manifest: &Manifest,
    layout: &ChunkLayout,
    cancel: &Cancel,
) -> Result<Vec<Opened>, StorageError> {
    let needed = manifest_files(manifest, layout)
        .map(|(path, info)| info.size - on_disk(&fs_path(dir, path)).min(info.size))
        .sum();
    check_space(dir, needed)?;
    lay_out(dir, manifest, layout, cancel, |_, path, info| {
        open_download_file(path, info, cancel)
    })
}

/// Lays out a download as [`create_download`] does, using `saved` to trust the chunks of files
/// unchanged since it was taken, and to check those of changed files without downloading them
/// again.
pub(super) fn resume_download(
    dir: &Path,
    manifest: &Manifest,
    layout: &ChunkLayout,
    saved: &[FileState],
    cancel: &Cancel,
) -> Result<Vec<Opened>, StorageError> {
    let needed = manifest_files(manifest, layout)
        .enumerate()
        .map(|(i, (_, info))| info.size - saved.get(i).map_or(0, |s| stored(info.size, &s.have)))
        .sum();
    check_space(dir, needed)?;
    lay_out(dir, manifest, layout, cancel, |i, path, info| {
        match saved
            .get(i)
            .and_then(|saved| Some((saved, saved_tree(info, saved)?)))
        {
            Some((saved, tree)) => resume_download_file(path, info, saved, tree, cancel),
            None => open_download_file(path, info, cancel),
        }
    })
}

/// The bytes of `have`'s chunks in a file of `size` bytes, counting only those within it.
fn stored(size: u64, have: &RangeList) -> u64 {
    (have.ranges().iter())
        .map(|range| data_len(size, range.start, range.end - range.start))
        .sum::<u64>()
        .min(size)
}

/// The bytes the file at `path` takes on disk, or 0 if it isn't a regular file. Downloads lay
/// files out sparse, so on Unix this counts the blocks allocated rather than the length.
fn on_disk(path: &Path) -> u64 {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => allocated(&metadata),
        _ => 0,
    }
}

#[cfg(unix)]
fn allocated(metadata: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    metadata.blocks().saturating_mul(512).min(metadata.len())
}

#[cfg(not(unix))]
fn allocated(metadata: &fs::Metadata) -> u64 {
    metadata.len()
}

/// Fails unless the file system that holds, or will hold, `dir` has `needed` bytes free.
fn check_space(dir: &Path, needed: u64) -> Result<(), StorageError> {
    if needed == 0 {
        return Ok(());
    }
    let Some(existing) = dir.ancestors().find(|path| path.exists()) else {
        return Ok(());
    };
    let available = fs4::available_space(existing).map_err(io_error(existing))?;
    match needed <= available {
        true => Ok(()),
        false => Err(StorageError::NoSpace {
            path: dir.to_owned(),
            needed,
            available,
        }),
    }
}

/// Creates `dir`, the bundle's directories, its files with `open_file` (given each file's
/// index, path and manifest fields), and finally its symlinks.
fn lay_out(
    dir: &Path,
    manifest: &Manifest,
    layout: &ChunkLayout,
    cancel: &Cancel,
    mut open_file: impl FnMut(usize, PathBuf, &FileInfo) -> Result<Opened, StorageError>,
) -> Result<Vec<Opened>, StorageError> {
    fs::create_dir_all(dir).map_err(io_error(dir))?;
    let mut dirs = Directories {
        base: dir,
        made: HashSet::new(),
    };
    for entry in manifest.entries() {
        cancel.check()?;
        match &entry.kind {
            EntryKind::Directory => dirs.ensure(&entry.path)?,
            EntryKind::File(_) => dirs.ensure(parent(&entry.path))?,
            EntryKind::Symlink { .. } => {}
        }
    }
    let opened = manifest_files(manifest, layout)
        .enumerate()
        .map(|(i, (path, info))| {
            cancel.check()?;
            open_file(i, fs_path(dir, path), info)
        })
        .collect::<Result<Vec<_>, _>>()?;
    // Symlinks come last, so nothing is ever written through one.
    for entry in manifest.entries() {
        if let EntryKind::Symlink { target } = &entry.kind {
            dirs.ensure(parent(&entry.path))?;
            create_symlink(manifest, dir, &entry.path, target)?;
        }
    }
    Ok(opened)
}

/// Creates directories under a download's top level, refusing anything in the way.
struct Directories<'a> {
    base: &'a Path,
    /// Manifest paths of the directories already checked or made.
    made: HashSet<String>,
}

impl Directories<'_> {
    /// Ensures the directory at manifest path `path` (`""` for the top level) and all its
    /// ancestors exist.
    fn ensure(&mut self, path: &str) -> Result<(), StorageError> {
        if path.is_empty() {
            return Ok(());
        }
        for dir in ancestors(path).chain([path]) {
            if self.made.contains(dir) {
                continue;
            }
            let fs_path = fs_path(self.base, dir);
            match fs::symlink_metadata(&fs_path) {
                Ok(metadata) if metadata.is_dir() => {}
                Ok(_) => return Err(unusable(&fs_path, "is in the way of a directory")),
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    fs::create_dir(&fs_path).map_err(io_error(&fs_path))?;
                }
                Err(e) => return Err(io_error(&fs_path)(e)),
            }
            self.made.insert(dir.to_owned());
        }
        Ok(())
    }
}

/// Opens one file of a download for reading and writing, creating it if it doesn't exist.
/// Returns the file and whether it existed.
fn open_writable(path: &Path, info: &FileInfo) -> Result<(File, bool), StorageError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    no_follow(&mut options);
    let existing = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => true,
        Ok(_) => return Err(unusable(path, "is in the way of a file")),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            options.create_new(true);
            false
        }
        Err(e) => return Err(io_error(path)(e)),
    };
    let file = options.open(path).map_err(io_error(path))?;
    set_executable(&file, info.executable).map_err(io_error(path))?;
    Ok((file, existing))
}

/// Opens or creates one file of a download, sized to `info.size`.
fn open_download_file(
    path: PathBuf,
    info: &FileInfo,
    cancel: &Cancel,
) -> Result<Opened, StorageError> {
    let (file, existing) = open_writable(&path, info)?;
    let len = file.metadata().map_err(io_error(&path))?.len();
    let mut tree = FileTree::new(chunk_count(info.size), info.root);
    let mut have = RangeList::new();
    if info.size == 0 {
        // An empty file is complete as soon as it exists.
        file.set_len(0).map_err(io_error(&path))?;
        tree = FileTree::from_leaves(&[leaf_hash(&[])]);
        have = all_chunks(0);
    } else if existing && len == info.size {
        let found = FileTree::from_leaves(&hash_leaves(&file, &path, info.size, cancel)?);
        if found.root() == Some(info.root) {
            tree = found;
            have = all_chunks(info.size);
        }
    } else {
        file.set_len(info.size).map_err(io_error(&path))?;
    }
    Ok(Opened {
        file: BundleFile::new(path.clone(), info.clone(), &file, true).map_err(io_error(&path))?,
        tree,
        have,
    })
}

/// Opens one file of a download with the state a checkpoint saved for it, whose tree is
/// `tree`. Its saved chunks are trusted if its size and modification time are unchanged, and
/// otherwise checked against their saved leaf hashes. A file of the wrong size is resized and
/// starts over.
fn resume_download_file(
    path: PathBuf,
    info: &FileInfo,
    saved: &FileState,
    tree: FileTree,
    cancel: &Cancel,
) -> Result<Opened, StorageError> {
    if info.size == 0 {
        return open_download_file(path, info, cancel);
    }
    let (file, _) = open_writable(&path, info)?;
    let metadata = file.metadata().map_err(io_error(&path))?;
    let have = if metadata.len() != info.size {
        file.set_len(info.size).map_err(io_error(&path))?;
        RangeList::new()
    } else if saved.size == info.size
        && saved.modified.is_some()
        && saved.modified == super::checkpoint::modified(&metadata)
    {
        saved.have.clone()
    } else {
        matching_chunks(&file, &path, info.size, &tree, &saved.have, cancel)?
    };
    Ok(Opened {
        file: BundleFile::new(path.clone(), info.clone(), &file, true).map_err(io_error(&path))?,
        tree,
        have,
    })
}

/// Creates the symlink at manifest path `path`, or accepts one already there with the same target.
fn create_symlink(
    manifest: &Manifest,
    base: &Path,
    path: &str,
    target: &str,
) -> Result<(), StorageError> {
    let fs_path = fs_path(base, path);
    let native_target: PathBuf = target.split('/').collect();
    match fs::symlink_metadata(&fs_path) {
        Ok(metadata) if metadata.is_symlink() => {
            let existing = fs::read_link(&fs_path).map_err(io_error(&fs_path))?;
            if existing == native_target {
                return Ok(());
            }
            Err(unusable(&fs_path, "is a symlink to somewhere else"))
        }
        Ok(_) => Err(unusable(&fs_path, "is in the way of a symlink")),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            symlink(manifest, path, target, &native_target, &fs_path).map_err(io_error(&fs_path))
        }
        Err(e) => Err(io_error(&fs_path)(e)),
    }
}

#[cfg(unix)]
fn symlink(
    _manifest: &Manifest,
    _path: &str,
    _target: &str,
    native_target: &Path,
    fs_path: &Path,
) -> io::Result<()> {
    std::os::unix::fs::symlink(native_target, fs_path)
}

/// Windows needs to know whether a symlink points at a directory.
#[cfg(windows)]
fn symlink(
    manifest: &Manifest,
    path: &str,
    target: &str,
    native_target: &Path,
    fs_path: &Path,
) -> io::Result<()> {
    use std::os::windows::fs::{symlink_dir, symlink_file};

    let resolved = tempest_protocol::path::symlink_steps(path, target)
        .ok()
        .and_then(|steps| steps.last().cloned())
        .unwrap_or_default();
    let prefix = format!("{resolved}/");
    let is_dir = resolved.is_empty()
        || manifest.entries().iter().any(|entry| {
            (entry.path == resolved && entry.kind == EntryKind::Directory)
                || entry.path.starts_with(&prefix)
        });
    if is_dir {
        symlink_dir(native_target, fs_path)
    } else {
        symlink_file(native_target, fs_path)
    }
}

/// Makes `options` refuse to open a symlink.
#[cfg(unix)]
fn no_follow(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.custom_flags(libc::O_NOFOLLOW);
}

/// Makes `options` open a symlink itself rather than its target.
#[cfg(windows)]
fn no_follow(options: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
}

#[cfg(unix)]
fn set_executable(file: &File, executable: bool) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = file.metadata()?.permissions();
    let mode = permissions.mode();
    let mode = if executable {
        // Executable by whoever may read it.
        mode | ((mode & 0o444) >> 2)
    } else {
        mode & !0o111
    };
    permissions.set_mode(mode);
    file.set_permissions(permissions)
}

#[cfg(not(unix))]
fn set_executable(_file: &File, _executable: bool) -> io::Result<()> {
    Ok(())
}

/// The leaf hash of every chunk of `file`, at `path` and `size` bytes long.
fn hash_leaves(
    file: &File,
    path: &Path,
    size: u64,
    cancel: &Cancel,
) -> Result<Vec<Hash>, StorageError> {
    if size == 0 {
        return Ok(vec![leaf_hash(&[])]);
    }
    let mut leaves = Vec::with_capacity(chunk_count(size) as usize);
    let mut buffer = vec![0; HASH_BATCH_CHUNKS * CHUNK_SIZE];
    let mut offset = 0;
    while offset < size {
        cancel.check()?;
        let len = (size - offset).min(buffer.len() as u64) as usize;
        read_exact_at(file, &mut buffer[..len], offset).map_err(io_error(path))?;
        leaves.extend(leaf_hashes(&buffer[..len]));
        offset += len as u64;
    }
    Ok(leaves)
}

/// Reads the bytes of chunk positions `[position, position + count)` of `file`, whose manifest
/// fields are `info`, that exist.
pub(super) fn read_chunks(
    file: &File,
    info: &FileInfo,
    position: u32,
    count: u32,
) -> io::Result<Vec<u8>> {
    let offset = u64::from(position) * CHUNK_SIZE as u64;
    let mut chunks = vec![0; data_len(info.size, position, count) as usize];
    read_exact_at(file, &mut chunks, offset)?;
    Ok(chunks)
}

/// Writes `chunks`, which start at chunk position `position`, into `file`.
///
/// The caller has checked that they fit within the file.
pub(super) fn write_chunks(file: &File, position: u32, chunks: &[u8]) -> io::Result<()> {
    write_all_at(file, chunks, u64::from(position) * CHUNK_SIZE as u64)
}

/// Sets `file`'s modification time to the one in the manifest, `millis` since the Unix epoch.
pub(super) fn restore_modified(file: &File, millis: i64) -> io::Result<()> {
    let offset = Duration::from_millis(millis.unsigned_abs());
    let time = if millis >= 0 {
        UNIX_EPOCH.checked_add(offset)
    } else {
        UNIX_EPOCH.checked_sub(offset)
    };
    // A time the platform can't represent is left as it is.
    match time {
        Some(time) => file.set_modified(time),
        None => Ok(()),
    }
}

#[cfg(unix)]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
}

#[cfg(unix)]
pub(super) fn write_all_at(file: &File, buf: &[u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(file, buf, offset)
}

#[cfg(windows)]
fn read_exact_at(file: &File, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        match file.seek_read(buf, offset) {
            Ok(0) => return Err(ErrorKind::UnexpectedEof.into()),
            Ok(n) => {
                buf = &mut buf[n..];
                offset += n as u64;
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(windows)]
pub(super) fn write_all_at(file: &File, mut buf: &[u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        match file.seek_write(buf, offset) {
            Ok(0) => return Err(ErrorKind::WriteZero.into()),
            Ok(n) => {
                buf = &buf[n..];
                offset += n as u64;
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_found_whatever_their_normal_form() {
        let dir = tempfile::tempdir().unwrap();
        // "é" decomposed, in a folder named "ü" decomposed, as macOS's older file systems
        // stored them.
        let folder = dir.path().join("u\u{308}");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("e\u{301}.txt"), "x").unwrap();
        let found = find_entry(dir.path(), "\u{fc}/\u{e9}.txt");
        assert_eq!(fs::read(&found).unwrap(), b"x");
        // A name that is nowhere comes back as it is.
        assert_eq!(find_entry(dir.path(), "none"), dir.path().join("none"));
    }

    #[test]
    fn a_cancelled_opening_stops() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("f"), [1; 100]).unwrap();
        let info = FileInfo {
            size: 100,
            root: tempest_protocol::tree::root(&[1; 100]),
            executable: false,
            modified: 0,
        };
        let manifest =
            Manifest::new("f", "", 0, "", vec![tempest_protocol::Entry::file("f", info)]).unwrap();
        let layout = ChunkLayout::new(&manifest);
        let cancel = Cancel::default();
        assert!(open_shared(dir.path(), &manifest, &layout, &cancel).is_ok());
        cancel.cancel();
        let cancelled = open_shared(dir.path(), &manifest, &layout, &cancel);
        assert!(matches!(cancelled, Err(StorageError::Cancelled)));
    }

    #[test]
    fn stored_bytes_stop_at_the_end_of_the_file() {
        let size = 2 * CHUNK_SIZE as u64 + 10;
        assert_eq!(stored(size, &RangeList::from_chunks([0, 2])), CHUNK_SIZE as u64 + 10);
        assert_eq!(stored(size, &RangeList::new()), 0);
    }
}
