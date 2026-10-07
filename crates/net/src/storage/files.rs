//! A bundle's files on disk: opening a shared folder, laying out a download, and positioned reads,
//! writes and syncs. Everything here blocks, so it runs on the blocking pool.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use tempest_protocol::hash::leaf_hash;
use tempest_protocol::path::{ancestors, parent};
use tempest_protocol::tree::{CHUNK_SIZE, chunk_count, leaf_hashes};
use tempest_protocol::{ChunkLayout, EntryKind, FileInfo, Hash, Manifest};

use super::StorageError;
use super::tree::FileTree;

/// How many chunks [`hash_leaves`] reads at a time.
const HASH_BATCH_CHUNKS: usize = 64;

/// One file of a bundle, open for positioned I/O.
#[derive(Debug)]
pub(super) struct BundleFile {
    /// Where the file is on disk.
    pub path: PathBuf,
    /// The open file: read-only when shared, read-write when downloading.
    pub file: File,
    /// The file's size, root and other fields from the manifest.
    pub info: FileInfo,
}

/// A file just opened, with what is known of its chunk tree and whether its chunks are all
/// present.
#[derive(Debug)]
pub(super) struct Opened {
    pub file: BundleFile,
    pub tree: FileTree,
    pub complete: bool,
}

/// Where the entry at manifest path `path` lies under `base`. The manifest's path rules make
/// every segment a plain name on every platform, so this never leaves `base`.
fn fs_path(base: &Path, path: &str) -> PathBuf {
    let mut fs_path = base.to_path_buf();
    fs_path.extend(path.split('/'));
    fs_path
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
) -> Result<Vec<Opened>, StorageError> {
    manifest_files(manifest, layout)
        .map(|(path, info)| {
            let path = fs_path(base, path);
            let metadata = fs::symlink_metadata(&path).map_err(io_error(&path))?;
            if !metadata.is_file() {
                return Err(unusable(&path, "is not a regular file"));
            }
            let file = File::open(&path).map_err(io_error(&path))?;
            let changed = || unusable(&path, "has changed since the bundle was made");
            if file.metadata().map_err(io_error(&path))?.len() != info.size {
                return Err(changed());
            }
            let tree =
                FileTree::from_leaves(&hash_leaves(&file, info.size).map_err(io_error(&path))?);
            if tree.root() != Some(info.root) {
                return Err(changed());
            }
            Ok(Opened {
                file: BundleFile {
                    path,
                    file,
                    info: info.clone(),
                },
                tree,
                complete: true,
            })
        })
        .collect()
}

