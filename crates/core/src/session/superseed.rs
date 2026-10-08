//! Superseeding (D-063): a bundle's only complete copy offers each chunk to only one peer until
//! every chunk has been sent once.
//!
//! Have and HaveUpdate describe the chunks a peer "has and is willing to provide", so a
//! superseeding peer announces only what it offers. Each connected peer is offered a window of
//! ranges of chunks, each of the configured request size, that nobody has been sent yet (D-223).
//! Once that peer has a whole range, which it shows by announcing it, it is offered another. The
//! window grows while no other peer is being served, so that a lone downloader isn't held to one
//! range per round trip, and shrinks to one range in a crowd. Superseeding ends when
//! every chunk has been sent once, or when another peer turns out to have every chunk; then
//! everything is announced to everyone.
//!
//! In `crowd` mode (D-223) the offers are made only while two or more connected peers lack
//! chunks. A lone such peer is announced every chunk instead, so a 1:1 send is not throttled. A
//! chunk, once announced, stays announced: when a second peer arrives, peers already shown
//! everything keep it, and only the newcomers are offered ranges, of chunks nobody has been sent.
//! When all but one such peer are gone, the one left is announced everything.

use tempest_protocol::Message;
use tempest_protocol::message::HaveUpdate;
use tempest_protocol::{ChunkRange, RangeList};

use super::Session;
use super::bundle::{Bundle, empty_file_chunks};
use super::conn::{Holdings, Offer};
use crate::chunks::ChunkSet;
use crate::config::Superseed as SuperseedMode;
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
    /// chunks, which every Have must include. In `crowd` mode, a connection that arrives while no
    /// other peer lacks chunks is announced every chunk instead.
    pub(super) fn new_offer(&self) -> Option<Offer> {
        self.superseed.as_ref()?;
        let bundle = self.bundle.as_ref()?;
        let count = bundle.layout.chunk_count();
        let alone = self.config.strategies.superseed == SuperseedMode::Crowd
            && self.peers_lacking_chunks() == 0;
        let mut announced = if alone {
            ChunkSet::full(count)
        } else {
            ChunkSet::new(count)
        };
        for chunk in empty_file_chunks(&bundle.manifest, &bundle.layout) {
            announced.insert(chunk);
        }
        Some(Offer {
            announced,
            pending: Vec::new(),
            window: 1,
        })
    }

    /// How many connections are to peers not known to have every chunk. These are the peers that
    /// "want the bundle", whatever they have been announced: a peer that has been announced
    /// everything can still want more. A connection whose opening is not complete counts.
    fn peers_lacking_chunks(&self) -> usize {
        (self.conns.values()).filter(|c| !c.is_complete()).count()
    }

    /// Whether offers are being made: always in `on` mode, in `crowd` mode while two or more
    /// connected peers lack chunks.
    fn offering(&self) -> bool {
        self.config.strategies.superseed == SuperseedMode::On || self.peers_lacking_chunks() >= 2
    }

    /// Reacts to a peer's holdings changing while superseeding: offers it ranges, or, if too few
    /// peers want the bundle, announces it everything.
    pub(super) fn review_offers(&mut self, now: Time, id: ConnId) {
        if self.offering() {
            self.offer_next(now, id);
        } else {
            self.reveal_to_lone_peer(now);
        }
    }

    /// Announces every chunk to the connected peers, if `crowd` mode is not offering. That is the
    /// case for a lone downloader, and for the one left when the others go.
    pub(super) fn reveal_to_lone_peer(&mut self, now: Time) {
        if self.superseed.is_none() || self.offering() {
            return;
        }
        let mut updates = Vec::new();
        for (&id, conn) in &mut self.conns {
            let open = conn.is_open();
            let Some(offer) = conn.offer.as_mut().filter(|_| open) else {
                continue;
            };
            let rest: Vec<u32> = offer.announced.iter_missing().collect();
            if rest.is_empty() {
                continue;
            }
            for &chunk in &rest {
                offer.announced.insert(chunk);
            }
            offer.pending.clear();
            updates.push((id, RangeList::from_chunks(rest)));
        }
        for (id, added) in updates {
            let update = HaveUpdate {
                added,
                removed: RangeList::new(),
            };
            self.send(id, Message::HaveUpdate(update));
        }
        self.review_serving(now);
    }

    /// Tops up a peer's window of offered ranges, dropping the ranges it now has.
    ///
    /// Each time the peer gains chunks, its window grows by one range, up to
    /// `upload.superseed_window`, unless this peer is serving another one; then it shrinks by
    /// one, to no less than one range. A lone downloader's window so grows until it keeps this
    /// peer's upload busy, while in a crowd each peer keeps about one range, and early arrivals
    /// don't hold ranges that later ones could be sent.
    pub(super) fn offer_next(&mut self, now: Time, id: ConnId) {
        let backlog = self
            .conns
            .iter()
            .filter(|&(&other, _)| other != id)
            .any(|(_, c)| !c.incoming.is_empty());
        let most = self.config.upload.superseed_window;
        let (Some(bundle), Some(conn)) = (&self.bundle, self.conns.get_mut(&id)) else {
            return;
        };
        let (Some(offer), Holdings::Known(held)) = (&mut conn.offer, &conn.holdings) else {
            return;
        };
        offer
            .pending
            .retain(|&(start, count)| !bundle.span(start, count).all(|c| held.contains(c)));
        offer.window = match backlog {
            true => offer.window.saturating_sub(1).max(1),
            false => offer.window.saturating_add(1).min(most),
        };
        while self.conns[&id]
            .offer
            .as_ref()
            .is_some_and(|o| o.pending.len() < o.window as usize)
        {
            if !self.offer_one(now, id) {
                break;
            }
        }
    }

    /// Offers a peer one more range, if any chunk is left to offer it.
    fn offer_one(&mut self, now: Time, id: ConnId) -> bool {
        let (Some(bundle), Some(superseed)) = (&self.bundle, &self.superseed) else {
            return false;
        };
        let conn = &self.conns[&id];
        let (Some(offer), Holdings::Known(held)) = (&conn.offer, &conn.holdings) else {
            return false;
        };
        let mut elsewhere = ChunkSet::new(bundle.layout.chunk_count());
        let ranges = self
            .conns
            .iter()
            .filter(|&(&other, _)| other != id)
            .filter_map(|(_, c)| c.offer.as_ref())
            .flat_map(|offer| &offer.pending);
        for &(start, count) in ranges {
            for chunk in bundle.span(start, count) {
                elsewhere.insert(chunk);
            }
        }
        let unoffered = |c: u32| !held.contains(c) && !offer.announced.contains(c);
        let unsent = |c: u32| unoffered(c) && !superseed.sent.contains(c);
        // Prefer chunks not on offer to anyone else. A peer with nothing on offer may also get
        // chunks on offer elsewhere and not yet sent, so that it isn't left idle; the rest of a
        // window never is, since that would send chunks twice.
        let chunk = superseed
            .sent
            .iter_missing()
            .find(|&c| unsent(c) && !elsewhere.contains(c))
            .or_else(|| {
                let idle = offer.pending.is_empty();
                idle.then(|| superseed.sent.iter_missing().find(|&c| unsent(c)))
                    .flatten()
            });
        let Some(chunk) = chunk else {
            return false;
        };
        let (start, count) = bundle.request_around(chunk, self.config.requests.chunks, unoffered);
        let span = bundle.span(start, count);
        let conn = self.conns.get_mut(&id).expect("open");
        let offer = conn.offer.as_mut().expect("superseeding");
        offer.pending.push((start, count));
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
        true
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
