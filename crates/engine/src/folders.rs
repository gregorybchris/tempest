//! Browsing the engine's own folders (D-258), for `GET /v1/folder` and `GET /v1/places`: a
//! folder's entries in the order people expect, and the places worth starting from.
//!
//! Everything here touches the disk and blocks, so the API calls it off the async executor.

use std::cmp::Ordering;
use std::fs::{self, Metadata};
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use tempest_api::folders::{EntryInfo, EntryKind, FolderInfo, PlaceInfo, PlaceKind};
use tempest_net::dirs::home_dir;

/// Why a folder can't be listed.
#[derive(Debug, thiserror::Error)]
pub enum FolderError {
    /// The path isn't absolute.
    #[error("the path must be absolute")]
    Relative,
    /// Nothing is there.
    #[error("{0} does not exist")]
    NotFound(String),
    /// A file is there.
    #[error("{0} is a file, not a folder")]
    NotAFolder(String),
    /// The engine's user may not read it.
    #[error("the engine isn't allowed to read {0}")]
    PermissionDenied(String),
    /// Reading it failed otherwise.
    #[error("can't read {path}: {source}")]
    Io {
        /// The folder.
        path: String,
        /// What went wrong.
        source: std::io::Error,
    },
}

impl FolderError {
    fn of(path: &Path, source: std::io::Error) -> Self {
        let shown = path.display().to_string();
        match source.kind() {
            ErrorKind::NotFound => Self::NotFound(shown),
            ErrorKind::PermissionDenied => Self::PermissionDenied(shown),
            ErrorKind::NotADirectory => Self::NotAFolder(shown),
            _ => Self::Io {
                path: shown,
                source,
            },
        }
    }
}

/// The folder at `path`, listing at most `limit` entries, and hidden ones only if `hidden`.
pub fn list(path: &str, hidden: bool, limit: usize) -> Result<FolderInfo, FolderError> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return Err(FolderError::Relative);
    }
    let path = normalize(path);
    let metadata = fs::metadata(&path).map_err(|e| FolderError::of(&path, e))?;
    if !metadata.is_dir() {
        return Err(FolderError::NotAFolder(path.display().to_string()));
    }
    let reader = fs::read_dir(&path).map_err(|e| FolderError::of(&path, e))?;

    // Names and kinds first, which reading the folder mostly gives for free, so even a huge
    // folder is sorted whole before the entries kept are looked at more closely.
    let mut found = Vec::new();
    for entry in reader {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let symlink = file_type.is_symlink();
        // A link's kind is its target's; a dangling one counts as a file.
        let metadata = match symlink || cfg!(windows) {
            true => fs::metadata(entry.path()).ok(),
            false => None,
        };
        let folder = match &metadata {
            Some(metadata) => metadata.is_dir(),
            None => !symlink && file_type.is_dir(),
        };
        let hidden_entry = name.starts_with('.') || metadata.as_ref().is_some_and(hidden_attribute);
        if hidden_entry && !hidden {
            continue;
        }
        found.push(Found {
            name,
            folder,
            symlink,
            hidden: hidden_entry,
            metadata,
        });
    }
    found.sort_by(|a, b| {
        b.folder
            .cmp(&a.folder)
            .then_with(|| natural(&a.name, &b.name))
    });
    let truncated = found.len() > limit;
    found.truncate(limit);

    let entries = found
        .into_iter()
        .map(|found| {
            let metadata = found
                .metadata
                .or_else(|| fs::metadata(path.join(&found.name)).ok());
            let modified = metadata
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .and_then(|since| i64::try_from(since.as_millis()).ok());
            EntryInfo {
                kind: match found.folder {
                    true => EntryKind::Folder,
                    false => EntryKind::File,
                },
                size: metadata.filter(|_| !found.folder).map(|m| m.len()),
                name: found.name,
                symlink: found.symlink,
                modified,
                hidden: found.hidden,
            }
        })
        .collect();
    Ok(FolderInfo {
        parent: path.parent().map(|p| p.display().to_string()),
        path: path.display().to_string(),
        entries,
        truncated,
    })
}

/// An entry while the folder is read.
struct Found {
    name: String,
    folder: bool,
    symlink: bool,
    hidden: bool,
    metadata: Option<Metadata>,
}

/// Whether Windows marks the entry hidden.
#[cfg(windows)]
fn hidden_attribute(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    metadata.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0
}

/// Whether Windows marks the entry hidden: elsewhere, never.
#[cfg(not(windows))]
fn hidden_attribute(_: &Metadata) -> bool {
    false
}

