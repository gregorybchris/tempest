//! What the session knows about the bundle's chunks once it has the manifest.

use std::ops::Range;
use std::sync::Arc;

use tempest_protocol::tree::{TreeNode, data_len};
use tempest_protocol::{ChunkLayout, Manifest, RangeList};

use crate::chunks::ChunkSet;

#[derive(Debug)]
pub(super) struct Bundle {
    pub manifest: Arc<Manifest>,
    pub layout: ChunkLayout,
    /// Chunks this peer has.
    pub have: ChunkSet,
    /// Chunks this peer lacks and hasn't requested.
    pub needed: ChunkSet,
    /// For each chunk, how many open connections' peers have it.
    pub availability: Vec<u32>,
}

/// `have` reaches past the bundle's last chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct OutOfRange;

impl Bundle {
    /// The bundle with `have` already stored. Every empty file's chunk is added, since a peer
    /// with the manifest has it (`protocol/messages.md`, Have).
    pub fn new(manifest: Arc<Manifest>, have: &RangeList) -> Result<Self, OutOfRange> {
        let layout = ChunkLayout::new(&manifest);
        let count = layout.chunk_count();
        if !have.fits(count) {
            return Err(OutOfRange);
        }
        let mut have = ChunkSet::from_ranges(count, have);
        for file in layout.files() {
            let size = manifest.entries()[file.entry].as_file().map(|f| f.size);
            if size == Some(0) {
                have.insert(file.first_chunk);
            }
        }
        let mut needed = ChunkSet::full(count);
        for chunk in have.iter() {
            needed.remove(chunk);
        }
        Ok(Self {
            manifest,
            layout,
            have,
            needed,
            availability: vec![0; count as usize],
        })
    }

    /// The bundle chunks a valid Request for `[start, start + count)` covers: the range, cut at
    /// the end of its file.
    pub fn span(&self, start: u32, count: u32) -> Range<u32> {
        let location = self.layout.locate(start).expect("valid request");
        let file = self.layout.files()[location.file];
        let end = (u64::from(start) + u64::from(count)).min(file.end().into());
        start..end as u32
    }

    /// The number of payload bytes in Data answering a valid Request for `[start, start + count)`.
    pub fn data_len(&self, start: u32, count: u32) -> u64 {
        let location = self.layout.locate(start).expect("valid request");
        let file = self.layout.files()[location.file];
        let size = self.manifest.entries()[file.entry]
            .as_file()
            .expect("layout files are files")
            .size;
        data_len(size, location.position, count)
    }

    /// The largest Request of at most `max_count` chunks that contains `chunk` and holds only
    /// chunks that are needed and that `provider` has. `chunk` itself must be one.
    pub fn request_around(&self, chunk: u32, max_count: u32, provider: &ChunkSet) -> (u32, u32) {
        let location = self.layout.locate(chunk).expect("chunk in the bundle");
        let file = self.layout.files()[location.file];
        let mut count = max_count;
        loop {
            let position = location.position - location.position % count;
            let start = file.first_chunk + position;
            let fits = TreeNode::for_range(file.chunk_count.into(), position, count).is_ok();
            if fits
                && self
                    .span(start, count)
                    .all(|i| self.needed.contains(i) && provider.contains(i))
            {
                return (start, count);
            }
            debug_assert!(count > 1, "a single needed chunk is always requestable");
            count /= 2;
        }
    }

    /// Marks a request's chunks as requested.
    pub fn mark_requested(&mut self, start: u32, count: u32) {
        for chunk in self.span(start, count) {
            self.needed.remove(chunk);
        }
    }

    /// Makes a request's chunks that are still missing needed again.
    pub fn release(&mut self, start: u32, count: u32) {
        for chunk in self.span(start, count) {
            if !self.have.contains(chunk) {
                self.needed.insert(chunk);
            }
        }
    }

    pub fn is_complete(&self) -> bool {
        self.have.is_full()
    }
}
