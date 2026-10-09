//! PropShare: upload shared in proportion to what each peer uploaded to us (D-062).

use std::time::Duration;

use rand::seq::IndexedRandom;

use super::bittorrent::Optimistic;
use super::{
    PeerView, QueuedUpload, ServeDecision, UploadContext, UploadStrategy, lowest_score, not_wanted,
};
use crate::Rng;
use crate::config::UploadSettings;
use crate::event::ServeReason;
use crate::io::ConnId;
use crate::time::Time;

/// The share of upload kept for optimistic peers, as in Levin et al.'s PropShare.
pub const OPTIMISTIC_SHARE: f64 = 0.2;

/// PropShare (Levin et al., "BitTorrent is an Auction").
///
/// It serves every wanting peer that uploaded to this peer over the rate window, plus
/// `optimistic_slots` others chosen at random and rotated every optimistic interval. Upload is
/// then divided by ordering responses: each contributor's share of upload is its share of what
/// this peer received, scaled to leave [`OPTIMISTIC_SHARE`] to the optimistic peers, and the next
/// response goes to the peer furthest below its share.
#[derive(Debug, Clone)]
pub struct PropShare {
    interval: Duration,
    rate_window: Duration,
    next_review: Option<Time>,
    optimistic: Optimistic,
}

impl PropShare {
    /// The strategy with the intervals and optimistic slot count of `settings`.
    pub fn new(settings: &UploadSettings) -> Self {
        Self {
            interval: settings.review_interval,
            rate_window: settings.rate_window,
            next_review: None,
            optimistic: Optimistic::new(settings),
        }
    }

    fn total_rate(ctx: &UploadContext<'_>) -> f64 {
        ctx.peers
            .iter()
            .filter(|p| p.wants)
            .map(|p| p.receive_rate)
            .sum()
    }

    fn proportional(peer: &PeerView, total: f64) -> ServeDecision {
        ServeDecision {
            conn: peer.conn,
            serve: true,
            reason: ServeReason::Proportional {
                permille: (peer.receive_rate / total * 1000.0).round() as u32,
            },
        }
    }

    fn full_review(&mut self, ctx: &UploadContext<'_>, rng: &mut Rng) -> Vec<ServeDecision> {
        self.next_review = Some(ctx.now + self.interval);
        let others: Vec<ConnId> = ctx
            .peers
            .iter()
            .filter(|p| p.wants && p.receive_rate <= 0.0)
            .map(|p| p.conn)
            .collect();
        self.optimistic.update(ctx.now, &others, rng);
        let total = Self::total_rate(ctx);
        ctx.peers
            .iter()
            .map(|peer| {
                if !peer.wants {
                    not_wanted(peer)
                } else if peer.receive_rate > 0.0 {
                    Self::proportional(peer, total)
                } else {
                    let optimistic = self.optimistic.conns.contains(&peer.conn);
                    ServeDecision {
                        conn: peer.conn,
                        serve: optimistic,
                        reason: if optimistic {
                            ServeReason::Optimistic
                        } else {
                            ServeReason::NotContributing
                        },
                    }
                }
            })
            .collect()
    }

    fn between_reviews(&mut self, ctx: &UploadContext<'_>, rng: &mut Rng) -> Vec<ServeDecision> {
        let total = Self::total_rate(ctx);
        let mut decisions = Vec::new();
        for peer in ctx.peers {
            if peer.serving && !peer.wants {
                decisions.push(not_wanted(peer));
            } else if !peer.serving && peer.wants && peer.receive_rate > 0.0 {
                decisions.push(Self::proportional(peer, total));
            }
        }
        self.optimistic
            .conns
            .retain(|c| ctx.peer(*c).is_some_and(|p| p.wants));
        let free = self
            .optimistic
            .slots
            .saturating_sub(self.optimistic.conns.len());
        let waiting: Vec<ConnId> = ctx
            .peers
            .iter()
            .filter(|p| p.wants && !p.serving && p.receive_rate <= 0.0)
            .map(|p| p.conn)
            .collect();
        for &conn in waiting.choose_multiple(rng, free) {
            self.optimistic.conns.push(conn);
            decisions.push(ServeDecision {
                conn,
                serve: true,
                reason: ServeReason::Optimistic,
            });
        }
        decisions
    }
}

impl UploadStrategy for PropShare {
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

    fn next_upload(&mut self, queue: &[QueuedUpload], ctx: &UploadContext<'_>) -> usize {
        let served = || ctx.peers.iter().filter(|p| p.serving);
        let total: f64 = served().map(|p| p.receive_rate).sum();
        let others = served().filter(|p| p.receive_rate <= 0.0).count() as f64;
        let contributors_share = if others == 0.0 {
            1.0
        } else {
            1.0 - OPTIMISTIC_SHARE
        };
        let window = self.rate_window.as_secs_f64();
        lowest_score(queue, ctx, |peer| {
            let share = if total <= 0.0 {
                // Nobody has contributed: share equally.
                1.0
            } else if peer.receive_rate > 0.0 {
                contributors_share * peer.receive_rate / total
            } else {
                OPTIMISTIC_SHARE / others.max(1.0)
            };
            let given = peer.send_rate * window + peer.in_progress as f64;
            given / share
        })
    }
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;

    use super::*;
    use crate::upload::testing::{apply, peer, queue};

    #[test]
    fn serves_contributors_and_an_optimistic_peer() {
        let mut peers: Vec<PeerView> = (1..=5).map(peer).collect();
        peers[0].receive_rate = 300.0;
        peers[1].receive_rate = 100.0;
        peers[4].wants = false;
        peers[4].receive_rate = 1000.0;
        let mut strategy = PropShare::new(&UploadSettings::default());
        let mut rng = Rng::seed_from_u64(2);
        let ctx = UploadContext {
            now: Time::ZERO,
            complete: false,
            peers: &peers.clone(),
        };
        let decisions = strategy.review(&ctx, &mut rng);
        let served = apply(&mut peers, &decisions);
        assert_eq!(served.len(), 3);
        assert!(served.contains(&1) && served.contains(&2));
        assert!(decisions.contains(&ServeDecision {
            conn: ConnId(1),
            serve: true,
            reason: ServeReason::Proportional { permille: 750 },
        }));
    }

    #[test]
    fn responses_follow_contribution() {
        let mut peers: Vec<PeerView> = (1..=3).map(peer).collect();
        for p in &mut peers {
            p.serving = true;
        }
        peers[0].receive_rate = 300.0;
        peers[1].receive_rate = 100.0;
        let mut strategy = PropShare::new(&UploadSettings::default());
        // Simulate many responses of equal size, tracking what each peer was given.
        let mut given = [0u64; 3];
        for _ in 0..1000 {
            for (p, &g) in peers.iter_mut().zip(&given) {
                p.in_progress = g;
            }
            let ctx = UploadContext {
                now: Time::ZERO,
                complete: false,
                peers: &peers,
            };
            let index = strategy.next_upload(&queue(&[1, 2, 3]), &ctx);
            given[index] += 1000;
        }
        let total: u64 = given.iter().sum();
        let shares: Vec<f64> = given.iter().map(|&g| g as f64 / total as f64).collect();
        // 80% split 3:1 between the contributors, 20% to the optimistic peer.
        assert!((shares[0] - 0.6).abs() < 0.01, "{shares:?}");
        assert!((shares[1] - 0.2).abs() < 0.01, "{shares:?}");
        assert!((shares[2] - 0.2).abs() < 0.01, "{shares:?}");
    }
}
