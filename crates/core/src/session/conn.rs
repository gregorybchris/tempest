//! The session's state for one connection.

use std::collections::BTreeMap;

use tempest_protocol::message::{HaveUpdate, Hello};
use tempest_protocol::{Message, PeerId, RangeList};

use crate::chunks::ChunkSet;
use crate::io::{ConnInfo, OutRequest, RequestId, StreamId};
use crate::meter::Meter;
use crate::time::Time;

/// One connection, from the opening sequence until it closes.
#[derive(Debug)]
pub(super) struct Conn {
    pub info: ConnInfo,
    /// Whether this peer has sent Hello and Have.
    pub sent_opening: bool,
    /// The other peer's Hello, once received.
    pub hello: Option<Hello>,
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
    pub out: BTreeMap<RequestId, OutRequest>,
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
    /// Bytes of verified Data received.
    pub received: Meter,
    /// Bytes of Data sent.
    pub sent: Meter,
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
    pub fn new(info: ConnInfo, rate_window: std::time::Duration) -> Self {
        Self {
            info,
            sent_opening: false,
            hello: None,
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
            retry_at: None,
            lacks_manifest: false,
            held: Vec::new(),
            incoming: BTreeMap::new(),
            sent_candidates: false,
            received: Meter::new(rate_window),
            sent: Meter::new(rate_window),
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
