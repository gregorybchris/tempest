//! BitTorrent's choking algorithm: serve the peers that upload to us fastest, plus a rotating
//! optimistic slot (D-051).

use std::time::Duration;

use rand::seq::{IndexedRandom, SliceRandom};

use super::{PeerView, ServeDecision, UploadContext, UploadStrategy, not_wanted};
use crate::Rng;
use crate::config::UploadSettings;
use crate::event::ServeReason;
use crate::io::ConnId;
use crate::time::Time;

/// BitTorrent's choking algorithm, the default upload strategy while downloading.
///
/// Every review interval, it serves the `reciprocal_slots` interested peers that uploaded to
/// this peer fastest over the rate window, and `optimistic_slots` other interested peers chosen
/// at random, rotated every optimistic interval so newcomers can start. Between reviews it only
/// stops serving peers that want nothing more and fills slots left free.
#[derive(Debug, Clone)]
pub struct BitTorrent {
    slots: usize,
    interval: Duration,
    next_review: Option<Time>,
    optimistic: Optimistic,
}

impl BitTorrent {
    /// The strategy with the slot counts and intervals of `settings`.
    pub fn new(settings: &UploadSettings) -> Self {
        Self {
            slots: settings.reciprocal_slots as usize,
            interval: settings.review_interval,
            next_review: None,
            optimistic: Optimistic::new(settings),
        }
    }

    fn full_review(&mut self, ctx: &UploadContext<'_>, rng: &mut Rng) -> Vec<ServeDecision> {
        self.next_review = Some(ctx.now + self.interval);
        let mut wanting: Vec<&PeerView> = ctx.peers.iter().filter(|p| p.wants).collect();
        // Shuffling first breaks ties between equal rates at random.
        wanting.shuffle(rng);
        wanting.sort_by(|a, b| b.receive_rate.total_cmp(&a.receive_rate));
        let (regular, rest) = wanting.split_at(self.slots.min(wanting.len()));
        let rest: Vec<ConnId> = rest.iter().map(|p| p.conn).collect();
        self.optimistic.update(ctx.now, &rest, rng);

        ctx.peers
            .iter()
            .map(|peer| {
                let (serve, reason) = if regular.iter().any(|p| p.conn == peer.conn) {
                    if peer.receive_rate > 0.0 {
                        let rate = peer.receive_rate as u64;
                        (true, ServeReason::Reciprocal { rate })
                    } else {
                        (true, ServeReason::FreeSlot)
                    }
                } else if self.optimistic.conns.contains(&peer.conn) {
                    (true, ServeReason::Optimistic)
                } else if !peer.wants {
                    return not_wanted(peer);
                } else {
                    (false, ServeReason::Outpaced)
                };
                ServeDecision {
                    conn: peer.conn,
                    serve,
                    reason,
                }
            })
            .collect()
    }

    fn between_reviews(&mut self, ctx: &UploadContext<'_>, rng: &mut Rng) -> Vec<ServeDecision> {
        let mut decisions: Vec<ServeDecision> = ctx
            .peers
            .iter()
            .filter(|p| p.serving && !p.wants)
            .map(not_wanted)
            .collect();
        self.optimistic
            .conns
            .retain(|c| ctx.peer(*c).is_some_and(|p| p.wants));
        let served = ctx.peers.iter().filter(|p| p.serving && p.wants).count();
        let free = (self.slots + self.optimistic.slots).saturating_sub(served);
        let waiting: Vec<ConnId> = ctx
            .peers
            .iter()
            .filter(|p| p.wants && !p.serving)
            .map(|p| p.conn)
            .collect();
        decisions.extend(
            waiting
                .choose_multiple(rng, free)
                .map(|&conn| ServeDecision {
                    conn,
                    serve: true,
                    reason: ServeReason::FreeSlot,
                }),
        );
        decisions
    }
}

impl UploadStrategy for BitTorrent {
    fn review(&mut self, ctx: &UploadContext<'_>, rng: &mut Rng) -> Vec<ServeDecision> {
        let due = self.next_review.is_none_or(|t| t <= ctx.now) || self.optimistic.due(ctx.now);
        if due {
            self.full_review(ctx, rng)
        } else {
            self.between_reviews(ctx, rng)
        }
    }

    fn next_review(&self) -> Option<Time> {
        self.next_review.min(self.optimistic.next_rotation)
    }
}

/// Optimistic slots: peers served at random, regardless of what they gave, rotated on a timer.
#[derive(Debug, Clone)]
pub(super) struct Optimistic {
    pub slots: usize,
    interval: Duration,
    pub conns: Vec<ConnId>,
    pub next_rotation: Option<Time>,
}

