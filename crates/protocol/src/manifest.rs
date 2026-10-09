//! Manifests and bundle IDs (`protocol/content.md`, The manifest).
//!
//! A manifest describes a bundle: its name and other descriptive fields, and its entries (files,
//! symlinks and empty directories). Its canonical encoding determines the bundle ID. A
//! [`Manifest`] value is always valid: [`Manifest::new`] and decoding both check every rule.

use std::collections::HashMap;

use crate::hash::{Hash, leaf_hash, manifest_hash};
use crate::path::{
    EscapesBundle, PathError, ancestors, fold_case, validate_name, validate_path,
    validate_symlink_target,
};
use crate::tree::chunk_count;
use crate::wire::{Decode, DecodeError, Encode, Reader, Writer};

/// The only manifest format version.
pub const MANIFEST_VERSION: u8 = 1;

/// The largest encoded manifest: 64 MiB.
pub const MAX_MANIFEST_LEN: usize = 64 * 1024 * 1024;

/// The most chunks a bundle may have, since chunk indexes are 32-bit.
pub const MAX_BUNDLE_CHUNKS: u64 = u32::MAX as u64;

const KIND_FILE: u8 = 0;
const KIND_SYMLINK: u8 = 1;
const KIND_DIRECTORY: u8 = 2;

/// A bundle ID: the hash of an encoded manifest, `SHA-256(0x02 ‖ manifest bytes)`.
pub type BundleId = Hash;

/// A file entry's fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInfo {
    /// The file's size in bytes.
    pub size: u64,
    /// The root of the file's chunk tree.
    pub root: Hash,
    /// Whether the file is executable.
    pub executable: bool,
    /// The file's modification time, in milliseconds since the Unix epoch.
    pub modified: i64,
}

/// What an entry is, with the fields for its kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryKind {
    /// A regular file.
    File(FileInfo),
    /// A symbolic link to a relative `target`.
    Symlink {
        /// The link's target, relative to the link's own directory.
        target: String,
    },
    /// An empty directory. Directories that contain entries are implicit.
    Directory,
}

/// One item in a manifest: a path relative to the bundle's top level, and what is there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The entry's path, with `/` between segments.
    pub path: String,
    /// What the entry is.
    pub kind: EntryKind,
}

impl Entry {
    /// A file entry.
    pub fn file(path: impl Into<String>, info: FileInfo) -> Self {
        Self {
            path: path.into(),
            kind: EntryKind::File(info),
        }
    }

    /// A symlink entry.
    pub fn symlink(path: impl Into<String>, target: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            kind: EntryKind::Symlink {
                target: target.into(),
            },
        }
    }

    /// An empty directory entry.
    pub fn directory(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            kind: EntryKind::Directory,
        }
    }

    /// The file fields, if this is a file.
    pub fn as_file(&self) -> Option<&FileInfo> {
        match &self.kind {
            EntryKind::File(info) => Some(info),
            _ => None,
        }
    }
}

