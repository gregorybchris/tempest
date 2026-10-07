//! Upload strategies: whom to serve, and whose request to answer next (`specs/strategies.md`,
//! D-051, D-062, D-063).
//!
//! A strategy makes two decisions. [`UploadStrategy::review`] picks the peers told Serving
//! (yes): the session calls it when something relevant changes and when the strategy asks to be
//! woken. [`UploadStrategy::next_upload`] picks which accepted request gets the next free upload
//! slot.

mod bittorrent;
mod propshare;
mod simple;

use std::fmt;

use tempest_protocol::PeerId;

pub use self::bittorrent::BitTorrent;
pub use self::propshare::PropShare;
pub use self::simple::{Cooperative, FairTorrent, RoundRobin};
use crate::Rng;
use crate::event::ServeReason;
use crate::io::{ConnId, StreamId};
use crate::time::Time;

/// What an upload strategy knows about one open connection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PeerView {
    /// The connection.
    pub conn: ConnId,
    /// The other peer.
    pub peer: PeerId,
    /// Whether the peer said it wants chunks this peer has.
    pub wants: bool,
    /// Whether this peer is serving it now.
    pub serving: bool,
    /// Whether the peer has every chunk.
    pub complete: bool,
    /// Bytes of verified Data received from the peer, in total.
    pub received: u64,
    /// Bytes of Data sent to the peer, in total.
    pub sent: u64,
    /// Bytes of Data being sent to the peer now.
    pub in_progress: u64,
    /// The recent rate of receiving from the peer, in bytes per second.
    pub receive_rate: f64,
    /// The recent rate of sending to the peer, in bytes per second.
    pub send_rate: f64,
}

/// What an upload strategy knows when deciding.
#[derive(Debug, Clone, Copy)]
pub struct UploadContext<'a> {
    /// The current time.
    pub now: Time,
    /// Whether this peer has every chunk.
    pub complete: bool,
    /// Every open connection, in connection order.
    pub peers: &'a [PeerView],
}

impl UploadContext<'_> {
    /// The view of `conn`, if it is open.
    pub fn peer(&self, conn: ConnId) -> Option<&PeerView> {
        self.peers.iter().find(|p| p.conn == conn)
    }
}

/// A strategy's decision to serve a peer or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServeDecision {
    /// The connection.
    pub conn: ConnId,
    /// Whether to serve it.
    pub serve: bool,
    /// Why.
    pub reason: ServeReason,
}

/// A request accepted and waiting for an upload slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueuedUpload {
    /// The connection.
    pub conn: ConnId,
    /// The request's stream.
    pub stream: StreamId,
    /// The number of payload bytes the response will carry.
    pub bytes: u64,
}

/// An upload strategy.
pub trait UploadStrategy: fmt::Debug + Send {
    /// Decides whom to serve. The session calls this when a connection opens or closes, when a
    /// peer's Want changes, when this peer completes, and at [`UploadStrategy::next_review`].
    ///
    /// Peers without a decision keep their current state.
    fn review(&mut self, ctx: &UploadContext<'_>, rng: &mut Rng) -> Vec<ServeDecision>;

    /// When the strategy next wants [`UploadStrategy::review`] called regardless of events.
    fn next_review(&self) -> Option<Time> {
        None
    }

    /// Which queued request to answer next, as an index into `queue`, which is in arrival order
    /// and not empty. The default answers the oldest first.
    fn next_upload(&mut self, _queue: &[QueuedUpload], _ctx: &UploadContext<'_>) -> usize {
        0
    }
}

/// The decision for a peer that doesn't want anything: stop serving it.
fn not_wanted(peer: &PeerView) -> ServeDecision {
    ServeDecision {
        conn: peer.conn,
        serve: false,
        reason: ServeReason::NotWanted,
    }
}

/// Serves every peer that wants data, for `reason`, and stops serving the rest.
fn serve_all_wanting(ctx: &UploadContext<'_>, reason: ServeReason) -> Vec<ServeDecision> {
    ctx.peers
        .iter()
        .map(|peer| {
            if peer.wants {
                ServeDecision {
                    conn: peer.conn,
                    serve: true,
                    reason,
                }
            } else {
                not_wanted(peer)
            }
        })
        .collect()
}

/// The index of the queued request whose peer scores lowest, the oldest among equals. Requests
/// from connections no longer open score last.
fn lowest_score(
    queue: &[QueuedUpload],
    ctx: &UploadContext<'_>,
    mut score: impl FnMut(&PeerView) -> f64,
) -> usize {
    let mut best = (0, f64::INFINITY);
    for (index, item) in queue.iter().enumerate() {
        let value = ctx.peer(item.conn).map_or(f64::INFINITY, &mut score);
        if value < best.1 {
            best = (index, value);
        }
    }
    best.0
}

#[cfg(test)]
pub(crate) mod testing {
    //! Helpers for the strategies' unit tests.

    use tempest_protocol::Hash;

    use super::*;

    /// An open connection numbered `n` that wants data, with nothing transferred.
    pub fn peer(n: u8) -> PeerView {
        PeerView {
            conn: ConnId(n.into()),
            peer: Hash::new([n; 32]),
            wants: true,
            serving: false,
            complete: false,
            received: 0,
            sent: 0,
            in_progress: 0,
            receive_rate: 0.0,
            send_rate: 0.0,
        }
    }

    /// The connections a strategy decided to serve, and applies the decisions to `peers`.
    pub fn apply(peers: &mut [PeerView], decisions: &[ServeDecision]) -> Vec<u64> {
        for decision in decisions {
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

    /// A queue with one request from each of `conns`.
    pub fn queue(conns: &[u64]) -> Vec<QueuedUpload> {
        conns
            .iter()
            .map(|&c| QueuedUpload {
                conn: ConnId(c),
                stream: StreamId(c),
                bytes: 1000,
            })
            .collect()
    }
}
