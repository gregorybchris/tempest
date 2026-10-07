//! Sets of chunk indexes within a bundle.

use tempest_protocol::{ChunkRange, RangeList};

/// A set of chunk indexes below a fixed bundle chunk count, stored as a bitmap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkSet {
    words: Vec<u64>,
    capacity: u32,
    len: u32,
}

impl ChunkSet {
    /// The empty set for a bundle of `capacity` chunks.
    pub fn new(capacity: u32) -> Self {
        Self {
            words: vec![0; capacity.div_ceil(64) as usize],
            capacity,
            len: 0,
        }
    }

    /// The set of every chunk of a bundle of `capacity` chunks.
    pub fn full(capacity: u32) -> Self {
        let mut set = Self::new(capacity);
        set.insert_range(ChunkRange {
            start: 0,
            end: capacity,
        });
        set
    }

    /// The set of `ranges`, which must fit within `capacity` chunks.
    ///
    /// # Panics
    ///
    /// If a range ends past `capacity`.
    pub fn from_ranges(capacity: u32, ranges: &RangeList) -> Self {
        let mut set = Self::new(capacity);
        for &range in ranges.ranges() {
            set.insert_range(range);
        }
        set
    }

    /// The bundle's chunk count: every index in the set is below it.
    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    /// The number of chunks in the set.
    pub fn len(&self) -> u32 {
        self.len
    }

    /// Whether the set is empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Whether the set holds every chunk below its capacity.
    pub fn is_full(&self) -> bool {
        self.len == self.capacity
    }

    /// Whether the set contains `index`. Indexes at or past the capacity are never contained.
    pub fn contains(&self, index: u32) -> bool {
        index < self.capacity && self.words[word(index)] & bit(index) != 0
    }

    /// Whether the set contains every index in `range`.
    pub fn contains_range(&self, range: ChunkRange) -> bool {
        range.end <= self.capacity && (range.start..range.end).all(|i| self.contains(i))
    }

    /// Adds `index`, returning whether it was absent.
    ///
    /// # Panics
    ///
    /// If `index` is at or past the capacity.
    pub fn insert(&mut self, index: u32) -> bool {
        assert!(index < self.capacity, "chunk {index} out of range");
        let (w, b) = (word(index), bit(index));
        let absent = self.words[w] & b == 0;
        self.words[w] |= b;
        self.len += u32::from(absent);
        absent
    }

    /// Removes `index`, returning whether it was present.
    pub fn remove(&mut self, index: u32) -> bool {
        if !self.contains(index) {
            return false;
        }
        self.words[word(index)] &= !bit(index);
        self.len -= 1;
        true
    }

    /// Adds every index in `range`.
    ///
    /// # Panics
    ///
    /// If the range ends past the capacity.
    pub fn insert_range(&mut self, range: ChunkRange) {
        for index in range.start..range.end {
            self.insert(index);
        }
    }

    /// The indexes in the set, in increasing order.
    pub fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        self.words.iter().enumerate().flat_map(|(w, &bits)| {
            let base = w as u32 * 64;
            BitIter(bits).map(move |b| base + b)
        })
    }

    /// The indexes in both `self` and `other`, in increasing order.
    ///
    /// Both sets must have the same capacity.
    pub fn intersection<'a>(&'a self, other: &'a ChunkSet) -> impl Iterator<Item = u32> + 'a {
        debug_assert_eq!(self.capacity, other.capacity);
        self.words
            .iter()
            .zip(&other.words)
            .enumerate()
            .flat_map(|(w, (&a, &b))| {
                let base = w as u32 * 64;
                BitIter(a & b).map(move |i| base + i)
            })
    }

    /// The set as a canonical range list.
    pub fn to_ranges(&self) -> RangeList {
        let mut ranges = Vec::new();
        let mut current: Option<ChunkRange> = None;
        for index in self.iter() {
            match &mut current {
                Some(range) if range.end == index => range.end += 1,
                _ => {
                    ranges.extend(current.take());
                    current = Some(ChunkRange {
                        start: index,
                        end: index + 1,
                    });
                }
            }
        }
        ranges.extend(current);
        RangeList::from_ranges(ranges).expect("ranges built in canonical form")
    }
}

fn word(index: u32) -> usize {
    (index / 64) as usize
}

fn bit(index: u32) -> u64 {
    1 << (index % 64)
}

/// The positions of the set bits in a word, lowest first.
struct BitIter(u64);

impl Iterator for BitIter {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        if self.0 == 0 {
            return None;
        }
        let position = self.0.trailing_zeros();
        self.0 &= self.0 - 1;
        Some(position)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use proptest::prelude::*;

    use super::*;

    proptest! {
        #[test]
        fn behaves_like_a_btreeset(
            capacity in 0..300u32,
            ops in proptest::collection::vec((any::<bool>(), 0..300u32), 0..200),
        ) {
            let mut set = ChunkSet::new(capacity);
            let mut model = BTreeSet::new();
            for (insert, index) in ops {
                if index >= capacity {
                    prop_assert!(!set.remove(index));
                    continue;
                }
                if insert {
                    prop_assert_eq!(set.insert(index), model.insert(index));
                } else {
                    prop_assert_eq!(set.remove(index), model.remove(&index));
                }
            }
            prop_assert_eq!(set.len() as usize, model.len());
            prop_assert_eq!(set.iter().collect::<BTreeSet<_>>(), model.clone());
            let ranges = set.to_ranges();
            prop_assert_eq!(ranges.chunks().collect::<BTreeSet<_>>(), model);
            prop_assert_eq!(ChunkSet::from_ranges(capacity, &ranges), set);
        }

        #[test]
        fn intersection_matches_the_model(
            a in proptest::collection::btree_set(0..200u32, 0..100),
            b in proptest::collection::btree_set(0..200u32, 0..100),
        ) {
            let set = |s: &BTreeSet<u32>| {
                let mut set = ChunkSet::new(200);
                s.iter().for_each(|&i| { set.insert(i); });
                set
            };
            let expected: Vec<u32> = a.intersection(&b).copied().collect();
            prop_assert_eq!(set(&a).intersection(&set(&b)).collect::<Vec<_>>(), expected);
        }
    }

    #[test]
    fn full_set() {
        let set = ChunkSet::full(70);
        assert!(set.is_full());
        assert_eq!(set.len(), 70);
        assert!(!set.contains(70));
        assert_eq!(
            set.to_ranges().ranges(),
            &[ChunkRange { start: 0, end: 70 }]
        );
        assert!(ChunkSet::full(0).is_full());
    }
}
