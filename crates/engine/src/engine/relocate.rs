//! Moving a transfer's files to another folder, all or nothing.
//!
//! Each item, a file or a folder, is renamed into place. Across file systems, where renaming
//! fails, it is copied instead, and the originals are deleted only once the move is recorded.
//! If anything fails, what was moved is moved back and what was copied is deleted. A move a
//! crash interrupted is undone at the next start (see [`undo_interrupted`]).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// A file or folder and where it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Item {
    pub from: PathBuf,
    pub to: PathBuf,
}

/// How an item got where it went.
enum Placed {
    Renamed,
    Copied,
}

/// Moves every item, or none. Returns the originals of the items copied rather than renamed,
/// for [`remove_originals`] once the move is recorded.
pub(super) fn move_all(items: &[Item]) -> io::Result<Vec<PathBuf>> {
    move_all_with(items, |from, to| fs::rename(from, to))
}

/// [`move_all`], renaming with `rename`.
fn move_all_with(
    items: &[Item],
    rename: impl Fn(&Path, &Path) -> io::Result<()>,
) -> io::Result<Vec<PathBuf>> {
    for item in items {
        if item.to.symlink_metadata().is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} already exists", item.to.display()),
            ));
        }
    }
    let mut placed: Vec<(&Item, Placed)> = Vec::new();
    for item in items {
        // Nothing to move for a file not made yet.
        if item.from.symlink_metadata().is_err() {
            continue;
        }
        match place(item, &rename) {
            Ok(how) => placed.push((item, how)),
            Err(error) => {
                undo(&placed, &rename);
                return Err(io::Error::new(
                    error.kind(),
                    format!("{}: {error}", item.from.display()),
                ));
            }
        }
    }
    Ok(placed
        .into_iter()
        .filter(|(_, how)| matches!(how, Placed::Copied))
        .map(|(item, _)| item.from.clone())
        .collect())
}

/// Deletes the originals of copied items. The data is safe at its new place, so an original
/// left behind is only clutter.
pub(super) fn remove_originals(originals: &[PathBuf]) {
    for original in originals {
        if let Err(error) = remove(original) {
            tracing::warn!(
                "couldn't delete {} after moving it: {error}",
                original.display()
            );
        }
    }
}

/// Undoes a move that a crash interrupted before it was recorded: each item found at its new
/// place and not at its old one was renamed, and is renamed back. An item at both places was
/// being copied, perhaps not completely, so the copy is left alone; its path is returned.
pub(super) fn undo_interrupted(items: &[Item]) -> io::Result<Vec<PathBuf>> {
    let mut copies = Vec::new();
    for item in items {
        let at_new = item.to.symlink_metadata().is_ok();
        let at_old = item.from.symlink_metadata().is_ok();
        match (at_old, at_new) {
            (false, true) => {
                if let Some(parent) = item.from.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::rename(&item.to, &item.from).map_err(|error| {
                    io::Error::new(
                        error.kind(),
                        format!("{} can't go back: {error}", item.to.display()),
                    )
                })?;
            }
            (true, true) => copies.push(item.to.clone()),
            (_, false) => {}
        }
    }
    Ok(copies)
}

fn place(item: &Item, rename: &impl Fn(&Path, &Path) -> io::Result<()>) -> io::Result<Placed> {
    if let Some(parent) = item.to.parent() {
        fs::create_dir_all(parent)?;
    }
    match rename(&item.from, &item.to) {
        Ok(()) => Ok(Placed::Renamed),
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
            match copy(&item.from, &item.to) {
                Ok(()) => Ok(Placed::Copied),
                Err(error) => {
                    let _ = remove(&item.to);
                    Err(error)
                }
            }
        }
        Err(error) => Err(error),
    }
}

/// Puts back what was placed, latest first.
fn undo(placed: &[(&Item, Placed)], rename: &impl Fn(&Path, &Path) -> io::Result<()>) {
    for (item, how) in placed.iter().rev() {
        let _ = match how {
            Placed::Renamed => rename(&item.to, &item.from),
            Placed::Copied => remove(&item.to),
        };
    }
}

/// Copies a file, symlink or folder, keeping files' permissions and modification times, so a
/// moved transfer resumes without hashing (D-179).
fn copy(from: &Path, to: &Path) -> io::Result<()> {
    let metadata = from.symlink_metadata()?;
    if metadata.is_symlink() {
        return copy_symlink(from, to);
    }
    if metadata.is_dir() {
        fs::create_dir(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy(&entry.path(), &to.join(entry.file_name()))?;
        }
        return Ok(());
    }
    fs::copy(from, to)?;
    let file = fs::OpenOptions::new().write(true).open(to)?;
    file.set_modified(metadata.modified()?)?;
    file.sync_all()
}

