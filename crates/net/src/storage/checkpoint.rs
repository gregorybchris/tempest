//! What a bundle's storage knows, saved so a later run can resume without hashing every file.

use std::fs::Metadata;
use std::time::UNIX_EPOCH;

use tempest_protocol::tree::TreeNode;
use tempest_protocol::wire::{Decode, DecodeError, Encode, Reader, Writer};
use tempest_protocol::{ChunkLayout, ChunkRange, Hash, RangeList};

/// The encoding's version, so a future change can tell old checkpoints apart.
const VERSION: u8 = 1;

/// A snapshot of a bundle's storage: for each file, the chunks stored and synced, what is known
/// of its chunk tree, and its size and modification time when the snapshot was taken.
///
/// [`Storage::resume`](super::Storage::resume) trusts a file's chunks only while its size and
/// modification time are unchanged, and checks them against the saved leaf hashes otherwise.
/// The tree itself is checked against the manifest's root, so a corrupt checkpoint is detected
/// rather than believed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub(super) files: Vec<FileState>,
}

/// What a checkpoint records about one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FileState {
    /// The file's size on disk.
    pub size: u64,
    /// Its modification time on disk, as seconds and nanoseconds since the Unix epoch.
    pub modified: Option<(i64, u32)>,
    /// The chunks stored, as positions within the file.
    pub have: RangeList,
    /// Its known tree nodes, as [`FileTree::known_nodes`](super::tree::FileTree::known_nodes)
    /// lists them.
    pub nodes: Vec<(TreeNode, Hash)>,
}

impl Checkpoint {
    /// The number of chunks stored.
    pub fn chunks_stored(&self) -> u64 {
        self.files.iter().map(|file| file.have.chunk_count()).sum()
    }

    /// The chunks stored, numbered across the bundle laid out as `layout`. Files the checkpoint
    /// doesn't cover, or chunks past a file's end, count as missing.
    pub fn have(&self, layout: &ChunkLayout) -> RangeList {
        let mut ranges: Vec<ChunkRange> = Vec::new();
        for (file, span) in self.files.iter().zip(layout.files()) {
            for range in file.have.ranges() {
                let end = range.end.min(span.chunk_count);
                if range.start >= end {
                    break;
                }
                let (start, end) = (span.first_chunk + range.start, span.first_chunk + end);
                match ranges.last_mut() {
                    // A file's last chunk and the next file's first join up.
                    Some(last) if last.end == start => last.end = end,
                    _ => ranges.push(ChunkRange { start, end }),
                }
            }
        }
        RangeList::from_ranges(ranges).expect("ranges built in order, apart and nonempty")
    }

    /// Encodes the checkpoint for saving.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u8(VERSION);
        w.list(&self.files);
        w.into_bytes()
    }

    /// Decodes a saved checkpoint. Its contents are checked against the manifest only when it
    /// is used.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CheckpointError> {
        let mut r = Reader::new(bytes);
        let version = r.u8()?;
        if version != VERSION {
            return Err(CheckpointError::UnknownVersion(version));
        }
        let files = r.list()?;
        r.finish()?;
        Ok(Self { files })
    }
}

/// Why saved bytes are not a checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CheckpointError {
    /// The bytes don't decode.
    #[error("not a storage checkpoint: {0}")]
    Decode(#[from] DecodeError),
    /// The checkpoint was saved by a version of Tempest that encodes them differently.
    #[error("unknown storage checkpoint version {0}")]
    UnknownVersion(u8),
}

impl Encode for FileState {
    fn encode(&self, w: &mut Writer) {
        w.u64(self.size);
        match self.modified {
            Some((seconds, nanos)) => {
                w.bool(true);
                w.i64(seconds);
                w.u32(nanos);
            }
            None => w.bool(false),
        }
        self.have.encode(w);
        let nodes: Vec<Node> = self
            .nodes
            .iter()
            .map(|&(node, hash)| Node(node, hash))
            .collect();
        w.list(&nodes);
    }
}

impl Decode for FileState {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let size = r.u64()?;
        let modified = match r.bool()? {
            true => Some((r.i64()?, r.u32()?)),
            false => None,
        };
        let have = RangeList::decode(r)?;
        let nodes = r.list::<Node>()?.into_iter().map(|n| (n.0, n.1)).collect();
        Ok(Self {
            size,
            modified,
            have,
            nodes,
        })
    }
}

