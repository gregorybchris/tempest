//! Chunk selection: which chunk to request next from a provider (`specs/strategies.md`, D-059).
//!
//! A selector picks one chunk. The session then grows it into the largest aligned Request around
//! it that the provider can serve and that holds only chunks still needed.

use std::fmt;

use rand::Rng as _;
use rand::seq::IteratorRandom;

use crate::Rng;
use crate::chunks::ChunkSet;
use crate::event::SelectReason;

/// What a selector knows when choosing a chunk to request from one provider.
#[derive(Debug, Clone, Copy)]
pub struct SelectContext<'a> {
    /// Chunks this peer has.
    pub have: &'a ChunkSet,
    /// Chunks this peer lacks and hasn't requested.
    pub needed: &'a ChunkSet,
    /// Chunks the provider has.
    pub provider: &'a ChunkSet,
    /// For each chunk, how many connected peers have it.
    pub availability: &'a [u32],
}

impl SelectContext<'_> {
    /// The chunks that may be requested from the provider: needed, and held by the provider.
    pub fn candidates(&self) -> impl Iterator<Item = u32> + '_ {
        self.needed.intersection(self.provider)
    }
}

/// A chunk selection strategy.
pub trait ChunkSelector: fmt::Debug + Send {
    /// Picks a chunk among [`SelectContext::candidates`], or `None` if there are none.
    fn select(&mut self, ctx: &SelectContext<'_>, rng: &mut Rng) -> Option<(u32, SelectReason)>;
}

/// Rarest-first, with the first few chunks picked at random (the default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RarestFirst {
    /// How many chunks this peer must have before it switches from random to rarest-first.
    pub random_first: u32,
}

impl ChunkSelector for RarestFirst {
    fn select(&mut self, ctx: &SelectContext<'_>, rng: &mut Rng) -> Option<(u32, SelectReason)> {
        if ctx.have.len() < self.random_first {
            let chunk = ctx.candidates().choose(rng)?;
            return Some((chunk, SelectReason::RandomFirst));
        }
        // Reservoir sampling among the chunks with the fewest holders breaks ties uniformly.
        let mut best: Option<(u32, u32)> = None;
        let mut ties = 0u32;
        for chunk in ctx.candidates() {
            let holders = ctx.availability[chunk as usize];
            match best {
                Some((_, fewest)) if holders > fewest => {}
                Some((_, fewest)) if holders == fewest => {
                    ties += 1;
                    if rng.random_range(0..ties) == 0 {
                        best = Some((chunk, holders));
                    }
                }
                _ => {
                    best = Some((chunk, holders));
                    ties = 1;
                }
            }
        }
        best.map(|(chunk, holders)| (chunk, SelectReason::Rarest { holders }))
    }
}

/// Any needed chunk, at random.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Random;

impl ChunkSelector for Random {
    fn select(&mut self, ctx: &SelectContext<'_>, rng: &mut Rng) -> Option<(u32, SelectReason)> {
        let chunk = ctx.candidates().choose(rng)?;
        Some((chunk, SelectReason::Random))
    }
}

/// The lowest-numbered needed chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sequential;

impl ChunkSelector for Sequential {
    fn select(&mut self, ctx: &SelectContext<'_>, _rng: &mut Rng) -> Option<(u32, SelectReason)> {
        let chunk = ctx.candidates().next()?;
        Some((chunk, SelectReason::Sequential))
    }
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;

    use super::*;

    fn set(capacity: u32, chunks: &[u32]) -> ChunkSet {
        let mut set = ChunkSet::new(capacity);
        chunks.iter().for_each(|&c| {
            set.insert(c);
        });
        set
    }

    #[test]
    fn rarest_first_picks_among_the_rarest() {
        let have = set(6, &[0]);
        let needed = set(6, &[1, 2, 3, 4, 5]);
        let provider = set(6, &[0, 1, 2, 3, 4]);
        let availability = [5, 3, 1, 2, 1, 0];
        let ctx = SelectContext {
            have: &have,
            needed: &needed,
            provider: &provider,
            availability: &availability,
        };
        let mut selector = RarestFirst { random_first: 0 };
        let mut rng = Rng::seed_from_u64(1);
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..100 {
            let (chunk, reason) = selector.select(&ctx, &mut rng).unwrap();
            assert_eq!(reason, SelectReason::Rarest { holders: 1 });
            seen.insert(chunk);
        }
        // Chunk 5 is rarer but the provider lacks it.
        assert_eq!(seen.into_iter().collect::<Vec<_>>(), vec![2, 4]);
    }

    #[test]
    fn rarest_first_starts_at_random() {
        let empty = ChunkSet::new(4);
        let all = ChunkSet::full(4);
        let ctx = SelectContext {
            have: &empty,
            needed: &all,
            provider: &all,
            availability: &[1, 1, 1, 1],
        };
        let mut rng = Rng::seed_from_u64(1);
        let (_, reason) = RarestFirst { random_first: 1 }
            .select(&ctx, &mut rng)
            .unwrap();
        assert_eq!(reason, SelectReason::RandomFirst);
    }

    #[test]
    fn sequential_and_random_respect_candidates() {
        let have = ChunkSet::new(8);
        let needed = set(8, &[3, 5, 6]);
        let provider = set(8, &[1, 5, 6]);
        let ctx = SelectContext {
            have: &have,
            needed: &needed,
            provider: &provider,
            availability: &[0; 8],
        };
        let mut rng = Rng::seed_from_u64(1);
        assert_eq!(Sequential.select(&ctx, &mut rng).unwrap().0, 5);
        for _ in 0..20 {
            assert!([5, 6].contains(&Random.select(&ctx, &mut rng).unwrap().0));
        }
        let none = ChunkSet::new(8);
        let ctx = SelectContext {
            provider: &none,
            ..ctx
        };
        assert_eq!(Random.select(&ctx, &mut rng), None);
    }
}