/// Why a manifest is invalid.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManifestError {
    /// The bundle's name was not a valid path segment.
    #[error("invalid name: {0}")]
    InvalidName(PathError),
    /// The manifest had no entries.
    #[error("a manifest must have at least one entry")]
    NoEntries,
    /// An entry's path broke a rule.
    #[error("invalid path {path:?}: {error}")]
    InvalidPath {
        /// The entry's path.
        path: String,
        /// The rule it broke.
        error: PathError,
    },
    /// Entries were not sorted by the bytes of their paths.
    #[error("entry {0:?} is out of order")]
    Unsorted(String),
    /// Two entries had the same path.
    #[error("duplicate path {0:?}")]
    DuplicatePath(String),
    /// Two different paths, of entries or of the directories containing them, were equal when
    /// compared case-insensitively.
    #[error("paths {0:?} and {1:?} differ only by case")]
    CaseCollision(String, String),
    /// An entry was inside a file or symlink.
    #[error("{path:?} is inside the file or symlink {parent:?}")]
    InsideFileOrSymlink {
        /// The entry's path.
        path: String,
        /// The file or symlink containing it.
        parent: String,
    },
    /// A directory entry was present for a directory that contains other entries.
    #[error("directory entry {0:?} is not empty")]
    NonEmptyDirectory(String),
    /// An empty file's root was not the root of an empty file, `SHA-256(0x00)`.
    #[error("empty file {0:?} has the wrong root")]
    EmptyFileRoot(String),
    /// A symlink's target broke a path rule.
    #[error("symlink {path:?} has invalid target: {error}")]
    InvalidSymlinkTarget {
        /// The symlink's path.
        path: String,
        /// The rule its target broke.
        error: PathError,
    },
    /// Resolving a symlink's target left the bundle's top level.
    #[error("symlink {0:?}: target leads out of the bundle")]
    SymlinkEscapes(String),
    /// Resolving a symlink's target passed through or landed on a symlink.
    #[error("symlink {path:?} points through or at the symlink {via:?}")]
    SymlinkThroughSymlink {
        /// The symlink's path.
        path: String,
        /// The symlink its target reaches.
        via: String,
    },
    /// The files had more than [`MAX_BUNDLE_CHUNKS`] chunks in total.
    #[error("the bundle has more than 2^32 - 1 chunks")]
    TooManyChunks,
    /// The encoded manifest was larger than [`MAX_MANIFEST_LEN`].
    #[error("the manifest is {0} bytes, more than the limit of 64 MiB")]
    TooLarge(usize),
}

/// A valid manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    name: String,
    comment: String,
    created: i64,
    creator: String,
    entries: Vec<Entry>,
}

impl Manifest {
    /// Creates a manifest, sorting `entries` by path and checking every rule.
    pub fn new(
        name: impl Into<String>,
        comment: impl Into<String>,
        created: i64,
        creator: impl Into<String>,
        mut entries: Vec<Entry>,
    ) -> Result<Self, ManifestError> {
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        let manifest = Self {
            name: name.into(),
            comment: comment.into(),
            created,
            creator: creator.into(),
            entries,
        };
        manifest.validate()?;
        let len = manifest.encoded_len();
        if len > MAX_MANIFEST_LEN {
            return Err(ManifestError::TooLarge(len));
        }
        Ok(manifest)
    }

    /// The suggested display name for the bundle.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The creator's free-text description. May be empty.
    pub fn comment(&self) -> &str {
        &self.comment
    }

    /// When the bundle was created, in milliseconds since the Unix epoch.
    pub fn created(&self) -> i64 {
        self.created
    }

    /// The name and version of the tool that created the bundle.
    pub fn creator(&self) -> &str {
        &self.creator
    }

    /// The entries, sorted by the bytes of their paths.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The entry's path, if the bundle is exactly one file entry.
    pub fn single_file(&self) -> Option<&str> {
        match self.entries.as_slice() {
            [entry] if entry.as_file().is_some() => Some(&entry.path),
            _ => None,
        }
    }

    /// The bundle ID.
    pub fn bundle_id(&self) -> BundleId {
        manifest_hash(&self.to_bytes())
    }

    /// The total size of all files, in bytes.
    pub fn total_size(&self) -> u64 {
        self.entries
            .iter()
            .filter_map(Entry::as_file)
            .map(|file| file.size)
            .sum()
    }

    /// The length of the manifest's encoding, without encoding it.
    pub fn encoded_len(&self) -> usize {
        let string = |s: &str| 4 + s.len();
        let entries: usize = self
            .entries
            .iter()
            .map(|entry| {
                1 + string(&entry.path)
                    + match &entry.kind {
                        EntryKind::File(_) => 8 + 32 + 1 + 8,
                        EntryKind::Symlink { target } => string(target),
                        EntryKind::Directory => 0,
                    }
            })
            .sum();
        1 + string(&self.name) + string(&self.comment) + 8 + string(&self.creator) + 4 + entries
    }