/// `path` with `.` and `..` resolved by its text alone, so a folder reached through a link
/// keeps the path it was reached by. `..` at a root stays there.
fn normalize(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normal.pop();
            }
            other => normal.push(other),
        }
    }
    normal
}

/// Natural order: case aside, with runs of digits compared as numbers, so `file2` comes before
/// `file10`. Names equal that way are ordered by their text, so the order is total.
pub fn natural(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        let (c, d) = match (x.peek(), y.peek()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(&c), Some(&d)) => (c, d),
        };
        if c.is_ascii_digit() && d.is_ascii_digit() {
            let digits = |chars: &mut std::iter::Peekable<std::str::Chars>| {
                let mut run = String::new();
                while let Some(c) = chars.next_if(char::is_ascii_digit) {
                    run.push(c);
                }
                run
            };
            let (m, n) = (digits(&mut x), digits(&mut y));
            let (m, n) = (m.trim_start_matches('0'), n.trim_start_matches('0'));
            let order = m.len().cmp(&n.len()).then_with(|| m.cmp(n));
            if order.is_ne() {
                return order;
            }
            continue;
        }
        let order = c.to_lowercase().cmp(d.to_lowercase());
        if order.is_ne() {
            return order;
        }
        x.next();
        y.next();
    }
}

/// Where browsing starts when no path is given: the save folder if it exists, else the home
/// folder, else the root.
pub fn start(save_folder: Option<&Path>) -> PathBuf {
    start_with(save_folder, home_dir().as_deref())
}

fn start_with(save_folder: Option<&Path>, home: Option<&Path>) -> PathBuf {
    [save_folder, home]
        .into_iter()
        .flatten()
        .find(|path| path.is_dir())
        .map_or_else(root, Path::to_path_buf)
}

/// The root of the file system: on Windows, the system drive's.
fn root() -> PathBuf {
    if cfg!(windows) {
        let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_owned());
        PathBuf::from(format!("{drive}\\"))
    } else {
        PathBuf::from("/")
    }
}

/// The home folder's usual folders, by name. Linux and Windows call Movies Videos.
const USUAL: [&str; 7] = [
    "Desktop",
    "Documents",
    "Downloads",
    "Movies",
    "Videos",
    "Music",
    "Pictures",
];

/// The places worth starting from: the home folder, the save folder, the home folder's usual
/// folders, mounted volumes or drives, and the root. Each is listed once, and only if it is
/// there.
pub fn places(save_folder: Option<&Path>) -> Vec<PlaceInfo> {
    let user = std::env::var("USER").ok();
    let volumes = if cfg!(windows) {
        drives()
    } else {
        let roots: &[&str] = if cfg!(target_os = "macos") {
            &["/Volumes"]
        } else {
            &["/media", "/mnt", "/run/media"]
        };
        let roots: Vec<PathBuf> = roots.iter().map(PathBuf::from).collect();
        volumes_in(&roots, user.as_deref())
            .into_iter()
            .map(|path| (PlaceKind::Volume, path))
            .collect()
    };
    places_with(home_dir().as_deref(), save_folder, volumes)
}

fn places_with(
    home: Option<&Path>,
    save_folder: Option<&Path>,
    volumes: Vec<(PlaceKind, PathBuf)>,
) -> Vec<PlaceInfo> {
    let mut candidates = Vec::new();
    if let Some(home) = home {
        candidates.push((PlaceKind::Home, "Home".to_owned(), home.to_path_buf()));
    }
    if let Some(save_folder) = save_folder {
        let name = "Save folder".to_owned();
        candidates.push((PlaceKind::SaveFolder, name, save_folder.to_path_buf()));
    }
    if let Some(home) = home {
        for name in USUAL {
            candidates.push((PlaceKind::Folder, name.to_owned(), home.join(name)));
        }
    }
    for (kind, path) in volumes {
        candidates.push((kind, name_of(&path), path));
    }
    candidates.push((PlaceKind::Root, "Root".to_owned(), root()));

    let mut places: Vec<PlaceInfo> = Vec::new();
    for (kind, name, path) in candidates {
        let Some(text) = path.to_str() else { continue };
        if !path.is_dir() || places.iter().any(|p| p.path == text) {
            continue;
        }
        places.push(PlaceInfo {
            name,
            path: text.to_owned(),
            kind,
        });
    }
    places
}

