//! The strategic client: a BitTyrant-style upload strategy for simulated misbehaving peers
//! (D-070, D-166).
//!
//! BitTyrant (Piatek et al., "Do incentives build robustness in BitTorrent?", NSDI 2007) treats
//! uploading as buying download. For each peer it estimates the download it gets back (*d*) and
//! the upload needed to get it (*u*), serves the peers with the best *d*/*u* until its upload
//! capacity is spent, and gives each just its *u*. It then learns: a peer that keeps serving it
//! is probably being overpaid, and one that doesn't, underpaid. The strategy lives in the
//! simulator because it exists to test how well the real strategies resist it; no setting
//! names it.

use std::collections::BTreeMap;
use std::time::Duration;

use rand::seq::SliceRandom;
use tempest_core::Rng;
use tempest_core::config::UploadSettings;
use tempest_core::event::ServeReason;
use tempest_core::io::ConnId;
use tempest_core::time::Time;
use tempest_core::upload::{PeerView, QueuedUpload, ServeDecision, UploadContext, UploadStrategy};
use tempest_protocol::PeerId;

/// How many reviews in a row a peer must serve this one before its price is lowered.
pub const RECIPROCATION_ROUNDS: u32 = 3;

/// What a price is multiplied by after [`RECIPROCATION_ROUNDS`] of being served.
pub const DECREASE: f64 = 0.9;

/// What a price is multiplied by after a review in which the peer, though served, didn't
/// serve this one.
pub const INCREASE: f64 = 1.2;

/// How many times more connections a strategic peer keeps than its settings say.
pub const PEER_SET_FACTOR: u32 = 2;

/// What the strategy has learned about one peer.
#[derive(Debug, Clone, Copy)]
struct Estimate {
    /// *u*, the upload believed needed for the peer to serve this one, in equal shares: the
    /// upload capacity divided among BitTorrent's slots.
    price: f64,
    /// *d*: the rate the peer last uploaded to this one at, in bytes per second, if it ever did.
    gives: Option<f64>,
    /// Reviews in a row in which the peer served this one while this one served it.
    rounds: u32,
}

impl Default for Estimate {
    fn default() -> Self {
        Self {
            price: 1.0,
            gives: None,
            rounds: 0,
        }
    }
}

/// A BitTyrant-style upload strategy.
///
/// Prices are measured in equal shares of the upload capacity among BitTorrent's slots, and
/// start at one. Every review interval the strategy serves the interested peers with the best
/// ratio of what they last gave to their price, until the prices add up to the slot count, and
/// orders responses so that each served peer gets upload in proportion to its price. Peers that
/// never uploaded to it rank last, in random order, and get what the others leave; peers that
/// serve it optimistically reveal what they give.
#[derive(Debug, Clone)]
pub struct Strategic {
    interval: Duration,
    rate_window: Duration,
    slots: f64,
    next_review: Option<Time>,
    estimates: BTreeMap<PeerId, Estimate>,
}

impl Strategic {
    /// The strategy with the review interval, rate window and slot counts of `settings`.
    pub fn new(settings: &UploadSettings) -> Self {
        Self {
            interval: settings.review_interval,
            rate_window: settings.rate_window,
            slots: f64::from((settings.reciprocal_slots + settings.optimistic_slots).max(1)),
            next_review: None,
            estimates: BTreeMap::new(),
        }
    }

    fn price(&self, peer: &PeerView) -> f64 {
        self.estimates.get(&peer.peer).map_or(1.0, |e| e.price)
    }

    /// Learns from the last interval: notes what each peer gave, and raises or lowers the price
    /// of each peer served in it.
    fn learn(&mut self, ctx: &UploadContext<'_>) {
        for peer in ctx.peers {
            let estimate = self.estimates.entry(peer.peer).or_default();
            if peer.receive_rate > 0.0 {
                estimate.gives = Some(peer.receive_rate);
            }
            if !peer.serving {
                continue;
            }
            if peer.serves_us {
                estimate.rounds += 1;
                if estimate.rounds >= RECIPROCATION_ROUNDS {
                    estimate.price *= DECREASE;
                    estimate.rounds = 0;
                }
            } else {
                estimate.rounds = 0;
                estimate.price *= INCREASE;
            }
        }
    }