/// Lays out a download of `manifest` in `dir`: creates `dir` if needed, then every directory,
/// every file at its full size, and finally every symlink.
///
/// Nothing already in `dir` is followed: a symlink, or anything else that isn't what the manifest
/// says should be at a path, is an error. A file that already exists with the right size and
/// root is kept and counts as complete; any other existing file is resized and downloaded again.
pub(super) fn create_download(
    dir: &Path,
    manifest: &Manifest,
    layout: &ChunkLayout,
) -> Result<Vec<Opened>, StorageError> {
    fs::create_dir_all(dir).map_err(io_error(dir))?;
    let mut dirs = Directories {
        base: dir,
        made: HashSet::new(),
    };
    for entry in manifest.entries() {
        match &entry.kind {
            EntryKind::Directory => dirs.ensure(&entry.path)?,
            EntryKind::File(_) => dirs.ensure(parent(&entry.path))?,
            EntryKind::Symlink { .. } => {}
        }
    }
    let opened = manifest_files(manifest, layout)
        .map(|(path, info)| open_download_file(fs_path(dir, path), info))
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

/// Opens or creates one file of a download, sized to `info.size`.
fn open_download_file(path: PathBuf, info: &FileInfo) -> Result<Opened, StorageError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    no_follow(&mut options);
    let existing = match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() => true,
        Ok(_) => return Err(unusable(&path, "is in the way of a file")),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            options.create_new(true);
            false
        }
        Err(e) => return Err(io_error(&path)(e)),
    };
    let file = options.open(&path).map_err(io_error(&path))?;
    set_executable(&file, info.executable).map_err(io_error(&path))?;

    let count = chunk_count(info.size);
    let mut opened = Opened {
        tree: FileTree::new(count, info.root),
        complete: false,
        file: BundleFile {
            path,
            file,
            info: info.clone(),
        },
    };
    let bundle_file = &opened.file;
    let len = bundle_file
        .file
        .metadata()
        .map_err(io_error(&bundle_file.path))?
        .len();
    if info.size == 0 {
        // An empty file is complete as soon as it exists.
        bundle_file
            .file
            .set_len(0)
            .map_err(io_error(&bundle_file.path))?;
        opened.tree = FileTree::from_leaves(&[leaf_hash(&[])]);
        opened.complete = true;
    } else if existing && len == info.size {
        let leaves =
            hash_leaves(&bundle_file.file, info.size).map_err(io_error(&bundle_file.path))?;
        let tree = FileTree::from_leaves(&leaves);
        if tree.root() == Some(info.root) {
            opened.tree = tree;
            opened.complete = true;
        }
    } else {
        bundle_file
            .file
            .set_len(info.size)
            .map_err(io_error(&bundle_file.path))?;
    }
    Ok(opened)
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

/// The leaf hash of every chunk of `file`, which is `size` bytes long.
pub(super) fn hash_leaves(file: &File, size: u64) -> io::Result<Vec<Hash>> {
    if size == 0 {
        return Ok(vec![leaf_hash(&[])]);
    }
    let mut leaves = Vec::with_capacity(chunk_count(size) as usize);
    let mut buffer = vec![0; HASH_BATCH_CHUNKS * CHUNK_SIZE];
    let mut offset = 0;
    while offset < size {
        let len = (size - offset).min(buffer.len() as u64) as usize;
        read_exact_at(file, &mut buffer[..len], offset)?;
        leaves.extend(leaf_hashes(&buffer[..len]));
        offset += len as u64;
    }
    Ok(leaves)
}

/// Reads the bytes of chunk positions `[position, position + count)` of `file` that exist.
pub(super) fn read_chunks(file: &BundleFile, position: u32, count: u32) -> io::Result<Vec<u8>> {
    let offset = u64::from(position) * CHUNK_SIZE as u64;
    let len = tempest_protocol::tree::data_len(file.info.size, position, count);
    let mut chunks = vec![0; len as usize];
    read_exact_at(&file.file, &mut chunks, offset)?;
    Ok(chunks)
}

/// Writes `chunks`, which start at chunk position `position`, into `file`.
///
/// The caller has checked that they fit within the file.
pub(super) fn write_chunks(file: &BundleFile, position: u32, chunks: &[u8]) -> io::Result<()> {
    let offset = u64::from(position) * CHUNK_SIZE as u64;
    debug_assert!(offset + chunks.len() as u64 <= file.info.size);
    write_all_at(&file.file, chunks, offset)
}

/// Sets `file`'s modification time to the one in the manifest.
pub(super) fn restore_modified(file: &BundleFile) -> io::Result<()> {
    let millis = file.info.modified;
    let offset = Duration::from_millis(millis.unsigned_abs());
    let time = if millis >= 0 {
        UNIX_EPOCH.checked_add(offset)
    } else {
        UNIX_EPOCH.checked_sub(offset)
    };
    // A time the platform can't represent is left as it is.
    match time {
        Some(time) => file.file.set_modified(time),
        None => Ok(()),
    }
}

#[cfg(unix)]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
}

#[cfg(unix)]
fn write_all_at(file: &File, buf: &[u8], offset: u64) -> io::Result<()> {
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
fn write_all_at(file: &File, mut buf: &[u8], mut offset: u64) -> io::Result<()> {
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
