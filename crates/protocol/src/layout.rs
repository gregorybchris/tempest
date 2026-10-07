//! Bundle-wide chunk numbering (`protocol/messages.md`, Chunk numbering).
//!
//! Chunks are numbered with one index across the whole bundle. Files are taken in manifest order,
//! and each file's chunks occupy the next consecutive indexes. A chunk's position is its index
//! minus its file's first chunk.

use crate::manifest::Manifest;
use crate::tree::{InvalidRequest, TreeNode, chunk_count};

/// Where one file's chunks lie in the bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileChunks {
    /// The file's index in [`Manifest::entries`].
    pub entry: usize,
    /// The bundle index of the file's first chunk.
    pub first_chunk: u32,
    /// The number of chunks in the file.
    pub chunk_count: u32,
}

impl FileChunks {
    /// The bundle index just past the file's last chunk.
    pub fn end(&self) -> u32 {
        self.first_chunk + self.chunk_count
    }
}

/// A chunk's file and its position within it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkLocation {
    /// The file's index in [`ChunkLayout::files`].
    pub file: usize,
    /// The chunk's position within the file.
    pub position: u32,
}

/// The chunk numbering of a bundle: where each file's chunks lie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkLayout {
    files: Vec<FileChunks>,
    chunk_count: u32,
}

impl ChunkLayout {
    /// The chunk numbering of `manifest`'s files.
    pub fn new(manifest: &Manifest) -> Self {
        let mut files = Vec::new();
        let mut next = 0u32;
        for (entry, file) in manifest.entries().iter().enumerate() {
            let Some(file) = file.as_file() else {
                continue;
            };
            // A valid manifest has at most 2^32 - 1 chunks, so neither conversion nor sum
            // overflows.
            let count = u32::try_from(chunk_count(file.size)).expect("valid manifest");
            files.push(FileChunks {
                entry,
                first_chunk: next,
                chunk_count: count,
            });
            next += count;
        }
        Self {
            files,
            chunk_count: next,
        }
    }

    /// The files' chunks, in manifest order.
    pub fn files(&self) -> &[FileChunks] {
        &self.files
    }

    /// The total number of chunks in the bundle.
    pub fn chunk_count(&self) -> u32 {
        self.chunk_count
    }

    /// The file and position of the chunk with bundle index `index`, if there is one.
    pub fn locate(&self, index: u32) -> Option<ChunkLocation> {
        if index >= self.chunk_count {
            return None;
        }
        // The first file that ends after `index`. Files are never empty, so it contains it.
        let file = self.files.partition_point(|f| f.end() <= index);
        Some(ChunkLocation {
            file,
            position: index - self.files[file].first_chunk,
        })
    }

    /// The file and tree node that a Request for `[start, start + count)` names, or why the
    /// Request is invalid.
    ///
    /// This doesn't check the provider's `max_request_chunks`.
    pub fn request_node(
        &self,
        start: u32,
        count: u32,
    ) -> Result<(usize, TreeNode), InvalidRequest> {
        let location = self.locate(start).ok_or(InvalidRequest::OutsideBundle)?;
        let file = &self.files[location.file];
        let node = TreeNode::for_range(file.chunk_count.into(), location.position, count)?;
        Ok((location.file, node))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::{Hash, leaf_hash};
    use crate::manifest::{Entry, FileInfo};

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

    fn layout() -> ChunkLayout {
        // Chunk counts 3, 1 (empty file) and 2; the symlink and directory have no chunks.
        let manifest = Manifest::new(
            "b",
            "",
            0,
            "",
            vec![
                Entry::file("a", file(40_000)),
                Entry::symlink("b", "a"),
                Entry::file("c", file(0)),
                Entry::directory("d"),
                Entry::file("e", file(16_385)),
            ],
        )
        .unwrap();
        ChunkLayout::new(&manifest)
    }

    #[test]
    fn files_take_consecutive_indexes() {
        let layout = layout();
        let spans: Vec<_> = layout
            .files()
            .iter()
            .map(|f| (f.entry, f.first_chunk, f.chunk_count))
            .collect();
        assert_eq!(spans, [(0, 0, 3), (2, 3, 1), (4, 4, 2)]);
        assert_eq!(layout.chunk_count(), 6);
    }

    #[test]
    fn locate_maps_indexes_to_positions() {
        let layout = layout();
        let at = |file, position| Some(ChunkLocation { file, position });
        assert_eq!(layout.locate(0), at(0, 0));
        assert_eq!(layout.locate(2), at(0, 2));
        assert_eq!(layout.locate(3), at(1, 0));
        assert_eq!(layout.locate(5), at(2, 1));
        assert_eq!(layout.locate(6), None);
    }

    #[test]
    fn requests_name_one_node_of_one_file() {
        let layout = layout();
        assert_eq!(
            layout.request_node(0, 4),
            Ok((0, TreeNode { level: 2, index: 0 }))
        );
        assert_eq!(
            layout.request_node(2, 2),
            Ok((0, TreeNode { level: 1, index: 1 }))
        );
        assert_eq!(
            layout.request_node(3, 1),
            Ok((1, TreeNode { level: 0, index: 0 }))
        );
        assert_eq!(
            layout.request_node(3, 2),
            Err(InvalidRequest::PastEndOfTree)
        );
        assert_eq!(layout.request_node(5, 2), Err(InvalidRequest::Misaligned));
        assert_eq!(
            layout.request_node(6, 1),
            Err(InvalidRequest::OutsideBundle)
        );
    }
}