/// A path's last component, or the whole path for a root.
fn name_of(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// The volumes mounted in `roots`: each folder in them, in natural order, except a folder named
/// after `user`, whose own folders are the volumes (as in `/media/<user>` and
/// `/run/media/<user>`), and links back to the file system's root (as macOS's startup disk in
/// `/Volumes`).
fn volumes_in(roots: &[PathBuf], user: Option<&str>) -> Vec<PathBuf> {
    let folders = |dir: &Path| {
        let mut found: Vec<PathBuf> = fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        found.sort_by(|a, b| natural(&name_of(a), &name_of(b)));
        found
    };
    let mut volumes = Vec::new();
    for root in roots {
        for folder in folders(root) {
            if user.is_some_and(|user| folder.file_name().is_some_and(|n| n == user)) {
                volumes.extend(folders(&folder));
            } else {
                volumes.push(folder);
            }
        }
    }
    volumes.retain(|path| fs::canonicalize(path).is_ok_and(|real| real.parent().is_some()));
    volumes
}

/// The drive letters in use, on Windows.
fn drives() -> Vec<(PlaceKind, PathBuf)> {
    if !cfg!(windows) {
        return Vec::new();
    }
    // A and B are floppy drives, which can stall a check for seconds.
    (b'C'..=b'Z')
        .map(|letter| PathBuf::from(format!("{}:\\", letter as char)))
        .filter(|drive| drive.is_dir())
        .map(|drive| (PlaceKind::Drive, drive))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(folder: &FolderInfo) -> Vec<&str> {
        folder.entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn natural_order_ignores_case_and_counts_numbers() {
        let mut names = vec!["file10", "File2", "file1", "b", "A", "a", "x9y", "x10y", ""];
        names.sort_by(|a, b| natural(a, b));
        assert_eq!(
            names,
            ["", "A", "a", "b", "file1", "File2", "file10", "x9y", "x10y"]
        );
        assert_eq!(natural("2", "02"), "2".cmp("02"));
        assert_eq!(natural("a", "a"), Ordering::Equal);
    }

    #[test]
    fn folders_come_first_and_hidden_entries_only_when_asked() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["b.txt", "A.txt", "file10", "file9", ".secret"] {
            fs::write(dir.path().join(name), name).unwrap();
        }
        for name in ["zeta", "Alpha", ".config"] {
            fs::create_dir(dir.path().join(name)).unwrap();
        }
        let path = dir.path().to_str().unwrap();
        let shown = list(path, false, 100).unwrap();
        assert_eq!(
            names(&shown),
            ["Alpha", "zeta", "A.txt", "b.txt", "file9", "file10"]
        );
        assert!(!shown.truncated);
        let alpha = &shown.entries[0];
        assert_eq!((alpha.kind, alpha.size), (EntryKind::Folder, None));
        let a = &shown.entries[2];
        assert_eq!(
            (a.kind, a.size, a.hidden),
            (EntryKind::File, Some(5), false)
        );
        assert!(a.modified.is_some_and(|m| m > 0));

        let all = list(path, true, 100).unwrap();
        assert_eq!(names(&all)[..3], [".config", "Alpha", "zeta"]);
        let secret = all.entries.iter().find(|e| e.name == ".secret").unwrap();
        assert!(secret.hidden);
    }

    #[test]
    fn the_path_is_resolved_and_its_parent_given() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("a/b")).unwrap();
        let path = format!("{}/a/./b/..", dir.path().display());
        let folder = list(&path, false, 100).unwrap();
        assert_eq!(folder.path, dir.path().join("a").display().to_string());
        assert_eq!(folder.parent, Some(dir.path().display().to_string()));
        assert_eq!(names(&folder), ["b"]);
        assert_eq!(normalize(Path::new("/../a/../..")), Path::new("/"));
        if cfg!(unix) {
            assert_eq!(list("/", false, 1).unwrap().parent, None);
        }
    }

    #[test]
    fn big_folders_are_cut_short() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..12 {
            fs::write(dir.path().join(format!("f{i}")), "").unwrap();
        }
        let folder = list(dir.path().to_str().unwrap(), false, 10).unwrap();
        assert!(folder.truncated);
        assert_eq!(folder.entries.len(), 10);
        // The ones kept are the first in order, not the first the disk gave.
        assert_eq!(folder.entries[2].name, "f2");
        assert_eq!(folder.entries[9].name, "f9");
    }

    #[cfg(unix)]
    #[test]
    fn links_take_their_targets_kind_and_say_so() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("real")).unwrap();
        fs::write(dir.path().join("file"), "12345678").unwrap();
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("to-folder")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("file"), dir.path().join("to-file")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("gone"), dir.path().join("dangling")).unwrap();
        let folder = list(dir.path().to_str().unwrap(), false, 100).unwrap();
        let entry = |name: &str| folder.entries.iter().find(|e| e.name == name).unwrap();
        assert_eq!(
            names(&folder),
            ["real", "to-folder", "dangling", "file", "to-file"]
        );
        assert!(entry("to-folder").symlink && entry("to-folder").kind == EntryKind::Folder);
        assert_eq!(entry("to-file").size, Some(8));
        assert!(entry("to-file").symlink);
        assert!(entry("dangling").symlink && entry("dangling").kind == EntryKind::File);
        assert!(!entry("real").symlink);
        // A folder reached through a link keeps the path it was reached by.
        let through = dir.path().join("to-folder");
        let listed = list(through.to_str().unwrap(), false, 100).unwrap();
        assert_eq!(listed.path, through.display().to_string());
    }

    #[test]
    fn what_cant_be_listed_says_why() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("file"), "").unwrap();
        assert!(matches!(
            list("relative/path", false, 10),
            Err(FolderError::Relative)
        ));
        let missing = dir.path().join("missing");
        assert!(matches!(
            list(missing.to_str().unwrap(), false, 10),
            Err(FolderError::NotFound(_))
        ));
        let file = dir.path().join("file");
        assert!(matches!(
            list(file.to_str().unwrap(), false, 10),
            Err(FolderError::NotAFolder(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_the_engine_may_not_read_is_refused_as_such() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let result = list(locked.to_str().unwrap(), false, 10);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        // Root reads anything, so only check what an ordinary user gets.
        if let Err(error) = result {
            assert!(matches!(error, FolderError::PermissionDenied(_)), "{error}");
        }
    }

    #[test]
    fn browsing_starts_at_the_save_folder_then_home() {
        let dir = tempfile::tempdir().unwrap();
        let (save, home) = (dir.path().join("save"), dir.path().join("home"));
        fs::create_dir(&home).unwrap();
        assert_eq!(start_with(Some(&save), Some(&home)), home);
        fs::create_dir(&save).unwrap();
        assert_eq!(start_with(Some(&save), Some(&home)), save);
        assert_eq!(start_with(None, Some(&dir.path().join("gone"))), root());
    }

    #[test]
    fn places_are_listed_once_and_only_if_there() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        for name in ["Desktop", "Downloads", "Music"] {
            fs::create_dir_all(home.join(name)).unwrap();
        }
        let volume = dir.path().join("usb");
        fs::create_dir(&volume).unwrap();
        let places = places_with(
            Some(&home),
            Some(&home.join("Downloads")),
            vec![(PlaceKind::Volume, volume.clone())],
        );
        let listed: Vec<(PlaceKind, &str)> =
            places.iter().map(|p| (p.kind, p.name.as_str())).collect();
        assert_eq!(
            listed,
            [
                (PlaceKind::Home, "Home"),
                (PlaceKind::SaveFolder, "Save folder"),
                (PlaceKind::Folder, "Desktop"),
                (PlaceKind::Folder, "Music"),
                (PlaceKind::Volume, "usb"),
                (PlaceKind::Root, "Root"),
            ]
        );
        assert_eq!(places[4].path, volume.display().to_string());
        assert_eq!(places_with(None, None, Vec::new()).len(), 1);
    }

    #[test]
    fn volumes_are_the_folders_in_mount_points_and_in_the_users_own() {
        let dir = tempfile::tempdir().unwrap();
        let (media, mnt) = (dir.path().join("media"), dir.path().join("mnt"));
        for path in [
            "media/chris/USB 10",
            "media/chris/USB 9",
            "media/shared",
            "mnt/backup",
        ] {
            fs::create_dir_all(dir.path().join(path)).unwrap();
        }
        fs::write(mnt.join("not-a-volume"), "").unwrap();
        let found = volumes_in(
            &[media.clone(), mnt.clone(), dir.path().join("none")],
            Some("chris"),
        );
        assert_eq!(
            found,
            [
                media.join("chris/USB 9"),
                media.join("chris/USB 10"),
                media.join("shared"),
                mnt.join("backup"),
            ]
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/", mnt.join("startup")).unwrap();
            assert_eq!(
                volumes_in(std::slice::from_ref(&mnt), None),
                [mnt.join("backup")]
            );
        }
    }
}
