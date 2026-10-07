//! How long rarest-first takes to pick a chunk in a large bundle with many connected peers,
//! using the availability index, against scanning every candidate.
//!
//! Run with `cargo bench -p tempest-core --bench rarest_first`.

use std::hint::black_box;
use std::time::Instant;

use rand::{Rng as _, SeedableRng};
use tempest_core::Rng;
use tempest_core::availability::Availability;
use tempest_core::chunks::ChunkSet;
use tempest_core::select::{ChunkSelector, RarestFirst, SelectContext};

/// A bundle of `chunks` chunks, half of them stored, with 50 connected peers that each have a
/// random share of the chunks; and one provider among them.
fn scenario(chunks: u32, rng: &mut Rng) -> (ChunkSet, ChunkSet, ChunkSet, Availability) {
    let mut have = ChunkSet::new(chunks);
    let mut needed = ChunkSet::new(chunks);
    for chunk in 0..chunks {
        if rng.random_bool(0.5) {
            have.insert(chunk);
        } else {
            needed.insert(chunk);
        }
    }
    let mut availability = Availability::new(chunks);
    let mut provider = ChunkSet::new(chunks);
    for peer in 0..50 {
        let share = rng.random_range(0.05..1.0);
        for chunk in 0..chunks {
            if rng.random_bool(share) {
                availability.increment(chunk);
                if peer == 0 {
                    provider.insert(chunk);
                }
            }
        }
    }
    (have, needed, provider, availability)
}

/// Rarest-first by scanning every candidate, as before the index.
fn scan(ctx: &SelectContext<'_>, rng: &mut Rng) -> Option<u32> {
    let mut best: Option<(u32, u32)> = None;
    let mut ties = 0u32;
    for chunk in ctx.candidates() {
        let holders = ctx.availability.count(chunk);
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
    best.map(|(chunk, _)| chunk)
}

fn time(label: &str, picks: u32, mut pick: impl FnMut() -> Option<u32>) {
    let start = Instant::now();
    for _ in 0..picks {
        black_box(pick());
    }
    let per_pick = start.elapsed() / picks;
    println!("{label:<40} {per_pick:>10.2?} per pick");
}

fn main() {
    let mut rng = Rng::seed_from_u64(1);
    for chunks in [4_096, 65_536, 262_144] {
        let (have, needed, provider, availability) = scenario(chunks, &mut rng);
        let ctx = SelectContext {
            have: &have,
            needed: &needed,
            provider: &provider,
            availability: &availability,
            position: 0,
        };
        let picks = 2_000_000 / chunks;
        let mut selector = RarestFirst { random_first: 0 };
        time(
            &format!("{chunks} chunks, availability index"),
            picks,
            || selector.select(&ctx, &mut rng).map(|(chunk, _)| chunk),
        );
        time(
            &format!("{chunks} chunks, scanning candidates"),
            picks,
            || scan(&ctx, &mut rng),
        );
    }
}
