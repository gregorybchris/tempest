//! The session's state for one connection.

use std::collections::BTreeMap;

use tempest_protocol::message::{HaveUpdate, Hello};
use tempest_protocol::{Message, PeerId, RangeList};

use crate::chunks::ChunkSet;
use crate::config::Config;
use crate::io::{ConnInfo, OutRequest, RequestId, StreamId};
use crate::meter::Meter;
use crate::pipeline::{DeliveryRate, RttEstimate};
use crate::time::Time;

/// One connection, from the opening sequence until it closes.
#[derive(Debug)]
pub(super) struct Conn {
    pub info: ConnInfo,
    /// Whether this peer has sent Hello and Have.
    pub sent_opening: bool,
    /// The other peer's Hello, once received.
    pub hello: Option<Hello>,
    /// When the connection reached the session.
    pub connected_at: Time,
    /// When the opening sequence completed: both peers sent and received Hello and Have.
    pub opened_at: Option<Time>,
    /// The chunks the other peer has.
    pub holdings: Holdings,
    /// How many of the other peer's chunks this peer lacks.
    pub interesting: u32,
    /// Whether the other peer said it wants chunks this peer has.
    pub they_want: bool,
    /// Whether this peer said it wants chunks the other peer has.
    pub we_want: bool,
    /// Whether the other peer said it is serving this peer.
    pub serving_us: bool,
    /// Whether this peer said it is serving the other peer.
    pub we_serve: bool,
    /// This peer's open requests on the connection.
    pub out: BTreeMap<RequestId, Outgoing>,
    /// How many requests this peer keeps open with the other peer, once decided.
    pub in_flight: Option<u32>,
    /// The other peer's round-trip time.
    pub rtt: RttEstimate,
    /// The rate the other peer delivers Data to this peer at.
    pub delivery: DeliveryRate,
    /// Whether the other peer is snubbing this peer: it sent no Data for too long while
    /// requests were open.
    pub snubbed: bool,
    /// When the other peer last made progress on this peer's chunk requests: sent Data, or got
    /// its first open request after having none.
    pub progress_at: Time,
    /// When Data last went either way.
    pub exchanged_at: Time,
    /// When this peer may next send Peers.
    pub next_peers: Option<Time>,
    /// Whether the other peer has lacked chunks while connected, so that it having every chunk
    /// now means it received them.
    pub was_incomplete: bool,
    /// While superseeding: the chunks announced to the other peer, and the range offered to it.
    pub offer: Option<Offer>,
    /// No requests are sent on the connection before this time, after a Reject.
    pub retry_at: Option<Time>,
    /// Whether the other peer refused this peer the manifest.
    pub lacks_manifest: bool,
    /// Requests that arrived before the opening sequence completed, in arrival order.
    pub held: Vec<(StreamId, Message)>,
    /// The other peer's open requests that this peer has accepted.
    pub incoming: BTreeMap<StreamId, Incoming>,
    /// Whether this peer has sent Candidates.
    pub sent_candidates: bool,
    /// Whether the other peer has sent Candidates.
    pub received_candidates: bool,
    /// Bytes of verified Data received.
    pub received: Meter,
    /// Bytes of Data sent.
    pub sent: Meter,
}

/// One of this peer's open requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Outgoing {
    pub request: OutRequest,
    pub sent_at: Time,
    /// Bytes received on the connection when the request was sent, to measure the delivery
    /// rate when its response arrives.
    pub received_before: u64,
}

/// What a superseeding peer has announced to one other peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Offer {
    /// Every chunk announced to it.
    pub announced: ChunkSet,
    /// The ranges offered to it that it doesn't have every chunk of yet, as (start, count).
    pub pending: Vec<(u32, u32)>,
    /// How many ranges it may have on offer at once.
    pub window: u32,
}

/// An accepted request for chunks, waiting for or using an upload slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Incoming {
    pub start: u32,
    pub count: u32,
    pub bytes: u64,
    pub sending: bool,
}

/// What this peer knows about the chunks the other peer has.
#[derive(Debug)]
pub(super) enum Holdings {
    /// Without the manifest, Have and HaveUpdates can't be checked or stored as a bitmap, so they
    /// are kept as received and applied once the manifest arrives.
    Pending {
        updates: Vec<HaveUpdate>,
        ranges: usize,
    },
    /// The other peer's chunks, checked against the manifest.
    Known(ChunkSet),
}

impl Conn {
    pub fn new(info: ConnInfo, config: &Config, now: Time) -> Self {
        Self {
            info,
            sent_opening: false,
            hello: None,
            connected_at: now,
            opened_at: None,
            holdings: Holdings::Pending {
                updates: Vec::new(),
                ranges: 0,
            },
            interesting: 0,
            they_want: false,
            we_want: false,
            serving_us: false,
            we_serve: false,
            out: BTreeMap::new(),
            in_flight: None,
            rtt: RttEstimate::default(),
            delivery: DeliveryRate::default(),
            snubbed: false,
            progress_at: now,
            exchanged_at: now,
            next_peers: None,
            was_incomplete: false,
            offer: None,
            retry_at: None,
            lacks_manifest: false,
            held: Vec::new(),
            incoming: BTreeMap::new(),
            sent_candidates: false,
            received_candidates: false,
            received: Meter::new(config.upload.rate_window, now),
            sent: Meter::new(config.upload.rate_window, now),
        }
    }

    pub fn peer(&self) -> PeerId {
        self.info.peer
    }

    pub fn is_open(&self) -> bool {
        self.opened_at.is_some()
    }

    /// Whether requests may be sent on the connection at `now`.
    pub fn may_request(&self, now: Time) -> bool {
        self.is_open() && self.retry_at.is_none_or(|t| t <= now)
    }

    /// Whether the other peer has every chunk, as far as this peer knows.
    pub fn is_complete(&self) -> bool {
        matches!(&self.holdings, Holdings::Known(set) if set.is_full())
    }

    /// This peer's open chunk requests on the connection, as `(start, count)`.
    pub fn chunk_requests(&self) -> impl Iterator<Item = (RequestId, u32, u32)> + '_ {
        self.out.iter().filter_map(|(&id, out)| match out.request {
            OutRequest::Chunks { start, count } => Some((id, start, count)),
            OutRequest::Manifest => None,
        })
    }

    /// The number of the other peer's requests that are open: held or accepted.
    pub fn open_incoming(&self) -> usize {
        self.held.len() + self.incoming.len()
    }
}

/// Buffers a Have or HaveUpdate received before the manifest. Returns `false` if that would keep
/// more than `max` ranges.
pub(super) fn buffer_update(holdings: &mut Holdings, update: HaveUpdate, max: usize) -> bool {
    let Holdings::Pending { updates, ranges } = holdings else {
        unreachable!("only buffered before the manifest");
    };
    *ranges += update.added.ranges().len() + update.removed.ranges().len();
    updates.push(update);
    *ranges <= max
}

/// A HaveUpdate made from a Have.
pub(super) fn have_as_update(ranges: RangeList) -> HaveUpdate {
    HaveUpdate {
        added: ranges,
        removed: RangeList::new(),
    }
}