    /// The interested peers among `peers` that fit in `budget` shares, best ratio of what they
    /// gave to their price first, ties broken at random. Unless `strict`, the first is taken
    /// even if it doesn't fit.
    fn choose<'p>(
        &self,
        peers: impl Iterator<Item = &'p PeerView>,
        mut budget: f64,
        strict: bool,
        rng: &mut Rng,
    ) -> Vec<ConnId> {
        let mut ranked: Vec<(&PeerView, f64)> = peers
            .filter(|p| p.wants)
            .map(|p| {
                let gives = self.estimates.get(&p.peer).and_then(|e| e.gives);
                (p, gives.unwrap_or(0.0) / self.price(p))
            })
            .collect();
        ranked.shuffle(rng);
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut chosen = Vec::new();
        for (peer, _) in ranked {
            let price = self.price(peer);
            if price > budget && (strict || !chosen.is_empty()) {
                break;
            }
            budget -= price;
            chosen.push(peer.conn);
        }
        chosen
    }

    fn full_review(&mut self, ctx: &UploadContext<'_>, rng: &mut Rng) -> Vec<ServeDecision> {
        self.next_review = Some(ctx.now + self.interval);
        self.learn(ctx);
        let chosen = self.choose(ctx.peers.iter(), self.slots, false, rng);
        ctx.peers
            .iter()
            .map(|peer| {
                if !peer.wants {
                    not_wanted(peer)
                } else if chosen.contains(&peer.conn) {
                    serve(peer)
                } else {
                    ServeDecision {
                        conn: peer.conn,
                        serve: false,
                        reason: ServeReason::Outpaced,
                    }
                }
            })
            .collect()
    }

    /// Stops serving peers that want nothing more, and spends what is left of the budget on
    /// peers waiting to be served.
    fn between_reviews(&self, ctx: &UploadContext<'_>, rng: &mut Rng) -> Vec<ServeDecision> {
        let served = ctx.peers.iter().filter(|p| p.serving && p.wants);
        let left = self.slots - served.map(|p| self.price(p)).sum::<f64>();
        let waiting = ctx.peers.iter().filter(|p| !p.serving);
        let chosen = self.choose(waiting, left, true, rng);
        ctx.peers
            .iter()
            .filter_map(|peer| {
                if peer.serving && !peer.wants {
                    Some(not_wanted(peer))
                } else if chosen.contains(&peer.conn) {
                    Some(serve(peer))
                } else {
                    None
                }
            })
            .collect()
    }
}

fn not_wanted(peer: &PeerView) -> ServeDecision {
    ServeDecision {
        conn: peer.conn,
        serve: false,
        reason: ServeReason::NotWanted,
    }
}

fn serve(peer: &PeerView) -> ServeDecision {
    let reason = if peer.receive_rate > 0.0 {
        ServeReason::Reciprocal {
            rate: peer.receive_rate as u64,
        }
    } else {
        ServeReason::FreeSlot
    };
    ServeDecision {
        conn: peer.conn,
        serve: true,
        reason,
    }
}

impl UploadStrategy for Strategic {
    fn review(&mut self, ctx: &UploadContext<'_>, rng: &mut Rng) -> Vec<ServeDecision> {
        if self.next_review.is_none_or(|t| t <= ctx.now) {
            self.full_review(ctx, rng)
        } else {
            self.between_reviews(ctx, rng)
        }
    }

    fn next_review(&self) -> Option<Time> {
        self.next_review
    }

    /// The request of the served peer furthest below its price, the oldest among equals.
    fn next_upload(&mut self, queue: &[QueuedUpload], ctx: &UploadContext<'_>) -> usize {
        let window = self.rate_window.as_secs_f64();
        let mut best = (0, f64::INFINITY);
        for (index, item) in queue.iter().enumerate() {
            let Some(peer) = ctx.peer(item.conn) else {
                continue;
            };
            let given = peer.send_rate * window + peer.in_progress as f64;
            let score = given / self.price(peer);
            if score < best.1 {
                best = (index, score);
            }
        }
        best.0
    }
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;
    use tempest_core::StreamId;
    use tempest_protocol::Hash;

    use super::*;

    fn peer(n: u8) -> PeerView {
        PeerView {
            conn: ConnId(n.into()),
            peer: Hash::new([n; 32]),
            wants: true,
            serving: false,
            serves_us: false,
            complete: false,
            received: 0,
            sent: 0,
            in_progress: 0,
            receive_rate: 0.0,
            send_rate: 0.0,
        }
    }

    fn at(seconds: u64) -> Time {
        Time::ZERO + Duration::from_secs(seconds)
    }

