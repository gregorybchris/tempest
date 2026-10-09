//! Browsing the engine's own folders (D-258): `GET /v1/folder` lists one, and `GET /v1/places`
//! names the places worth starting from. Clients use them to pick paths on the engine's
//! machine, which a browser's own file picker can't show.
//!
//! Paths are strings, as the request gave them or as the engine resolved them, so a name the
//! engine can't read as UTF-8 shows with replacement characters rather than failing the listing.

use serde::{Deserialize, Serialize};

/// The most entries `GET /v1/folder` lists; a folder with more is cut short, with `truncated`.
pub const MAX_ENTRIES: usize = 10_000;

/// `GET /v1/folder`: one folder's entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderInfo {
    /// The folder's absolute path, with `.` and `..` resolved.
    pub path: String,
    /// The folder it is in; none at a root.
    pub parent: Option<String>,
    /// Folders first, then files, each in natural order: case aside, and `file2` before
    /// `file10`.
    pub entries: Vec<EntryInfo>,
    /// Whether the folder had more than [`MAX_ENTRIES`] to list, so some are left out.
    pub truncated: bool,
}

/// An entry of a folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryInfo {
    /// Its name.
    pub name: String,
    /// A folder or a file, after following a symbolic link.
    pub kind: EntryKind,
    /// Whether it is a symbolic link.
    pub symlink: bool,
    /// A file's size in bytes.
    pub size: Option<u64>,
    /// When it last changed, in milliseconds since the Unix epoch, if known.
    pub modified: Option<i64>,
    /// Whether it is hidden: its name starts with `.`, or on Windows it has the hidden
    /// attribute.
    pub hidden: bool,
}

/// What an entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// A folder, or a link to one.
    Folder,
    /// Anything else: a file, or a link to one or to nothing.
    File,
}

/// `GET /v1/places`: a folder worth starting from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaceInfo {
    /// What to call it.
    pub name: String,
    /// Its absolute path.
    pub path: String,
    /// What it is.
    pub kind: PlaceKind,
}

/// What a place is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaceKind {
    /// The home folder of the user the engine runs as.
    Home,
    /// The engine's default save folder.
    SaveFolder,
    /// One of the home folder's usual folders, such as Documents or Downloads.
    Folder,
    /// A mounted volume, such as a USB drive.
    Volume,
    /// A drive letter, on Windows.
    Drive,
    /// The root of the file system.
    Root,
}
