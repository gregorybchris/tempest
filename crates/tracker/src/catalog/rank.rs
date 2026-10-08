//! Ranking listings (D-130): by how well they match a search, how healthy their swarms are now,
//! their votes and their recent downloads.
//!
//! Everything here is a pure function of a listing's [`Signals`], so the orders can be tested
//! with made-up numbers.

use tempest_catalog_api::listing::VoteCounts;
use tempest_catalog_api::query::Sort;
use tempest_protocol::BundleId;
use tempest_protocol::tracker::BundleStats;

/// How many days of downloads count towards trending.
pub const TRENDING_DAYS: u64 = 30;

/// After how many days a download counts half as much towards trending.
const HALF_LIFE_DAYS: f64 = 3.0;

/// How much an incomplete peer counts towards a swarm's health, against a complete one.
const INCOMPLETE_WEIGHT: f64 = 0.25;

/// How strongly popularity raises a search match's relevance.
const POPULARITY_WEIGHT: f64 = 0.2;

/// What ranks a listing besides its text.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Signals {
    /// When its listing was first asked for, in seconds since the Unix epoch.
    pub created: u64,
    /// Its swarm now.
    pub swarm: BundleStats,
    /// Its votes.
    pub votes: VoteCounts,
    /// Its recent downloads, weighted by age: see [`trending`].
    pub trending: f64,
}

/// A listing to rank.
#[derive(Debug, Clone, PartialEq)]
pub struct Ranked {
    /// The listed bundle.
    pub bundle: BundleId,
    /// How well it matched the search's text.
    pub text_score: f32,
    /// What else ranks it.
    pub signals: Signals,
}

/// Recent downloads, each day's `(day, count)` weighing half as much every [`HALF_LIFE_DAYS`]
/// before `today`. Days are counted since the Unix epoch; days before the last
/// [`TRENDING_DAYS`] and days after `today` count nothing.
pub fn trending(days: impl IntoIterator<Item = (u64, u64)>, today: u64) -> f64 {
    days.into_iter()
        .filter(|&(day, _)| day <= today && today - day < TRENDING_DAYS)
        .map(|(day, count)| count as f64 * 0.5f64.powf((today - day) as f64 / HALF_LIFE_DAYS))
        .sum()
}

/// A swarm's health: its complete peers, and a quarter for each incomplete one.
pub fn health(swarm: &BundleStats) -> f64 {
    swarm.complete as f64 + INCOMPLETE_WEIGHT * swarm.incomplete as f64
}

/// Votes up less votes down.
pub fn net_votes(votes: &VoteCounts) -> i64 {
    let clamp = |n: u64| i64::try_from(n).unwrap_or(i64::MAX);
    clamp(votes.up) - clamp(votes.down)
}

/// How popular a listing is: the logarithms of its health, net votes (negative when they are)
/// and trending score, added, so each matters but none runs away with the ranking.
pub fn popularity(signals: &Signals) -> f64 {
    let net = net_votes(&signals.votes) as f64;
    health(&signals.swarm).ln_1p() + net.signum() * net.abs().ln_1p() + signals.trending.ln_1p()
}

/// A listing's relevance to a search: its text score, raised by its popularity. Without search
/// text every text score is the same, so popularity alone decides.
pub fn relevance(text_score: f32, signals: &Signals) -> f64 {
    f64::from(text_score) * (POPULARITY_WEIGHT * popularity(signals)).exp()
}