    /// Checks every rule except the size limit.
    fn validate(&self) -> Result<(), ManifestError> {
        validate_name(&self.name).map_err(ManifestError::InvalidName)?;
        if self.entries.is_empty() {
            return Err(ManifestError::NoEntries);
        }

        // Each entry on its own, and the order of entries.
        let mut total_chunks = 0;
        for (i, entry) in self.entries.iter().enumerate() {
            let path = &entry.path;
            validate_path(path).map_err(|error| ManifestError::InvalidPath {
                path: path.clone(),
                error,
            })?;
            if let Some(previous) = i.checked_sub(1).map(|j| &self.entries[j].path) {
                if previous == path {
                    return Err(ManifestError::DuplicatePath(path.clone()));
                }
                // `str` orders by bytes.
                if previous > path {
                    return Err(ManifestError::Unsorted(path.clone()));
                }
            }
            match &entry.kind {
                EntryKind::File(file) => {
                    if file.size == 0 && file.root != leaf_hash(&[]) {
                        return Err(ManifestError::EmptyFileRoot(path.clone()));
                    }
                    total_chunks += chunk_count(file.size);
                    if total_chunks > MAX_BUNDLE_CHUNKS {
                        return Err(ManifestError::TooManyChunks);
                    }
                }
                EntryKind::Symlink { target } => {
                    validate_symlink_target(target).map_err(|error| {
                        ManifestError::InvalidSymlinkTarget {
                            path: path.clone(),
                            error,
                        }
                    })?;
                }
                EntryKind::Directory => {}
            }
        }

        let tree = PathTree::new(&self.entries)?;

        // Symlink targets, resolved step by step.
        for entry in &self.entries {
            let EntryKind::Symlink { target } = &entry.kind else {
                continue;
            };
            let via = tree
                .symlink_on_route(&entry.path, target)
                .map_err(|EscapesBundle| ManifestError::SymlinkEscapes(entry.path.clone()))?;
            if let Some(via) = via {
                return Err(ManifestError::SymlinkThroughSymlink {
                    path: entry.path.clone(),
                    via: via.to_owned(),
                });
            }
        }
        Ok(())
    }
}

/// Every path in a manifest, entries and the directories that implicitly contain them, as a trie
/// of case-folded segments. Memory is linear in the manifest's size, however deep its paths.
struct PathTree<'a> {
    /// Node 0 is the top level.
    nodes: Vec<Node<'a>>,
    /// A node's children, by parent and case-folded segment.
    children: HashMap<(usize, String), usize>,
}

#[derive(Default)]
struct Node<'a> {
    /// The path as first seen: an entry's, or an implicit directory's. `None` until seen.
    original: Option<&'a str>,
    /// The entry's kind (`None` for an implicit directory).
    kind: Option<&'a EntryKind>,
}

impl<'a> PathTree<'a> {
    /// Indexes `entries`, checking that no two paths differ only by case, that nothing is inside
    /// a file or symlink, and that directory entries are empty.
    fn new(entries: &'a [Entry]) -> Result<Self, ManifestError> {
        let mut tree = Self {
            nodes: vec![Node::default()],
            children: HashMap::new(),
        };
        for entry in entries {
            let mut node = 0;
            for segment in entry.path.split('/') {
                node = tree.child(node, segment);
            }
            let slot = &mut tree.nodes[node];
            if let Some(other) = slot.original {
                return Err(ManifestError::CaseCollision(
                    other.to_owned(),
                    entry.path.clone(),
                ));
            }
            slot.original = Some(entry.path.as_str());
            slot.kind = Some(&entry.kind);
        }
        for entry in entries {
            let mut node = 0;
            let mut ends = ancestors(&entry.path).map(str::len);
            let mut end = ends.next();
            for segment in entry.path.split('/') {
                let Some(dir_end) = end else { break };
                node = tree.child(node, segment);
                end = ends.next();
                let dir = &entry.path[..dir_end];
                let slot = &mut tree.nodes[node];
                match (slot.original, slot.kind) {
                    (None, _) => slot.original = Some(dir),
                    (Some(other), _) if other != dir => {
                        return Err(ManifestError::CaseCollision(
                            other.to_owned(),
                            dir.to_owned(),
                        ));
                    }
                    (Some(_), None) => {}
                    (Some(_), Some(EntryKind::Directory)) => {
                        return Err(ManifestError::NonEmptyDirectory(dir.to_owned()));
                    }
                    (Some(_), Some(_)) => {
                        return Err(ManifestError::InsideFileOrSymlink {
                            path: entry.path.clone(),
                            parent: dir.to_owned(),
                        });
                    }
                }
            }
        }
        Ok(tree)
    }

