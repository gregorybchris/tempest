//! What the session knows about the bundle's chunks once it has the manifest.

use std::ops::Range;
use std::sync::Arc;

use tempest_protocol::tree::{TreeNode, data_len};
use tempest_protocol::{ChunkLayout, Manifest, RangeList};

use crate::availability::Availability;
use crate::chunks::ChunkSet;

#[derive(Debug)]
pub(super) struct Bundle {
    pub manifest: Arc<Manifest>,
    pub layout: ChunkLayout,
    /// Chunks this peer has.
    pub have: ChunkSet,
    /// Chunks this peer lacks and has no open request for.
    pub needed: ChunkSet,
    /// For each chunk, how many open requests ask for it. Above 1 only during endgame.
    pub requests: Vec<u32>,
    /// How many open connections' peers have each chunk.
    pub availability: Availability,
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
        for chunk in empty_file_chunks(&manifest, &layout) {
            have.insert(chunk);
        }
        let mut needed = ChunkSet::new(count);
        for chunk in have.iter_missing().collect::<Vec<_>>() {
            needed.insert(chunk);
        }
        Ok(Self {
            manifest,
            layout,
            have,
            needed,
            requests: vec![0; count as usize],
            availability: Availability::new(count),
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
    /// chunks that pass `accept`. `chunk` itself must pass.
    pub fn request_around(
        &self,
        chunk: u32,
        max_count: u32,
        accept: impl Fn(u32) -> bool,
    ) -> (u32, u32) {
        let location = self.layout.locate(chunk).expect("chunk in the bundle");
        let file = self.layout.files()[location.file];
        let mut count = max_count;
        loop {
            let position = location.position - location.position % count;
            let start = file.first_chunk + position;
            let fits = TreeNode::for_range(file.chunk_count.into(), position, count).is_ok();
            if fits && self.span(start, count).all(&accept) {
                return (start, count);
            }
            debug_assert!(count > 1, "a single accepted chunk is always requestable");
            count /= 2;
        }
    }

    /// Counts a new request for a range's chunks.
    pub fn mark_requested(&mut self, start: u32, count: u32) {
        for chunk in self.span(start, count) {
            self.requests[chunk as usize] += 1;
            self.needed.remove(chunk);
        }
    }

    /// Uncounts a request that ended, however it ended. Chunks that are still missing and no
    /// longer requested are needed again.
    pub fn release(&mut self, start: u32, count: u32) {
        for chunk in self.span(start, count) {
            let requests = &mut self.requests[chunk as usize];
            *requests -= 1;
            if *requests == 0 && !self.have.contains(chunk) {
                self.needed.insert(chunk);
            }
        }
    }

    /// Stores a range's chunks, returning the ones that are new and the number of payload bytes
    /// of the ones already stored.
    pub fn store(&mut self, start: u32, count: u32) -> (Vec<u32>, u64) {
        let mut added = Vec::new();
        let mut duplicate = 0;
        for chunk in self.span(start, count) {
            if self.have.insert(chunk) {
                added.push(chunk);
            } else {
                duplicate += self.data_len(chunk, 1);
            }
        }
        (added, duplicate)
    }

    pub fn is_complete(&self) -> bool {
        self.have.is_full()
    }

    /// Whether every missing chunk has an open request, so that endgame applies (D-061).
    pub fn in_endgame(&self) -> bool {
        !self.is_complete() && self.needed.is_empty()
    }
}

/// The chunk of each empty file, which every peer with the manifest has.
pub(super) fn empty_file_chunks<'a>(
    manifest: &'a Manifest,
    layout: &'a ChunkLayout,
) -> impl Iterator<Item = u32> + 'a {
    layout.files().iter().filter_map(|file| {
        let size = manifest.entries()[file.entry].as_file().map(|f| f.size);
        (size == Some(0)).then_some(file.first_chunk)
    })
}