/// Sorts `listings` by `sort`. Ties go to the newer listing, then to the lower bundle ID, so the
/// order is the same on every page.
pub fn sort(listings: &mut [Ranked], sort: Sort) {
    let key = |r: &Ranked| -> [f64; 2] {
        let s = &r.signals;
        match sort {
            Sort::Relevance => [relevance(r.text_score, s), 0.0],
            Sort::Newest => [0.0, 0.0],
            Sort::Seeders => [s.swarm.complete as f64, s.swarm.incomplete as f64],
            Sort::Votes => [net_votes(&s.votes) as f64, s.votes.up as f64],
            Sort::Trending => [s.trending, health(&s.swarm)],
        }
    };
    listings.sort_by(|a, b| {
        let (a_key, b_key) = (key(a), key(b));
        b_key[0]
            .total_cmp(&a_key[0])
            .then_with(|| b_key[1].total_cmp(&a_key[1]))
            .then_with(|| b.signals.created.cmp(&a.signals.created))
            .then_with(|| a.bundle.as_bytes().cmp(b.bundle.as_bytes()))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use tempest_protocol::Hash;

    fn ranked(n: u8, text_score: f32, signals: Signals) -> Ranked {
        Ranked {
            bundle: Hash::new([n; 32]),
            text_score,
            signals: Signals {
                created: u64::from(n),
                ..signals
            },
        }
    }

    fn swarm(complete: u64, incomplete: u64) -> BundleStats {
        BundleStats {
            complete,
            incomplete,
        }
    }

    fn votes(up: u64, down: u64) -> VoteCounts {
        VoteCounts { up, down }
    }

    fn order(mut listings: Vec<Ranked>, by: Sort) -> Vec<u8> {
        sort(&mut listings, by);
        listings.iter().map(|r| r.bundle.as_bytes()[0]).collect()
    }

    #[test]
    fn trending_halves_every_half_life() {
        assert_eq!(trending([(100, 8)], 100), 8.0);
        assert!((trending([(97, 8)], 100) - 4.0).abs() < 1e-9);
        assert!((trending([(94, 8), (100, 1)], 100) - 3.0).abs() < 1e-9);
        assert_eq!(trending([(100 - TRENDING_DAYS, 8), (101, 8)], 100), 0.0);
    }

    #[test]
    fn each_sort_uses_its_signal() {
        let listings = vec![
            ranked(
                1,
                1.0,
                Signals {
                    swarm: swarm(1, 9),
                    ..Signals::default()
                },
            ),
            ranked(
                2,
                1.0,
                Signals {
                    swarm: swarm(3, 0),
                    ..Signals::default()
                },
            ),
            ranked(
                3,
                1.0,
                Signals {
                    votes: votes(5, 1),
                    ..Signals::default()
                },
            ),
            ranked(
                4,
                1.0,
                Signals {
                    trending: 7.0,
                    ..Signals::default()
                },
            ),
        ];
        assert_eq!(order(listings.clone(), Sort::Newest), [4, 3, 2, 1]);
        assert_eq!(order(listings.clone(), Sort::Seeders), [2, 1, 4, 3]);
        assert_eq!(order(listings.clone(), Sort::Votes), [3, 4, 2, 1]);
        // Swarm health breaks trending's ties: 1 + 9/4 beats 3.
        assert_eq!(order(listings.clone(), Sort::Trending), [4, 1, 2, 3]);
        // Popularity: ln(1 + 7) for trending, ln(1 + 4) for net votes, ln(1 + health).
        assert_eq!(order(listings, Sort::Relevance), [4, 3, 1, 2]);
    }

    #[test]
    fn relevance_weighs_text_above_popularity() {
        let popular = Signals {
            swarm: swarm(20, 20),
            votes: votes(10, 0),
            ..Signals::default()
        };
        let listings = vec![
            ranked(1, 10.0, Signals::default()),
            ranked(2, 2.0, popular),
            ranked(3, 2.0, Signals::default()),
        ];
        assert_eq!(order(listings, Sort::Relevance), [1, 2, 3]);
        let down = Signals {
            votes: votes(0, 10),
            ..Signals::default()
        };
        assert!(relevance(1.0, &down) < relevance(1.0, &Signals::default()));
    }

    proptest! {
        #[test]
        fn sorting_is_a_stable_total_order(
            raw in proptest::collection::vec(
                (0.0f32..10.0, 0u64..5, 0u64..5, 0u64..5, 0u64..5, 0.0f64..10.0),
                0..20,
            ),
            which in 0..Sort::ALL.len(),
        ) {
            let mut listings: Vec<Ranked> = raw
                .into_iter()
                .enumerate()
                .map(|(n, (text, c, i, up, down, trend))| ranked(n as u8, text, Signals {
                    swarm: swarm(c, i),
                    votes: votes(up, down),
                    trending: trend,
                    ..Signals::default()
                }))
                .collect();
            let by = Sort::ALL[which];
            sort(&mut listings, by);
            let once = listings.clone();
            listings.reverse();
            sort(&mut listings, by);
            prop_assert_eq!(listings, once);
        }
    }
}