impl Optimistic {
    pub fn new(settings: &UploadSettings) -> Self {
        Self {
            slots: settings.optimistic_slots as usize,
            interval: settings.optimistic_interval,
            conns: Vec::new(),
            next_rotation: None,
        }
    }

    pub fn due(&self, now: Time) -> bool {
        self.next_rotation.is_some_and(|t| t <= now)
    }

    /// Keeps the optimistic peers that are still `eligible`, or picks new ones if it is time to
    /// rotate, then fills free slots at random from `eligible`.
    pub fn update(&mut self, now: Time, eligible: &[ConnId], rng: &mut Rng) {
        if self.next_rotation.is_none_or(|t| t <= now) {
            self.conns.clear();
            self.next_rotation = Some(now + self.interval);
        }
        self.conns.retain(|c| eligible.contains(c));
        let pool: Vec<ConnId> = eligible
            .iter()
            .copied()
            .filter(|c| !self.conns.contains(c))
            .collect();
        let free = self.slots.saturating_sub(self.conns.len());
        self.conns.extend(pool.choose_multiple(rng, free));
    }
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;

    use super::*;
    use crate::upload::testing::{apply, peer};

    fn at(seconds: u64) -> Time {
        Time::ZERO + Duration::from_secs(seconds)
    }

    #[test]
    fn serves_the_fastest_uploaders_and_one_optimistic_peer() {
        let mut peers: Vec<PeerView> = (1..=8).map(peer).collect();
        for (i, p) in peers.iter_mut().enumerate() {
            p.receive_rate = (i * 100) as f64;
        }
        peers[7].wants = false;
        let mut strategy = BitTorrent::new(&UploadSettings::default());
        let mut rng = Rng::seed_from_u64(3);
        let ctx = UploadContext {
            now: at(0),
            complete: false,
            peers: &peers.clone(),
        };
        let decisions = strategy.review(&ctx, &mut rng);
        let served = apply(&mut peers, &decisions);
        // Peers 4–7 uploaded fastest among those that want data; one of 1–3 is optimistic.
        assert_eq!(served.len(), 5);
        assert!([4, 5, 6, 7].iter().all(|c| served.contains(c)));
        let optimistic: Vec<_> = decisions
            .iter()
            .filter(|d| d.reason == ServeReason::Optimistic)
            .collect();
        assert_eq!(optimistic.len(), 1);
        assert!(optimistic[0].conn.0 <= 3);
        assert!(decisions.contains(&ServeDecision {
            conn: ConnId(7),
            serve: true,
            reason: ServeReason::Reciprocal { rate: 600 },
        }));
        assert_eq!(strategy.next_review(), Some(at(10)));
    }

    #[test]
    fn between_reviews_only_frees_and_fills_slots() {
        let mut peers: Vec<PeerView> = (1..=6).map(peer).collect();
        let mut strategy = BitTorrent::new(&UploadSettings::default());
        let mut rng = Rng::seed_from_u64(1);
        let ctx = UploadContext {
            now: at(0),
            complete: false,
            peers: &peers.clone(),
        };
        let served = apply(&mut peers, &strategy.review(&ctx, &mut rng));
        assert_eq!(served.len(), 5);

        // A served peer stops wanting; its slot goes to the one waiting.
        let gone = served[0] as usize - 1;
        peers[gone].wants = false;
        let ctx = UploadContext {
            now: at(1),
            complete: false,
            peers: &peers.clone(),
        };
        let decisions = strategy.review(&ctx, &mut rng);
        let now_served = apply(&mut peers, &decisions);
        assert_eq!(now_served.len(), 5);
        assert!(!now_served.contains(&served[0]));
        assert!(
            decisions
                .iter()
                .all(|d| d.reason == ServeReason::NotWanted || d.reason == ServeReason::FreeSlot)
        );
    }

    #[test]
    fn the_optimistic_slot_rotates() {
        let settings = UploadSettings {
            reciprocal_slots: 0,
            ..UploadSettings::default()
        };
        let mut strategy = BitTorrent::new(&settings);
        let mut rng = Rng::seed_from_u64(9);
        let mut picks = std::collections::BTreeSet::new();
        let mut peers: Vec<PeerView> = (1..=4).map(peer).collect();
        for step in 0..40 {
            let now = at(step * 10);
            let ctx = UploadContext {
                now,
                complete: false,
                peers: &peers.clone(),
            };
            let served = apply(&mut peers, &strategy.review(&ctx, &mut rng));
            assert_eq!(served.len(), 1);
            picks.insert(served[0]);
            // Rotation happens only every 30 s.
            if step % 3 != 0 {
                assert_eq!(
                    strategy.optimistic.next_rotation,
                    Some(at((step / 3) * 30 + 30))
                );
            }
        }
        assert_eq!(
            picks.len(),
            4,
            "every peer eventually gets the optimistic slot"
        );
    }
}