    /// The child of `parent` named `segment`, compared case-insensitively, made if missing.
    fn child(&mut self, parent: usize, segment: &str) -> usize {
        let next = self.nodes.len();
        let node = *self
            .children
            .entry((parent, fold_case(segment)))
            .or_insert(next);
        if node == next {
            self.nodes.push(Node::default());
        }
        node
    }

    /// The child of `parent` named `segment`, compared case-insensitively, if there is one.
    fn find(&self, parent: usize, segment: &str) -> Option<usize> {
        self.children.get(&(parent, fold_case(segment))).copied()
    }

    /// Resolves the target of the symlink at `link` one segment at a time, as
    /// [`symlink_steps`] does, and returns the path of the first symlink entry that a step
    /// reaches, compared case-insensitively.
    fn symlink_on_route(&self, link: &str, target: &str) -> Result<Option<&'a str>, EscapesBundle> {
        // The node reached after each segment; `None` once the route leaves the manifest's paths.
        let mut route: Vec<Option<usize>> = Vec::new();
        let mut node = Some(0);
        if let Some((dir, _)) = link.rsplit_once('/') {
            for segment in dir.split('/') {
                node = node.and_then(|node| self.find(node, segment));
                route.push(node);
            }
        }
        let mut via = None;
        for segment in target.split('/') {
            if segment == ".." {
                route.pop().ok_or(EscapesBundle)?;
            } else {
                let parent = route.last().copied().unwrap_or(Some(0));
                route.push(parent.and_then(|parent| self.find(parent, segment)));
            }
            if via.is_none()
                && let Some(Some(node)) = route.last()
                && let Node {
                    original: Some(original),
                    kind: Some(EntryKind::Symlink { .. }),
                } = &self.nodes[*node]
            {
                via = Some(*original);
            }
        }
        Ok(via)
    }
}

impl Encode for Entry {
    fn encode(&self, w: &mut Writer) {
        match &self.kind {
            EntryKind::File(file) => {
                w.u8(KIND_FILE);
                w.string(&self.path);
                w.u64(file.size);
                w.hash(&file.root);
                w.bool(file.executable);
                w.i64(file.modified);
            }
            EntryKind::Symlink { target } => {
                w.u8(KIND_SYMLINK);
                w.string(&self.path);
                w.string(target);
            }
            EntryKind::Directory => {
                w.u8(KIND_DIRECTORY);
                w.string(&self.path);
            }
        }
    }
}

/// Decodes an entry's fields only. Path rules are checked by [`Manifest`]'s decoding.
impl Decode for Entry {
    /// A directory: its kind and an empty path.
    const MIN_LEN: usize = 5;

    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let kind = r.u8()?;
        let path = r.string()?.to_owned();
        let kind = match kind {
            KIND_FILE => EntryKind::File(FileInfo {
                size: r.u64()?,
                root: r.hash()?,
                executable: r.bool()?,
                modified: r.i64()?,
            }),
            KIND_SYMLINK => EntryKind::Symlink {
                target: r.string()?.to_owned(),
            },
            KIND_DIRECTORY => EntryKind::Directory,
            other => return Err(DecodeError::UnknownEntryKind(other)),
        };
        Ok(Self { path, kind })
    }
}

impl Encode for Manifest {
    fn encode(&self, w: &mut Writer) {
        w.u8(MANIFEST_VERSION);
        w.string(&self.name);
        w.string(&self.comment);
        w.i64(self.created);
        w.string(&self.creator);
        w.list(&self.entries);
    }
}

