//! Chunk selection: which chunk to request next from a provider (`specs/strategies.md`, D-059).
//!
//! A selector picks one chunk. The session then grows it into the largest aligned Request around
//! it that the provider can serve and that holds only chunks still needed. Selection covers
//! chunks nobody has been asked for yet; endgame, which asks for chunks again, is the session's
//! job.

use std::fmt;

use crate::Rng;
use crate::availability::Availability;
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
    /// How many connected peers have each chunk.
    pub availability: &'a Availability,
    /// The chunk a media player is at, for streaming.
    pub position: u32,
}

impl SelectContext<'_> {
    /// The chunks that may be requested from the provider: needed, and held by the provider.
    pub fn candidates(&self) -> impl Iterator<Item = u32> + '_ {
        self.needed.intersection(self.provider)
    }

    fn random_candidate(&self, rng: &mut Rng) -> Option<u32> {
        ChunkSet::random_common(&[self.needed, self.provider], rng)
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
            let chunk = ctx.random_candidate(rng)?;
            return Some((chunk, SelectReason::RandomFirst));
        }
        // The lowest holder count with a candidate, then a random candidate at that count.
        let levels = ctx
            .availability
            .levels()
            .filter(|(_, level)| !level.is_empty());
        levels.into_iter().find_map(|(holders, level)| {
            let chunk = ChunkSet::random_common(&[level, ctx.needed, ctx.provider], rng)?;
            Some((chunk, SelectReason::Rarest { holders }))
        })
    }
}

/// Any needed chunk, at random.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Random;

impl ChunkSelector for Random {
    fn select(&mut self, ctx: &SelectContext<'_>, rng: &mut Rng) -> Option<(u32, SelectReason)> {
        let chunk = ctx.random_candidate(rng)?;
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

/// Chunks within a window after the playback position in order, and rarest-first elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Streaming {
    /// How many chunks from the position are requested in order.
    pub window: u32,
    /// The strategy for chunks outside the window.
    pub elsewhere: RarestFirst,
}

impl ChunkSelector for Streaming {
    fn select(&mut self, ctx: &SelectContext<'_>, rng: &mut Rng) -> Option<(u32, SelectReason)> {
        let end = ctx.position.saturating_add(self.window);
        let near = ChunkSet::first_common(&[ctx.needed, ctx.provider], ctx.position)
            .filter(|&chunk| chunk < end);
        match near {
            Some(chunk) => {
                let ahead = chunk - ctx.position;
                Some((chunk, SelectReason::NearPlayback { ahead }))
            }
            None => self.elsewhere.select(ctx, rng),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use proptest::prelude::*;
    use rand::SeedableRng;

    use super::*;
    use crate::Rng;

    fn set(capacity: u32, chunks: &[u32]) -> ChunkSet {
        let mut set = ChunkSet::new(capacity);
        chunks.iter().for_each(|&c| {
            set.insert(c);
        });
        set
    }

    fn availability(counts: &[u32]) -> Availability {
        let mut index = Availability::new(counts.len() as u32);
        for (chunk, &count) in counts.iter().enumerate() {
            (0..count).for_each(|_| index.increment(chunk as u32));
        }
        index
    }

    #[test]
    fn rarest_first_picks_among_the_rarest() {
        let have = set(6, &[0]);
        let needed = set(6, &[1, 2, 3, 4, 5]);
        let provider = set(6, &[0, 1, 2, 3, 4]);
        let availability = availability(&[5, 3, 1, 2, 1, 0]);
        let ctx = SelectContext {
            have: &have,
            needed: &needed,
            provider: &provider,
            availability: &availability,
            position: 0,
        };
        let mut selector = RarestFirst { random_first: 0 };
        let mut rng = Rng::seed_from_u64(1);
        let mut seen = BTreeSet::new();
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
        let availability = availability(&[1, 1, 1, 1]);
        let ctx = SelectContext {
            have: &empty,
            needed: &all,
            provider: &all,
            availability: &availability,
            position: 0,
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
        let availability = Availability::new(8);
        let ctx = SelectContext {
            have: &have,
            needed: &needed,
            provider: &provider,
            availability: &availability,
            position: 0,
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

    #[test]
    fn streaming_prefers_the_window_then_falls_back_to_rarest() {
        let have = set(20, &[0, 1, 2, 3]);
        let needed = set(20, &[4, 9, 10, 15, 18]);
        let all = ChunkSet::full(20);
        let mut counts = [3; 20];
        counts[18] = 1;
        let availability = availability(&counts);
        let mut selector = Streaming {
            window: 4,
            elsewhere: RarestFirst { random_first: 0 },
        };
        let mut rng = Rng::seed_from_u64(1);
        let mut ctx = SelectContext {
            have: &have,
            needed: &needed,
            provider: &all,
            availability: &availability,
            position: 8,
        };
        assert_eq!(
            selector.select(&ctx, &mut rng),
            Some((9, SelectReason::NearPlayback { ahead: 1 }))
        );
        ctx.position = 11;
        assert_eq!(
            selector.select(&ctx, &mut rng),
            Some((18, SelectReason::Rarest { holders: 1 }))
        );
    }

    fn any_context() -> impl Strategy<Value = (Vec<bool>, Vec<bool>, Vec<bool>, Vec<u32>, u32)> {
        (1..200usize).prop_flat_map(|n| {
            (
                proptest::collection::vec(any::<bool>(), n),
                proptest::collection::vec(any::<bool>(), n),
                proptest::collection::vec(any::<bool>(), n),
                proptest::collection::vec(0..6u32, n),
                0..n as u32 + 5,
            )
        })
    }

    proptest! {
        #[test]
        fn every_selector_picks_a_candidate_when_there_is_one(
            (have, needed, provider, counts, position) in any_context(),
            seed: u64,
        ) {
            let n = have.len() as u32;
            let from = |bits: &[bool]| {
                let mut set = ChunkSet::new(n);
                bits.iter().enumerate().filter(|(_, b)| **b).for_each(|(i, _)| {
                    set.insert(i as u32);
                });
                set
            };
            let (have, provider) = (from(&have), from(&provider));
            let mut needed = from(&needed);
            have.iter().for_each(|c| { needed.remove(c); });
            let availability = availability(&counts);
            let ctx = SelectContext {
                have: &have,
                needed: &needed,
                provider: &provider,
                availability: &availability,
                position,
            };
            let candidates: BTreeSet<u32> = ctx.candidates().collect();
            let rarest = candidates.iter().map(|&c| counts[c as usize]).min();
            let mut selectors: Vec<Box<dyn ChunkSelector>> = vec![
                Box::new(RarestFirst { random_first: 0 }),
                Box::new(RarestFirst { random_first: 3 }),
                Box::new(Random),
                Box::new(Sequential),
                Box::new(Streaming { window: 8, elsewhere: RarestFirst { random_first: 0 } }),
            ];
            let mut rng = Rng::seed_from_u64(seed);
            for selector in &mut selectors {
                let pick = selector.select(&ctx, &mut rng);
                prop_assert_eq!(pick.is_some(), !candidates.is_empty());
                if let Some((chunk, reason)) = pick {
                    prop_assert!(candidates.contains(&chunk));
                    if let SelectReason::Rarest { holders } = reason {
                        prop_assert_eq!(Some(holders), rarest);
                        prop_assert_eq!(counts[chunk as usize], holders);
                    }
                }
            }
        }
    }
}
