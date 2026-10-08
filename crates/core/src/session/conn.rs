//! The session's state for one connection.

use std::collections::BTreeMap;

use tempest_protocol::message::{GoodbyeReason, HaveUpdate, Hello};
use tempest_protocol::{ChunkRange, Message, PeerId, RangeList};

use super::{Fail, violation};
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
    /// Without the manifest, holdings can't be stored as a bitmap or checked against the
    /// bundle's size, so they are kept as ranges and applied once the manifest arrives.
    Pending(PendingHoldings),
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
            holdings: Holdings::Pending(PendingHoldings::default()),
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

/// The chunks the other peer has, as far as its Have and HaveUpdates said before the manifest
/// arrived. Each update is folded in as it arrives, checking every rule but the bundle's size,
/// so its cost depends on the ranges it names, not on how many updates came before.
#[derive(Debug, Default)]
pub(super) struct PendingHoldings {
    /// Disjoint, non-touching ranges, as `start -> end`.
    held: BTreeMap<u32, u32>,
    /// Whether any update has arrived.
    updated: bool,
    /// Whether an update after the Have added chunks: the peer lacked them, so it was
    /// incomplete while connected.
    grew: bool,
}

impl PendingHoldings {
    /// Folds in `update`, failing if it adds a chunk already held, removes one not held, or
    /// would leave more than `max` ranges.
    pub fn apply(&mut self, update: &HaveUpdate, max: usize) -> Result<(), Fail> {
        if update
            .added
            .ranges()
            .iter()
            .any(|r| self.overlaps(r.start, r.end))
        {
            return Err(violation("HaveUpdate added a chunk already held"));
        }
        if !update
            .removed
            .ranges()
            .iter()
            .all(|r| self.covers(r.start, r.end))
        {
            return Err(violation("HaveUpdate removed a chunk not held"));
        }
        for range in update.added.ranges() {
            self.insert(range.start, range.end);
        }
        for range in update.removed.ranges() {
            self.remove(range.start, range.end);
        }
        self.grew |= self.updated && !update.added.is_empty();
        self.updated = true;
        if self.held.len() > max {
            // This peer's own limit, not a rule of the protocol.
            return Err(Fail {
                reason: GoodbyeReason::Done,
                message: "too many Have ranges before the manifest".to_owned(),
            });
        }
        Ok(())
    }

    /// The chunks held, and whether the peer was incomplete at some point.
    pub fn finish(self) -> (RangeList, bool) {
        let ranges = (self.held.into_iter())
            .map(|(start, end)| ChunkRange { start, end })
            .collect();
        let held = RangeList::from_ranges(ranges).expect("kept canonical");
        (held, self.grew)
    }

    /// The held range that starts last before `end`, if any.
    fn before(&self, end: u32) -> Option<(u32, u32)> {
        self.held.range(..end).next_back().map(|(&s, &e)| (s, e))
    }

    fn overlaps(&self, start: u32, end: u32) -> bool {
        self.before(end).is_some_and(|(_, e)| e > start)
    }

    fn covers(&self, start: u32, end: u32) -> bool {
        self.before(start + 1).is_some_and(|(_, e)| e >= end)
    }

    /// Adds `[start, end)`, which overlaps nothing held, merging touching ranges.
    fn insert(&mut self, mut start: u32, mut end: u32) {
        if let Some((s, e)) = self.before(start + 1)
            && e == start
        {
            self.held.remove(&s);
            start = s;
        }
        if let Some(e) = self.held.remove(&end) {
            end = e;
        }
        self.held.insert(start, end);
    }

    /// Removes `[start, end)`, which lies within one held range.
    fn remove(&mut self, start: u32, end: u32) {
        let (s, e) = self.before(start + 1).expect("checked to be held");
        self.held.remove(&s);
        if s < start {
            self.held.insert(s, start);
        }
        if end < e {
            self.held.insert(end, e);
        }
    }
}

/// A HaveUpdate made from a Have.
pub(super) fn have_as_update(ranges: RangeList) -> HaveUpdate {
    HaveUpdate {
        added: ranges,
        removed: RangeList::new(),
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn update(added: &[u32], removed: &[u32]) -> HaveUpdate {
        HaveUpdate {
            added: RangeList::from_chunks(added.iter().copied()),
            removed: RangeList::from_chunks(removed.iter().copied()),
        }
    }

    #[test]
    fn updates_fold_into_one_set() {
        let mut pending = PendingHoldings::default();
        pending.apply(&update(&[0, 1, 2, 5], &[]), 10).unwrap();
        pending.apply(&update(&[3, 4], &[1]), 10).unwrap();
        let (held, grew) = pending.finish();
        assert_eq!(held, RangeList::from_chunks([0, 2, 3, 4, 5]));
        assert!(grew);
    }

    #[test]
    fn rules_are_checked_as_updates_arrive() {
        let mut pending = PendingHoldings::default();
        pending.apply(&update(&[2, 3], &[]), 10).unwrap();
        let added_twice = pending.apply(&update(&[3], &[]), 10).unwrap_err();
        assert_eq!(added_twice.reason, GoodbyeReason::ProtocolViolation);
        let removed_unheld = pending.apply(&update(&[], &[1, 2]), 10).unwrap_err();
        assert_eq!(removed_unheld.reason, GoodbyeReason::ProtocolViolation);
        let too_many = pending.apply(&update(&[5, 7, 9], &[]), 2).unwrap_err();
        assert_eq!(too_many.reason, GoodbyeReason::Done);
    }

    proptest! {
        /// Folding matches applying each update to a plain set of chunks.
        #[test]
        fn folding_matches_a_set(steps in prop::collection::vec(
            (prop::collection::btree_set(0u32..40, 0..8), prop::collection::btree_set(0u32..40, 0..8)),
            1..30,
        )) {
            let mut pending = PendingHoldings::default();
            let mut set = std::collections::BTreeSet::new();
            for (added, removed) in steps {
                let valid = added.iter().all(|c| !set.contains(c))
                    && removed.iter().all(|c| set.contains(c));
                let added: Vec<u32> = added.into_iter().collect();
                let removed: Vec<u32> = removed.into_iter().collect();
                let result = pending.apply(&update(&added, &removed), usize::MAX);
                prop_assert_eq!(result.is_ok(), valid);
                if !valid {
                    break;
                }
                set.extend(added.iter().copied());
                for c in &removed {
                    set.remove(c);
                }
            }
            let (held, _) = pending.finish();
            prop_assert_eq!(held, RangeList::from_chunks(set));
        }
    }
}
