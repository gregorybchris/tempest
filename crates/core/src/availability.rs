//! How many connected peers have each chunk, indexed for rarest-first selection.

use crate::chunks::ChunkSet;

/// For each chunk, how many connected peers have it, with the chunks grouped by that count.
///
/// Rarest-first wants "a random chunk among the rarest the provider has that this peer needs".
/// Scanning every chunk for that on each pick is too slow for large bundles across many peers,
/// so chunks are kept in one set per holder count. A pick walks the counts upwards and
/// intersects each level with the needed and provider sets, word by word.
#[derive(Debug, Clone)]
pub struct Availability {
    counts: Vec<u32>,
    /// `levels[n]` holds the chunks exactly `n` connected peers have. Levels are added as counts
    /// grow and kept once added.
    levels: Vec<ChunkSet>,
}

impl Availability {
    /// The index for a bundle of `capacity` chunks, none of which any peer has.
    pub fn new(capacity: u32) -> Self {
        Self {
            counts: vec![0; capacity as usize],
            levels: vec![ChunkSet::full(capacity)],
        }
    }

    /// How many connected peers have `chunk`.
    pub fn count(&self, chunk: u32) -> u32 {
        self.counts[chunk as usize]
    }

    /// Records one more peer having `chunk`.
    pub fn increment(&mut self, chunk: u32) {
        let count = &mut self.counts[chunk as usize];
        self.levels[*count as usize].remove(chunk);
        *count += 1;
        let level = *count as usize;
        if level == self.levels.len() {
            self.levels.push(ChunkSet::new(self.counts.len() as u32));
        }
        self.levels[level].insert(chunk);
    }

    /// Records one fewer peer having `chunk`.
    ///
    /// # Panics
    ///
    /// If no peer has it.
    pub fn decrement(&mut self, chunk: u32) {
        let count = &mut self.counts[chunk as usize];
        assert!(*count > 0, "chunk {chunk} has no holders");
        self.levels[*count as usize].remove(chunk);
        *count -= 1;
        self.levels[*count as usize].insert(chunk);
    }

    /// The chunks exactly `holders` connected peers have, if any chunk has ever had that many.
    pub fn level(&self, holders: u32) -> Option<&ChunkSet> {
        self.levels.get(holders as usize)
    }

    /// Each holder count with the set of chunks that have it, rarest first.
    pub fn levels(&self) -> impl Iterator<Item = (u32, &ChunkSet)> {
        self.levels
            .iter()
            .enumerate()
            .map(|(n, set)| (n as u32, set))
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    proptest! {
        #[test]
        fn levels_partition_the_chunks_by_count(
            ops in proptest::collection::vec((any::<bool>(), 0..40u32), 0..300),
        ) {
            let mut index = Availability::new(40);
            let mut model = [0u32; 40];
            for (up, chunk) in ops {
                if up {
                    index.increment(chunk);
                    model[chunk as usize] += 1;
                } else if model[chunk as usize] > 0 {
                    index.decrement(chunk);
                    model[chunk as usize] -= 1;
                }
            }
            for (chunk, &count) in model.iter().enumerate() {
                prop_assert_eq!(index.count(chunk as u32), count);
                for (level, set) in index.levels() {
                    prop_assert_eq!(set.contains(chunk as u32), level == count);
                }
            }
        }
    }
}
