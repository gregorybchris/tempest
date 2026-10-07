//! Strategies that serve every peer that wants data, differing only in whose request goes next.

use std::collections::BTreeMap;

use super::{
    PeerView, QueuedUpload, ServeDecision, UploadContext, UploadStrategy, lowest_score,
    serve_all_wanting,
};
use crate::Rng;
use crate::event::ServeReason;
use crate::io::ConnId;

/// Serve every peer that wants data, answering requests in arrival order; for trusted swarms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cooperative;

impl UploadStrategy for Cooperative {
    fn review(&mut self, ctx: &UploadContext<'_>, _rng: &mut Rng) -> Vec<ServeDecision> {
        serve_all_wanting(ctx, ServeReason::Cooperative)
    }
}

/// FairTorrent: serve every peer that wants data, and send the next response to the peer this
/// peer owes most: bytes received from it minus bytes sent to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FairTorrent;

impl FairTorrent {
    /// What this peer owes `peer`, counting Data being sent as sent.
    pub fn deficit(peer: &PeerView) -> i128 {
        i128::from(peer.received) - i128::from(peer.sent) - i128::from(peer.in_progress)
    }
}

impl UploadStrategy for FairTorrent {
    fn review(&mut self, ctx: &UploadContext<'_>, _rng: &mut Rng) -> Vec<ServeDecision> {
        serve_all_wanting(ctx, ServeReason::FairTorrent)
    }

    fn next_upload(&mut self, queue: &[QueuedUpload], ctx: &UploadContext<'_>) -> usize {
        // The largest deficit is the lowest negated deficit.
        lowest_score(queue, ctx, |peer| -(Self::deficit(peer) as f64))
    }
}

/// Serve every peer that wants data, answering the peers in turn: the next response goes to the
/// peer whose last one was longest ago. A complete peer's default (D-063), and the serving side
/// of superseeding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoundRobin {
    reason: ServeReason,
    /// For each connection, when it was last picked, by a counter of picks.
    last_pick: BTreeMap<ConnId, u64>,
    picks: u64,
}

impl RoundRobin {
    /// The strategy for a complete peer.
    pub fn new() -> Self {
        Self::with_reason(ServeReason::RoundRobin)
    }

    /// The strategy while superseeding.
    pub fn superseeding() -> Self {
        Self::with_reason(ServeReason::Superseeding)
    }

    fn with_reason(reason: ServeReason) -> Self {
        Self {
            reason,
            last_pick: BTreeMap::new(),
            picks: 0,
        }
    }
}

impl Default for RoundRobin {
    fn default() -> Self {
        Self::new()
    }
}

impl UploadStrategy for RoundRobin {
    fn review(&mut self, ctx: &UploadContext<'_>, _rng: &mut Rng) -> Vec<ServeDecision> {
        self.last_pick
            .retain(|conn, _| ctx.peers.iter().any(|p| p.conn == *conn));
        serve_all_wanting(ctx, self.reason)
    }

    fn next_upload(&mut self, queue: &[QueuedUpload], ctx: &UploadContext<'_>) -> usize {
        let last_pick = &self.last_pick;
        // Never-picked connections come first, at -1.
        let index = lowest_score(queue, ctx, |peer| {
            last_pick.get(&peer.conn).map_or(-1.0, |&n| n as f64)
        });
        self.picks += 1;
        self.last_pick.insert(queue[index].conn, self.picks);
        index
    }
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;

    use super::*;
    use crate::time::Time;
    use crate::upload::testing::{apply, peer, queue};

    #[test]
    fn serving_everyone_who_wants() {
        let mut peers = vec![peer(1), peer(2), peer(3)];
        peers[1].wants = false;
        peers[1].serving = true;
        let ctx = UploadContext {
            now: Time::ZERO,
            complete: false,
            peers: &peers.clone(),
        };
        let mut rng = Rng::seed_from_u64(0);
        for mut strategy in [
            Box::new(Cooperative) as Box<dyn UploadStrategy>,
            Box::new(FairTorrent),
            Box::new(RoundRobin::new()),
        ] {
            let decisions = strategy.review(&ctx, &mut rng);
            assert_eq!(apply(&mut peers.clone(), &decisions), [1, 3]);
            let stopped = decisions.iter().find(|d| d.conn == ConnId(2)).unwrap();
            assert_eq!(stopped.reason, ServeReason::NotWanted);
        }
    }

    #[test]
    fn fairtorrent_answers_the_peer_owed_most() {
        let mut peers = vec![peer(1), peer(2), peer(3)];
        peers[0].received = 5000;
        peers[0].sent = 4000;
        peers[1].received = 9000;
        peers[1].sent = 2000;
        peers[1].in_progress = 3000;
        peers[2].received = 100;
        let ctx = UploadContext {
            now: Time::ZERO,
            complete: false,
            peers: &peers,
        };
        // Deficits: 1000, 4000 and 100.
        assert_eq!(FairTorrent.next_upload(&queue(&[1, 3, 2, 2]), &ctx), 2);
        assert_eq!(FairTorrent.next_upload(&queue(&[3, 1]), &ctx), 1);
    }

    #[test]
    fn round_robin_takes_turns() {
        let peers = vec![peer(1), peer(2), peer(3)];
        let ctx = UploadContext {
            now: Time::ZERO,
            complete: true,
            peers: &peers,
        };
        let mut strategy = RoundRobin::new();
        let mut queue = queue(&[1, 1, 1, 2, 2, 3]);
        let mut order = Vec::new();
        while !queue.is_empty() {
            let index = strategy.next_upload(&queue, &ctx);
            order.push(queue.remove(index).conn.0);
        }
        assert_eq!(order, [1, 2, 3, 1, 2, 1]);
    }
}
