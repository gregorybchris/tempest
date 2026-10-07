//! Upload strategies: whom to serve, and whose request to answer next (`specs/strategies.md`,
//! D-051, D-062, D-063).

use std::fmt;

use tempest_protocol::PeerId;

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
    /// Bytes received from the peer, in total.
    pub received: u64,
    /// Bytes sent to the peer, in total.
    pub sent: u64,
    /// The recent rate of receiving from the peer, in bytes per second.
    pub receive_rate: f64,
    /// The recent rate of sending to the peer, in bytes per second.
    pub send_rate: f64,
}

/// What an upload strategy knows when deciding whom to serve.
#[derive(Debug, Clone, Copy)]
pub struct UploadContext<'a> {
    /// The current time.
    pub now: Time,
    /// Whether this peer has every chunk.
    pub complete: bool,
    /// Every open connection.
    pub peers: &'a [PeerView],
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

/// Serve every peer that wants data; for trusted swarms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cooperative;

impl UploadStrategy for Cooperative {
    fn review(&mut self, ctx: &UploadContext<'_>, _rng: &mut Rng) -> Vec<ServeDecision> {
        ctx.peers
            .iter()
            .map(|peer| ServeDecision {
                conn: peer.conn,
                serve: peer.wants,
                reason: if peer.wants {
                    ServeReason::Cooperative
                } else {
                    ServeReason::NotWanted
                },
            })
            .collect()
    }
}