    /// Runs a review over `peers` and applies its decisions. Returns the served connections.
    fn review(strategy: &mut Strategic, peers: &mut [PeerView], now: Time) -> Vec<u64> {
        let mut rng = Rng::seed_from_u64(now.since_epoch().as_secs());
        let snapshot = peers.to_vec();
        let ctx = UploadContext {
            now,
            complete: false,
            peers: &snapshot,
        };
        for decision in strategy.review(&ctx, &mut rng) {
            if let Some(peer) = peers.iter_mut().find(|p| p.conn == decision.conn) {
                peer.serving = decision.serve;
            }
        }
        peers
            .iter()
            .filter(|p| p.serving)
            .map(|p| p.conn.0)
            .collect()
    }

    fn price(strategy: &Strategic, n: u8) -> f64 {
        strategy.estimates[&Hash::new([n; 32])].price
    }

    #[test]
    fn serves_the_best_bargains_within_its_slots() {
        let mut peers: Vec<PeerView> = (1..=10).map(peer).collect();
        peers[0].receive_rate = 500.0;
        peers[1].receive_rate = 400.0;
        peers[9].wants = false;
        peers[9].receive_rate = 1000.0;
        let mut strategy = Strategic::new(&UploadSettings::default());
        // Every price starts at one share, so five peers fit, the two givers first.
        let served = review(&mut strategy, &mut peers, at(0));
        assert_eq!(served.len(), 5);
        assert!(served.contains(&1) && served.contains(&2));
        assert!(!served.contains(&10));
    }

    #[test]
    fn cheaper_peers_mean_more_peers_served() {
        let mut peers: Vec<PeerView> = (1..=10).map(peer).collect();
        let mut strategy = Strategic::new(&UploadSettings::default());
        for p in &peers {
            strategy.estimates.insert(
                p.peer,
                Estimate {
                    price: 0.5,
                    ..Estimate::default()
                },
            );
        }
        assert_eq!(review(&mut strategy, &mut peers, at(0)).len(), 10);
    }

    #[test]
    fn prices_fall_for_peers_that_reciprocate_and_rise_for_those_that_dont() {
        let mut peers: Vec<PeerView> = (1..=2).map(peer).collect();
        peers[0].serves_us = true;
        peers[0].receive_rate = 300.0;
        let mut strategy = Strategic::new(&UploadSettings::default());
        for round in 0..RECIPROCATION_ROUNDS {
            for p in &mut peers {
                p.serving = true;
            }
            review(&mut strategy, &mut peers, at(u64::from(round) * 10));
        }
        // Peer 1 served this one for three rounds; peer 2 never did.
        assert!((price(&strategy, 1) - DECREASE).abs() < 1e-9);
        let raised = INCREASE.powi(RECIPROCATION_ROUNDS as i32);
        assert!((price(&strategy, 2) - raised).abs() < 1e-9);
    }

    #[test]
    fn between_reviews_it_frees_and_fills_its_slots() {
        let mut peers: Vec<PeerView> = (1..=6).map(peer).collect();
        let mut strategy = Strategic::new(&UploadSettings::default());
        let served = review(&mut strategy, &mut peers, at(0));
        assert_eq!(served.len(), 5);
        // A served peer stops wanting; its slot goes to the one waiting.
        let gone = served[0];
        peers[gone as usize - 1].wants = false;
        let now_served = review(&mut strategy, &mut peers, at(5));
        assert_eq!(now_served.len(), 5);
        assert!(!now_served.contains(&gone));
        // With no slot free, a newcomer waits for the next review.
        peers.push(peer(7));
        assert_eq!(review(&mut strategy, &mut peers, at(6)), now_served);
        assert_eq!(strategy.next_review(), Some(at(10)));
    }

    #[test]
    fn responses_follow_prices() {
        let mut peers: Vec<PeerView> = (1..=2).map(peer).collect();
        let mut strategy = Strategic::new(&UploadSettings::default());
        for (p, price) in peers.iter().zip([3.0, 1.0]) {
            strategy.estimates.insert(
                p.peer,
                Estimate {
                    price,
                    ..Estimate::default()
                },
            );
        }
        let queue: Vec<QueuedUpload> = [1, 2]
            .iter()
            .map(|&c| QueuedUpload {
                conn: ConnId(c),
                stream: StreamId(c),
                bytes: 1000,
            })
            .collect();
        let mut given = [0u64; 2];
        for _ in 0..400 {
            for (p, &g) in peers.iter_mut().zip(&given) {
                p.in_progress = g;
            }
            let ctx = UploadContext {
                now: Time::ZERO,
                complete: false,
                peers: &peers,
            };
            given[strategy.next_upload(&queue, &ctx)] += 1000;
        }
        assert_eq!(given, [300_000, 100_000]);
    }
}