#[cfg(unix)]
fn copy_symlink(from: &Path, to: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(fs::read_link(from)?, to)
}

#[cfg(not(unix))]
fn copy_symlink(from: &Path, _to: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!("can't copy the symlink {}", from.display()),
    ))
}

fn remove(path: &Path) -> io::Result<()> {
    match path.symlink_metadata()?.is_dir() {
        true => fs::remove_dir_all(path),
        false => fs::remove_file(path),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::*;

    fn tree(root: &Path) {
        fs::create_dir_all(root.join("folder/inner")).unwrap();
        fs::write(root.join("folder/inner/a.txt"), "a").unwrap();
        fs::write(root.join("b.txt"), "b").unwrap();
    }

    fn items(from: &Path, to: &Path) -> Vec<Item> {
        ["folder", "b.txt"]
            .into_iter()
            .map(|name| Item {
                from: from.join(name),
                to: to.join(name),
            })
            .collect()
    }

    fn cross_device(_: &Path, _: &Path) -> io::Result<()> {
        Err(io::ErrorKind::CrossesDevices.into())
    }

    #[test]
    fn items_are_renamed() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("from"), dir.path().join("to/deeper"));
        tree(&from);
        assert!(move_all(&items(&from, &to)).unwrap().is_empty());
        assert_eq!(fs::read(to.join("folder/inner/a.txt")).unwrap(), b"a");
        assert_eq!(fs::read(to.join("b.txt")).unwrap(), b"b");
        assert!(!from.join("folder").exists() && !from.join("b.txt").exists());
    }

    #[test]
    fn items_are_copied_across_file_systems_keeping_times() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("from"), dir.path().join("to"));
        tree(&from);
        let then = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let file = fs::File::options()
            .write(true)
            .open(from.join("b.txt"))
            .unwrap();
        file.set_modified(then).unwrap();
        drop(file);
        let originals = move_all_with(&items(&from, &to), cross_device).unwrap();
        assert_eq!(fs::read(to.join("folder/inner/a.txt")).unwrap(), b"a");
        let modified = fs::metadata(to.join("b.txt")).unwrap().modified().unwrap();
        assert_eq!(modified, then);
        // The originals stay until the move is recorded.
        assert_eq!(originals, [from.join("folder"), from.join("b.txt")]);
        assert!(from.join("b.txt").exists());
        remove_originals(&originals);
        assert!(!from.join("folder").exists() && !from.join("b.txt").exists());
    }

    #[test]
    fn an_interrupted_move_is_undone() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("from"), dir.path().join("to"));
        tree(&from);
        // The crash came after "folder" was renamed and while "b.txt" was being copied.
        fs::create_dir_all(&to).unwrap();
        fs::rename(from.join("folder"), to.join("folder")).unwrap();
        fs::write(to.join("b.txt"), "partial").unwrap();
        let copies = undo_interrupted(&items(&from, &to)).unwrap();
        assert_eq!(copies, [to.join("b.txt")]);
        assert_eq!(fs::read(from.join("folder/inner/a.txt")).unwrap(), b"a");
        assert_eq!(fs::read(from.join("b.txt")).unwrap(), b"b");
        assert!(!to.join("folder").exists());
        // Undoing again changes nothing.
        assert_eq!(undo_interrupted(&items(&from, &to)).unwrap(), copies);
    }

    #[test]
    fn a_failure_puts_everything_back() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("from"), dir.path().join("to"));
        tree(&from);
        // The first item renames; the second fails.
        let rename = |a: &Path, b: &Path| match a.ends_with("b.txt") {
            true => Err(io::ErrorKind::PermissionDenied.into()),
            false => fs::rename(a, b),
        };
        let error = move_all_with(&items(&from, &to), rename).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(fs::read(from.join("folder/inner/a.txt")).unwrap(), b"a");
        assert_eq!(fs::read(from.join("b.txt")).unwrap(), b"b");
        assert!(!to.join("folder").exists());

        // Copies are deleted on failure too, and the originals kept.
        let copy_then_fail = |a: &Path, _: &Path| match a.ends_with("b.txt") {
            true => Err(io::ErrorKind::PermissionDenied.into()),
            false => Err(io::ErrorKind::CrossesDevices.into()),
        };
        assert!(move_all_with(&items(&from, &to), copy_then_fail).is_err());
        assert!(from.join("folder/inner/a.txt").exists());
        assert!(!to.join("folder").exists());
    }

    #[test]
    fn nothing_moves_onto_something_already_there() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("from"), dir.path().join("to"));
        tree(&from);
        fs::create_dir_all(&to).unwrap();
        fs::write(to.join("b.txt"), "theirs").unwrap();
        let error = move_all(&items(&from, &to)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert!(from.join("folder").exists());
        assert_eq!(fs::read(to.join("b.txt")).unwrap(), b"theirs");
    }
}
