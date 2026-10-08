//! Chunk ranges and canonical range lists (`protocol/messages.md`, Common structures).

use crate::wire::{Decode, DecodeError, Encode, Reader, Writer};

/// A half-open range of chunk indexes, `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChunkRange {
    /// The first chunk index, inclusive.
    pub start: u32,
    /// The last chunk index, exclusive.
    pub end: u32,
}

impl ChunkRange {
    /// The number of chunks in the range.
    pub fn len(&self) -> u32 {
        self.end.saturating_sub(self.start)
    }

    /// Whether the range has no chunks.
    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }

    /// Whether the range contains chunk `index`.
    pub fn contains(&self, index: u32) -> bool {
        self.start <= index && index < self.end
    }
}

impl Encode for ChunkRange {
    fn encode(&self, w: &mut Writer) {
        w.u32(self.start);
        w.u32(self.end);
    }
}

impl Decode for ChunkRange {
    const MIN_LEN: usize = 8;

    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            start: r.u32()?,
            end: r.u32()?,
        })
    }
}

/// A set of chunks, as a list of ranges in its one canonical form.
///
/// The ranges are sorted, non-empty, and neither overlap nor touch. Whether they lie within a
/// bundle's chunks can only be checked once the manifest is known, with [`RangeList::fits`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Hash)]
pub struct RangeList(Vec<ChunkRange>);

impl RangeList {
    /// The empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Wraps ranges that must already be in canonical form.
    pub fn from_ranges(ranges: Vec<ChunkRange>) -> Result<Self, DecodeError> {
        let canonical = ranges.iter().all(|r| !r.is_empty())
            && ranges.windows(2).all(|pair| pair[0].end < pair[1].start);
        if canonical {
            Ok(Self(ranges))
        } else {
            Err(DecodeError::InvalidRangeList)
        }
    }

    /// Builds the canonical list for a set of chunk indexes, given in any order.
    pub fn from_chunks(chunks: impl IntoIterator<Item = u32>) -> Self {
        let mut chunks: Vec<u32> = chunks.into_iter().collect();
        chunks.sort_unstable();
        chunks.dedup();

        let mut ranges: Vec<ChunkRange> = Vec::new();
        let mut chunks = chunks.into_iter().peekable();
        while let Some(start) = chunks.next() {
            let mut last = start;
            while chunks.next_if(|&next| next == last + 1).is_some() {
                last += 1;
            }
            // A range can't express index u32::MAX, since its end would not fit in a u32. The
            // protocol has no such chunk: a bundle has at most 2³² − 1 chunks.
            let end = last
                .checked_add(1)
                .expect("chunk index u32::MAX does not exist");
            ranges.push(ChunkRange { start, end });
        }
        Self(ranges)
    }

    /// The ranges, in order.
    pub fn ranges(&self) -> &[ChunkRange] {
        &self.0
    }

    /// Whether the set is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The number of chunks in the set.
    pub fn chunk_count(&self) -> u64 {
        self.0.iter().map(|r| u64::from(r.len())).sum()
    }

    /// Whether the set contains chunk `index`.
    pub fn contains(&self, index: u32) -> bool {
        let after = self.0.partition_point(|r| r.start <= index);
        after > 0 && self.0[after - 1].contains(index)
    }

    /// Whether every range lies within a bundle of `chunk_count` chunks.
    pub fn fits(&self, chunk_count: u32) -> bool {
        self.0.last().is_none_or(|r| r.end <= chunk_count)
    }

    /// Every chunk index in the set, in ascending order.
    pub fn chunks(&self) -> impl Iterator<Item = u32> + '_ {
        self.0.iter().flat_map(|r| r.start..r.end)
    }
}

impl Encode for RangeList {
    fn encode(&self, w: &mut Writer) {
        w.list(&self.0);
    }
}

impl Decode for RangeList {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Self::from_ranges(r.list()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(start: u32, end: u32) -> ChunkRange {
        ChunkRange { start, end }
    }

    #[test]
    fn from_chunks_merges_runs() {
        let list = RangeList::from_chunks([5, 1, 2, 3, 9, 2]);
        assert_eq!(list.ranges(), &[range(1, 4), range(5, 6), range(9, 10)]);
        assert_eq!(list.chunk_count(), 5);
        assert!(list.contains(3) && list.contains(9));
        assert!(!list.contains(4) && !list.contains(0) && !list.contains(10));
    }

    #[test]
    fn non_canonical_lists_are_rejected() {
        for ranges in [
            vec![range(3, 3)],
            vec![range(4, 2)],
            vec![range(0, 2), range(2, 4)],
            vec![range(0, 3), range(2, 4)],
            vec![range(5, 6), range(0, 2)],
        ] {
            assert_eq!(
                RangeList::from_ranges(ranges),
                Err(DecodeError::InvalidRangeList)
            );
        }
    }

    #[test]
    fn fits_checks_the_last_range() {
        let list = RangeList::from_chunks([0, 7]);
        assert!(list.fits(8));
        assert!(!list.fits(7));
        assert!(RangeList::new().fits(0));
    }
}