/// A tree node and its hash, as encoded in a checkpoint.
struct Node(TreeNode, Hash);

impl Encode for Node {
    fn encode(&self, w: &mut Writer) {
        // Levels are at most 64, as a file has fewer than 2^64 chunks.
        w.u8(self.0.level as u8);
        w.u64(self.0.index);
        w.hash(&self.1);
    }
}

impl Decode for Node {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let level = r.u8()?.into();
        let index = r.u64()?;
        Ok(Self(TreeNode { level, index }, r.hash()?))
    }
}

/// A file's modification time as recorded in a checkpoint.
pub(super) fn modified(metadata: &Metadata) -> Option<(i64, u32)> {
    let since = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some((i64::try_from(since.as_secs()).ok()?, since.subsec_nanos()))
}

/// The ranges of `have` within `span`, as positions within the file `span` covers.
pub(super) fn file_ranges(have: &RangeList, span: ChunkRange) -> RangeList {
    RangeList::from_ranges(
        have.ranges()
            .iter()
            .filter_map(|range| {
                let start = range.start.max(span.start);
                let end = range.end.min(span.end);
                (start < end).then(|| ChunkRange {
                    start: start - span.start,
                    end: end - span.start,
                })
            })
            .collect(),
    )
    .expect("clipping sorted, disjoint ranges keeps them so")
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use tempest_protocol::hash::leaf_hash;

    use super::*;

    fn file_entry(path: &str, size: u64) -> tempest_protocol::manifest::Entry {
        let info = tempest_protocol::manifest::FileInfo {
            size,
            root: leaf_hash(path.as_bytes()),
            executable: false,
            modified: 0,
        };
        tempest_protocol::manifest::Entry::file(path, info)
    }

    fn state(seed: u8) -> FileState {
        FileState {
            size: 40_000,
            modified: Some((1_700_000_000, 5)),
            have: RangeList::from_chunks([0, 1]),
            nodes: vec![
                (TreeNode { level: 0, index: 0 }, leaf_hash(&[seed])),
                (TreeNode { level: 1, index: 1 }, leaf_hash(&[seed, 1])),
            ],
        }
    }

    #[test]
    fn have_numbers_chunks_across_the_bundle() {
        let manifest = tempest_protocol::Manifest::new(
            "bundle",
            "",
            0,
            "",
            vec![
                file_entry("a", 3 * 16384),
                file_entry("b", 2 * 16384),
                file_entry("c", 4 * 16384),
            ],
        )
        .unwrap();
        let layout = ChunkLayout::new(&manifest);
        let with = |have: &[u32]| FileState {
            have: RangeList::from_chunks(have.iter().copied()),
            ..state(0)
        };
        let checkpoint = Checkpoint {
            // The second file's chunk 5 is past its end, and the third file isn't covered.
            files: vec![with(&[1, 2]), with(&[0, 1, 5])],
        };
        assert_eq!(
            checkpoint.have(&layout),
            RangeList::from_chunks([1, 2, 3, 4])
        );
    }

    #[test]
    fn a_checkpoint_round_trips() {
        let checkpoint = Checkpoint {
            files: vec![
                state(1),
                FileState {
                    modified: None,
                    ..state(2)
                },
            ],
        };
        let bytes = checkpoint.to_bytes();
        assert_eq!(Checkpoint::from_bytes(&bytes), Ok(checkpoint.clone()));
        assert_eq!(checkpoint.chunks_stored(), 4);
        assert!(Checkpoint::from_bytes(&bytes[..bytes.len() - 1]).is_err());
        let mut other_version = bytes;
        other_version[0] = 9;
        assert!(Checkpoint::from_bytes(&other_version).is_err());
    }

    #[test]
    fn ranges_are_clipped_to_the_file() {
        let have = RangeList::from_chunks([1, 2, 3, 7, 8, 12]);
        let span = ChunkRange { start: 2, end: 8 };
        assert_eq!(file_ranges(&have, span), RangeList::from_chunks([0, 1, 5]));
    }

    proptest! {
        /// Whatever a damaged database holds, decoding fails cleanly or gives a checkpoint that
        /// encodes back to the same bytes.
        #[test]
        fn decoding_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..300)) {
            if let Ok(checkpoint) = Checkpoint::from_bytes(&bytes) {
                prop_assert_eq!(checkpoint.to_bytes(), bytes);
            }
        }
    }
}
