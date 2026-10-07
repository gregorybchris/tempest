//! Superseeding (D-063): a bundle's only complete copy offers each chunk to only one peer until
//! every chunk has been sent once.
//!
//! Have and HaveUpdate describe the chunks a peer "has and is willing to provide", so a
//! superseeding peer announces only what it offers. Each connected peer is offered one range of
//! chunks, of the configured request size, that nobody has been sent yet. Once that peer has the
//! whole range, which it shows by announcing it, it is offered another. Superseeding ends when
//! every chunk has been sent once, or when another peer turns out to have every chunk; then
//! everything is announced to everyone.

use std::ops::Range;

use tempest_protocol::Message;
use tempest_protocol::message::HaveUpdate;
use tempest_protocol::{ChunkRange, RangeList};

use super::Session;
use super::bundle::{Bundle, empty_file_chunks};
use super::conn::{Holdings, Offer};
use crate::chunks::ChunkSet;
use crate::event::{EventKind, SuperseedEnd};
use crate::io::ConnId;
use crate::time::Time;

/// The superseeding state.
#[derive(Debug, Clone)]
pub(super) struct Superseed {
    /// Chunks sent at least once, counting every empty file's chunk, which needs no sending.
    sent: ChunkSet,
}

impl Superseed {
    pub fn new(bundle: &Bundle) -> Self {
        let mut sent = ChunkSet::new(bundle.layout.chunk_count());
        for chunk in empty_file_chunks(&bundle.manifest, &bundle.layout) {
            sent.insert(chunk);
        }
        Self { sent }
    }
}

impl Session {
    /// The announcement state for a new connection while superseeding: only the empty files'
    /// chunks, which every Have must include.
    pub(super) fn new_offer(&self) -> Option<Offer> {
        self.superseed.as_ref()?;
        let bundle = self.bundle.as_ref()?;
        let mut announced = ChunkSet::new(bundle.layout.chunk_count());
        for chunk in empty_file_chunks(&bundle.manifest, &bundle.layout) {
            announced.insert(chunk);
        }
        Some(Offer {
            announced,
            pending: None,
        })
    }

    /// Offers a peer its next range once it has the last one.
    pub(super) fn offer_next(&mut self, now: Time, id: ConnId) {
        let (Some(bundle), Some(superseed)) = (&self.bundle, &self.superseed) else {
            return;
        };
        let conn = &self.conns[&id];
        let (Some(offer), Holdings::Known(held)) = (&conn.offer, &conn.holdings) else {
            return;
        };
        if let Some((start, count)) = offer.pending
            && !bundle.span(start, count).all(|c| held.contains(c))
        {
            return;
        }
        let elsewhere: Vec<Range<u32>> = self
            .conns
            .iter()
            .filter(|&(&other, _)| other != id)
            .filter_map(|(_, c)| c.offer.as_ref()?.pending)
            .map(|(start, count)| bundle.span(start, count))
            .collect();
        let unoffered = |c: u32| !held.contains(c) && !offer.announced.contains(c);
        let unsent = |c: u32| unoffered(c) && !superseed.sent.contains(c);
        // Prefer chunks not on offer to anyone else, then any not yet sent.
        let chunk = superseed
            .sent
            .iter_missing()
            .find(|&c| unsent(c) && !elsewhere.iter().any(|r| r.contains(&c)))
            .or_else(|| superseed.sent.iter_missing().find(|&c| unsent(c)));
        let next = chunk.map(|c| bundle.request_around(c, self.config.requests.chunks, unoffered));
        let span = next.map(|(start, count)| bundle.span(start, count));

        let conn = self.conns.get_mut(&id).expect("open");
        let offer = conn.offer.as_mut().expect("superseeding");
        offer.pending = next;
        let (Some((start, count)), Some(span)) = (next, span) else {
            return;
        };
        for chunk in span.clone() {
            offer.announced.insert(chunk);
        }
        let peer = conn.peer();
        let added = RangeList::from_ranges(vec![ChunkRange {
            start: span.start,
            end: span.end,
        }])
        .expect("one non-empty range");
        let update = HaveUpdate {
            added,
            removed: RangeList::new(),
        };
        self.send(id, Message::HaveUpdate(update));
        let kind = EventKind::Offered {
            conn: id,
            peer,
            start,
            count,
        };
        self.emit(now, kind);
    }

    /// Records chunks sent, ending superseeding once every chunk has been sent once.
    pub(super) fn record_sent(&mut self, now: Time, start: u32, count: u32) {
        let (Some(bundle), Some(superseed)) = (&self.bundle, &mut self.superseed) else {
            return;
        };
        for chunk in bundle.span(start, count) {
            superseed.sent.insert(chunk);
        }
        if superseed.sent.is_full() {
            self.end_superseeding(now, SuperseedEnd::AllSent);
        }
    }

    /// Stops superseeding: announces every chunk to every peer and switches to the complete
    /// peer's upload strategy.
    pub(super) fn end_superseeding(&mut self, now: Time, reason: SuperseedEnd) {
        if self.superseed.take().is_none() {
            return;
        }
        self.emit(now, EventKind::SuperseedingEnded { reason });
        let bundle = self
            .bundle
            .as_ref()
            .expect("superseeding needs the manifest");
        let mut updates = Vec::new();
        for (&id, conn) in &mut self.conns {
            let Some(offer) = conn.offer.take() else {
                continue;
            };
            let rest: Vec<u32> = offer.announced.iter_missing().collect();
            if conn.sent_opening && !rest.is_empty() {
                updates.push((id, RangeList::from_chunks(rest)));
            }
        }
        debug_assert!(bundle.is_complete());
        for (id, added) in updates {
            let update = HaveUpdate {
                added,
                removed: RangeList::new(),
            };
            self.send(id, Message::HaveUpdate(update));
        }
        self.uploader = self.upload_strategy();
        self.review_serving(now);
    }
}