/// Decodes a manifest and checks every rule, including the size limit.
impl Decode for Manifest {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let start = r.remaining();
        let version = r.u8()?;
        if version != MANIFEST_VERSION {
            return Err(DecodeError::UnknownManifestVersion(version));
        }
        let manifest = Self {
            name: r.string()?.to_owned(),
            comment: r.string()?.to_owned(),
            created: r.i64()?,
            creator: r.string()?.to_owned(),
            entries: r.list()?,
        };
        let len = start - r.remaining();
        let invalid = |error| DecodeError::InvalidManifest(Box::new(error));
        if len > MAX_MANIFEST_LEN {
            return Err(invalid(ManifestError::TooLarge(len)));
        }
        manifest.validate().map_err(invalid)?;
        Ok(manifest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(size: u64) -> FileInfo {
        FileInfo {
            size,
            root: if size == 0 {
                leaf_hash(&[])
            } else {
                Hash::ZERO
            },
            executable: false,
            modified: 0,
        }
    }

    fn manifest(entries: Vec<Entry>) -> Result<Manifest, ManifestError> {
        Manifest::new("bundle", "", 0, "test", entries)
    }

    #[test]
    fn deep_paths_cost_memory_linear_in_their_length() {
        // 200 entries of about 4 KiB each would hold hundreds of MB if every ancestor were a key.
        let entries = (0..200)
            .map(|n| {
                let deep = "a/".repeat(2040);
                Entry::file(format!("x{n:04}/{deep}f"), file(1))
            })
            .collect();
        let mut entries: Vec<Entry> = entries;
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        manifest(entries).unwrap();
    }

    #[test]
    fn needs_an_entry_and_a_valid_name() {
        assert_eq!(manifest(vec![]), Err(ManifestError::NoEntries));
        assert!(matches!(
            Manifest::new("a/b", "", 0, "", vec![Entry::directory("d")]),
            Err(ManifestError::InvalidName(_))
        ));
    }

    #[test]
    fn new_sorts_entries_by_bytes() {
        let m = manifest(vec![
            Entry::file("b", file(1)),
            Entry::file("a/z", file(1)),
            Entry::file("C", file(1)),
            Entry::file("a-", file(1)),
        ])
        .unwrap();
        let paths: Vec<_> = m.entries().iter().map(|e| e.path.as_str()).collect();
        // '-' (0x2d) sorts before '/' (0x2f).
        assert_eq!(paths, ["C", "a-", "a/z", "b"]);
        assert_eq!(m.encoded_len(), m.to_bytes().len());
    }

    #[test]
    fn duplicate_and_case_colliding_paths() {
        assert_eq!(
            manifest(vec![Entry::file("a", file(1)), Entry::file("a", file(1))]),
            Err(ManifestError::DuplicatePath("a".into()))
        );
        assert_eq!(
            manifest(vec![Entry::file("a", file(1)), Entry::file("A", file(1))]),
            Err(ManifestError::CaseCollision("A".into(), "a".into()))
        );
        // Implicit directories count too.
        assert_eq!(
            manifest(vec![
                Entry::file("A/b", file(1)),
                Entry::file("a/c", file(1))
            ]),
            Err(ManifestError::CaseCollision("A".into(), "a".into()))
        );
        assert_eq!(
            manifest(vec![
                Entry::directory("ß"),
                Entry::file("\u{1E9E}/x", file(1))
            ]),
            Err(ManifestError::CaseCollision("ß".into(), "\u{1E9E}".into()))
        );
    }

    #[test]
    fn nothing_inside_files_or_symlinks() {
        assert_eq!(
            manifest(vec![Entry::file("a", file(1)), Entry::file("a/b", file(1))]),
            Err(ManifestError::InsideFileOrSymlink {
                path: "a/b".into(),
                parent: "a".into()
            })
        );
        assert_eq!(
            manifest(vec![Entry::symlink("a", "x"), Entry::file("A/b", file(1))]),
            Err(ManifestError::CaseCollision("a".into(), "A".into()))
        );
        assert_eq!(
            manifest(vec![Entry::symlink("a", "x"), Entry::directory("a/d")]),
            Err(ManifestError::InsideFileOrSymlink {
                path: "a/d".into(),
                parent: "a".into()
            })
        );
    }

    #[test]
    fn directory_entries_must_be_empty() {
        assert_eq!(
            manifest(vec![Entry::directory("d"), Entry::file("d/f", file(1))]),
            Err(ManifestError::NonEmptyDirectory("d".into()))
        );
        assert!(manifest(vec![Entry::directory("d"), Entry::directory("e/f")]).is_ok());
    }

    #[test]
    fn empty_files_have_the_empty_root() {
        let mut info = file(0);
        info.root = Hash::ZERO;
        assert_eq!(
            manifest(vec![Entry::file("e", info)]),
            Err(ManifestError::EmptyFileRoot("e".into()))
        );
    }

    #[test]
    fn total_chunks_fit_in_32_bits() {
        let max = MAX_BUNDLE_CHUNKS * 16_384;
        assert!(manifest(vec![Entry::file("a", file(max))]).is_ok());
        assert_eq!(
            manifest(vec![Entry::file("a", file(max)), Entry::file("b", file(0))]),
            Err(ManifestError::TooManyChunks)
        );
        assert_eq!(
            manifest(vec![
                Entry::file("a", file(u64::MAX)),
                Entry::file("b", file(u64::MAX))
            ]),
            Err(ManifestError::TooManyChunks)
        );
    }

    #[test]
    fn symlink_rules() {
        let ok = |entries| manifest(entries).map(|_| ());
        assert_eq!(ok(vec![Entry::symlink("d/l", "../x")]), Ok(()));
        // Dangling targets are allowed.
        assert_eq!(ok(vec![Entry::symlink("l", "missing/x")]), Ok(()));
        assert_eq!(
            ok(vec![Entry::symlink("l", "../x")]),
            Err(ManifestError::SymlinkEscapes("l".into()))
        );
        assert!(matches!(
            ok(vec![Entry::symlink("l", "/etc/passwd")]),
            Err(ManifestError::InvalidSymlinkTarget { .. })
        ));
        assert_eq!(
            ok(vec![Entry::symlink("l", "l")]),
            Err(ManifestError::SymlinkThroughSymlink {
                path: "l".into(),
                via: "l".into()
            })
        );
        assert_eq!(
            ok(vec![Entry::symlink("a", "d"), Entry::symlink("b", "A/x")]),
            Err(ManifestError::SymlinkThroughSymlink {
                path: "b".into(),
                via: "a".into()
            })
        );
        // Passing through a symlink and back out again is still passing through it.
        assert_eq!(
            ok(vec![
                Entry::symlink("a", "d"),
                Entry::symlink("b", "a/../c")
            ]),
            Err(ManifestError::SymlinkThroughSymlink {
                path: "b".into(),
                via: "a".into()
            })
        );
    }

    #[test]
    fn decoding_rejects_bad_version_kind_order_and_trailing_bytes() {
        let m = manifest(vec![Entry::file("a", file(1)), Entry::directory("b")]).unwrap();
        let bytes = m.to_bytes();
        assert_eq!(Manifest::from_bytes(&bytes), Ok(m.clone()));

        let mut bad = bytes.clone();
        bad[0] = 2;
        assert_eq!(
            Manifest::from_bytes(&bad),
            Err(DecodeError::UnknownManifestVersion(2))
        );

        let mut bad = bytes.clone();
        let directory_kind = bytes.len() - 1 - 4 - 1;
        bad[directory_kind] = 3;
        assert_eq!(
            Manifest::from_bytes(&bad),
            Err(DecodeError::UnknownEntryKind(3))
        );

        let mut bad = bytes.clone();
        bad.push(0);
        assert_eq!(
            Manifest::from_bytes(&bad),
            Err(DecodeError::TrailingBytes(1))
        );

        // Swap the order of the entries.
        let mut w = Writer::new();
        w.u8(MANIFEST_VERSION);
        w.string("bundle");
        w.string("");
        w.i64(0);
        w.string("test");
        w.list(&[m.entries()[1].clone(), m.entries()[0].clone()]);
        assert_eq!(
            Manifest::from_bytes(&w.into_bytes()),
            Err(DecodeError::InvalidManifest(Box::new(
                ManifestError::Unsorted("a".into())
            )))
        );
    }
}
