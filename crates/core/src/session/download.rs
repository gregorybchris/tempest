//! The requester's side: fetching the manifest, requesting chunks, endgame, and handling
//! responses.

use std::sync::Arc;

use rand::Rng as _;
use tempest_protocol::hash::manifest_hash;
use tempest_protocol::message::{GoodbyeReason, HaveUpdate, ManifestMessage, RejectReason};
use tempest_protocol::tree::CHUNK_SIZE;
use tempest_protocol::{Decode, Manifest, Message, RangeList};

use super::bundle::Bundle;
use super::conn::{Holdings, Outgoing};
use super::{Fail, Outcome, Session, violation};
use crate::chunks::ChunkSet;
use crate::config::WhenComplete;
use crate::event::{CancelReason, Event, EventKind, SelectReason};
use crate::io::{Action, ConnId, OutRequest, RequestId, Response};
use crate::pipeline::{Provider, requests_in_flight};
use crate::select::SelectContext;
use crate::time::Time;

impl Session {
    /// Asks one connected peer for the manifest, if it is still needed and nobody is being asked.
    pub(super) fn request_manifest(&mut self, now: Time) {
        if self.bundle.is_some() || self.manifest_request.is_some() {
            return;
        }
        let Some((&id, _)) = self
            .conns
            .iter()
            .find(|(_, c)| c.may_request(now) && !c.lacks_manifest)
        else {
            return;
        };
        let request = next_id(&mut self.next_request);
        let conn = self.conns.get_mut(&id).expect("open");
        let outgoing = Outgoing {
            request: OutRequest::Manifest,
            sent_at: now,
            received_before: conn.received.total(),
        };
        conn.out.insert(request, outgoing);
        self.manifest_request = Some((id, request));
        self.actions.push(Action::Request {
            conn: id,
            request,
            message: OutRequest::Manifest,
        });
    }

    /// Requests chunks on every connection that has room.
    pub(super) fn fill_all(&mut self, now: Time) {
        let ids: Vec<ConnId> = self.conns.keys().copied().collect();
        for id in ids {
            self.fill(now, id);
        }
    }

    /// Requests chunks from a provider until it has as many requests open as its pipeline
    /// should hold (D-060). Once every missing chunk is requested, requests chunks again from
    /// other providers as the endgame strategy allows (D-061).
    pub(super) fn fill(&mut self, now: Time, id: ConnId) {
        let Some(bundle) = &mut self.bundle else {
            return;
        };
        let Some(conn) = self.conns.get_mut(&id) else {
            return;
        };
        if !conn.serving_us || !conn.may_request(now) || bundle.is_complete() {
            return;
        }
        let (Some(hello), Holdings::Known(provider)) = (&conn.hello, &conn.holdings) else {
            return;
        };
        let peer = conn.info.peer;
        let max_count = self.config.requests.chunks.min(hello.max_request_chunks);
        let limit = hello
            .max_open_requests
            .saturating_sub(self.config.requests.headroom)
            .max(1);
        let (target, reason) = requests_in_flight(
            &self.config.requests,
            Provider {
                rate: conn.delivery.get(),
                rtt: conn.rtt.get(),
                request_bytes: u64::from(max_count) * CHUNK_SIZE as u64,
                limit,
                snubbed: conn.snubbed,
            },
        );
        if conn.in_flight != Some(target) {
            conn.in_flight = Some(target);
            self.events.push(Event {
                time: now,
                kind: EventKind::InFlightChanged {
                    conn: id,
                    peer,
                    requests: target,
                    reason,
                },
            });
        }
        if !bundle.in_endgame() {
            self.endgame = false;
        }
        let max_copies = self
            .config
            .strategies
            .endgame
            .max_copies(&self.config.endgame);
        let idle_before = conn.chunk_requests().next().is_none();

        while conn.out.len() < target as usize {
            let pick = if bundle.needed.is_empty() {
                None
            } else if conn.snubbed {
                // A snubbing provider gets only chunks nobody else has, so that it can't hold
                // up chunks others could send.
                let only_here = bundle.availability.level(1);
                only_here
                    .and_then(|level| {
                        ChunkSet::random_common(&[level, &bundle.needed, provider], &mut self.rng)
                    })
                    .map(|chunk| {
                        let needed = &bundle.needed;
                        let range = bundle.request_around(chunk, 1, |c| {
                            needed.contains(c) && provider.contains(c)
                        });
                        (range, SelectReason::Rarest { holders: 1 })
                    })
            } else {
                let ctx = SelectContext {
                    have: &bundle.have,
                    needed: &bundle.needed,
                    provider,
                    availability: &bundle.availability,
                    position: self.position,
                };
                self.selector
                    .select(&ctx, &mut self.rng)
                    .map(|(chunk, reason)| {
                        let needed = &bundle.needed;
                        let range = bundle.request_around(chunk, max_count, |c| {
                            needed.contains(c) && provider.contains(c)
                        });
                        (range, reason)
                    })
            };
            let pick = match pick {
                Some(pick) => Some(pick),
                None if bundle.in_endgame() => {
                    let open: Vec<_> = conn
                        .chunk_requests()
                        .map(|(_, start, count)| bundle.span(start, count))
                        .collect();
                    let eligible = |c: u32| {
                        !bundle.have.contains(c)
                            && provider.contains(c)
                            && bundle.requests[c as usize] < max_copies
                            && !open.iter().any(|r| r.contains(&c))
                    };
                    endgame_pick(bundle, eligible, &mut self.rng).map(|(chunk, copies)| {
                        let range = bundle.request_around(chunk, max_count, eligible);
                        (range, SelectReason::Endgame { copies })
                    })
                }
                None => None,
            };
            let Some(((start, count), reason)) = pick else {
                break;
            };
            if matches!(reason, SelectReason::Endgame { .. }) && !self.endgame {
                self.endgame = true;
                let missing = bundle.layout.chunk_count() - bundle.have.len();
                self.events.push(Event {
                    time: now,
                    kind: EventKind::EndgameStarted { missing },
                });
            }
            bundle.mark_requested(start, count);
            let request = next_id(&mut self.next_request);
            let message = OutRequest::Chunks { start, count };
            let outgoing = Outgoing {
                request: message,
                sent_at: now,
                received_before: conn.received.total(),
            };
            conn.out.insert(request, outgoing);
            self.actions.push(Action::Request {
                conn: id,
                request,
                message,
            });
            self.events.push(Event {
                time: now,
                kind: EventKind::Requested {
                    conn: id,
                    peer,
                    start,
                    count,
                    reason,
                },
            });
        }
        if idle_before && conn.chunk_requests().next().is_some() {
            conn.progress_at = now;
        }
    }

    /// Cancels every chunk request to providers that have sent no Data for too long, and gives
    /// them one request at a time until they do.
    pub(super) fn check_snubs(&mut self, now: Time) {
        let timeout = self.config.requests.snub_timeout;
        let snubbing: Vec<ConnId> = self
            .conns
            .iter()
            .filter(|(_, c)| c.chunk_requests().next().is_some() && c.progress_at + timeout <= now)
            .map(|(&id, _)| id)
            .collect();
        for id in &snubbing {
            let conn = self.conns.get_mut(id).expect("listed");
            conn.snubbed = true;
            let peer = conn.peer();
            self.emit(now, EventKind::Snubbed { conn: *id, peer });
            let requests: Vec<_> = self.conns[id].chunk_requests().collect();
            for (request, start, count) in requests {
                self.cancel(now, *id, request, start, count, CancelReason::Snubbed);
            }
        }
        if !snubbing.is_empty() {
            self.fill_all(now);
        }
    }

    /// Resets one of this peer's chunk requests and makes its chunks available to others.
    fn cancel(
        &mut self,
        now: Time,
        id: ConnId,
        request: RequestId,
        start: u32,
        count: u32,
        reason: CancelReason,
    ) {
        let conn = self.conns.get_mut(&id).expect("open");
        conn.out.remove(&request);
        let peer = conn.peer();
        self.release(start, count);
        self.actions.push(Action::Cancel { conn: id, request });
        let kind = EventKind::Cancelled {
            conn: id,
            peer,
            start,
            count,
            reason,
        };
        self.emit(now, kind);
    }

    pub(super) fn on_response(
        &mut self,
        now: Time,
        id: ConnId,
        request: RequestId,
        response: Response,
    ) -> Outcome {
        let conn = self.conns.get_mut(&id).expect("open");
        let Some(outgoing) = conn.out.remove(&request) else {
            // A response racing a cancel, or one already handled.
            return Ok(());
        };
        let sent = outgoing.request;
        conn.rtt.sample(now.since(outgoing.sent_at));
        let peer = conn.peer();
        match (sent, response) {
            (OutRequest::Chunks { start, count }, Response::Data(Ok(()))) => {
                self.on_chunks(now, id, (start, count), &outgoing);
                Ok(())
            }
            (OutRequest::Chunks { start, count }, Response::Data(Err(error))) => {
                self.release(start, count);
                Err(Fail {
                    reason: GoodbyeReason::BadData,
                    message: format!("Data for chunks from {start} failed verification: {error}"),
                })
            }
            (OutRequest::Chunks { start, count }, Response::Reject(reject)) => {
                conn.retry_at = Some(now + self.config.requests.retry_delay);
                let reason = reject.reason;
                self.emit(
                    now,
                    EventKind::RequestRejected {
                        conn: id,
                        peer,
                        reason,
                    },
                );
                self.release(start, count);
                self.fill_all(now);
                Ok(())
            }
            (OutRequest::Manifest, Response::Manifest(manifest)) => {
                self.manifest_request = None;
                self.on_manifest(now, id, manifest)
            }
            (OutRequest::Manifest, Response::Reject(reject)) => {
                self.manifest_request = None;
                if reject.reason == RejectReason::DontHave {
                    conn.lacks_manifest = true;
                } else {
                    conn.retry_at = Some(now + self.config.requests.retry_delay);
                }
                let reason = reject.reason;
                self.emit(
                    now,
                    EventKind::RequestRejected {
                        conn: id,
                        peer,
                        reason,
                    },
                );
                self.request_manifest(now);
                Ok(())
            }
            (sent, response) => {
                match sent {
                    OutRequest::Chunks { start, count } => self.release(start, count),
                    OutRequest::Manifest => self.manifest_request = None,
                }
                Err(violation(format!(
                    "{} answered with {}",
                    match sent {
                        OutRequest::Chunks { .. } => "Request",
                        OutRequest::Manifest => "ManifestRequest",
                    },
                    response_name(&response)
                )))
            }
        }
    }

    fn release(&mut self, start: u32, count: u32) {
        self.bundle
            .as_mut()
            .expect("chunk requests need the manifest")
            .release(start, count);
    }

    /// Records verified chunks, cancels requests they make redundant, tells every peer, and
    /// keeps requesting.
    fn on_chunks(&mut self, now: Time, id: ConnId, (start, count): (u32, u32), sent: &Outgoing) {
        let bundle = self
            .bundle
            .as_mut()
            .expect("chunk requests need the manifest");
        let (added, duplicate) = bundle.store(start, count);
        bundle.release(start, count);
        let bytes = bundle.data_len(start, count);
        // Other requests still open for a new chunk are endgame duplicates.
        let duplicates = added.iter().any(|&c| bundle.requests[c as usize] > 0);
        for conn in self.conns.values_mut() {
            if let Holdings::Known(set) = &conn.holdings {
                let lost = added.iter().filter(|&&c| set.contains(c)).count() as u32;
                conn.interesting -= lost;
            }
        }
        let complete = bundle.is_complete();

        let conn = self.conns.get_mut(&id).expect("open");
        conn.received.record(now, bytes);
        let delivered = conn.received.total() - sent.received_before;
        conn.delivery.sample(delivered, now.since(sent.sent_at));
        conn.progress_at = now;
        conn.exchanged_at = now;
        conn.snubbed = false;
        self.downloaded += bytes;
        self.duplicate += duplicate;
        let peer = conn.peer();
        self.emit(
            now,
            EventKind::Received {
                conn: id,
                peer,
                start,
                count,
                bytes,
                duplicate,
            },
        );
        if duplicates {
            self.cancel_redundant(now);
        }

        self.announce(RangeList::from_chunks(added));
        if complete && !self.complete {
            self.on_complete(now);
        }
        if duplicates {
            self.fill_all(now);
        } else {
            self.fill(now, id);
        }
    }

    /// Cancels every chunk request whose chunks this peer now has.
    fn cancel_redundant(&mut self, now: Time) {
        let bundle = self.bundle.as_ref().expect("manifest known");
        let redundant: Vec<_> = self
            .conns
            .iter()
            .flat_map(|(&id, c)| c.chunk_requests().map(move |r| (id, r)))
            .filter(|&(_, (_, start, count))| {
                bundle.span(start, count).all(|c| bundle.have.contains(c))
            })
            .collect();
        for (id, (request, start, count)) in redundant {
            self.cancel(now, id, request, start, count, CancelReason::Duplicate);
        }
    }

    /// This peer has every chunk: switch to the complete peer's upload strategy.
    ///
    /// Connections with peers that are complete too stay open: those peers close them when
    /// this peer's HaveUpdate shows it complete, so the announcement isn't cut off.
    fn on_complete(&mut self, now: Time) {
        self.complete = true;
        self.emit(now, EventKind::Completed);
        let keep =
            self.custom_uploader && self.config.strategies.when_complete == WhenComplete::Unchanged;
        if !keep {
            self.uploader = self.upload_strategy();
        }
        self.idle_since = None;
        self.update_idle(now);
        self.review_serving(now);
        self.check_linger(now);
    }

    /// Sends a HaveUpdate for newly stored chunks, and updates Want everywhere.
    fn announce(&mut self, added: RangeList) {
        let ids: Vec<ConnId> = self.conns.keys().copied().collect();
        for id in ids {
            if !added.is_empty() && self.conns[&id].sent_opening {
                let update = HaveUpdate {
                    added: added.clone(),
                    removed: RangeList::new(),
                };
                self.send(id, Message::HaveUpdate(update));
            }
            self.update_want(id);
        }
    }

    /// Checks and installs the manifest, then applies every connection's buffered holdings.
    fn on_manifest(&mut self, now: Time, id: ConnId, message: ManifestMessage) -> Outcome {
        // A manifest that doesn't match the bundle ID can only come from a misbehaving peer.
        let bad = |message: String| Fail {
            reason: GoodbyeReason::BadData,
            message,
        };
        if manifest_hash(&message.manifest) != self.bundle_id {
            return Err(bad("the manifest does not match the bundle ID".to_owned()));
        }
        let manifest = Manifest::from_bytes(&message.manifest)
            .map_err(|error| bad(format!("the manifest is invalid: {error}")))?;
        let manifest = Arc::new(manifest);
        let bundle = Bundle::new(manifest.clone(), &RangeList::new()).expect("empty fits");
        let count = bundle.layout.chunk_count();
        let empty_files = bundle.have.to_ranges();
        self.bundle = Some(bundle);
        self.actions.push(Action::StoreManifest {
            manifest,
            bytes: message.manifest.into(),
        });
        let peer = self.conns[&id].peer();
        self.emit(now, EventKind::ManifestReceived { conn: id, peer });

        // Apply what each peer said it has, closing any whose ranges break the rules.
        let ids: Vec<ConnId> = self.conns.keys().copied().collect();
        let mut failed = Vec::new();
        for conn_id in ids {
            let conn = self.conns.get_mut(&conn_id).expect("listed");
            let pending =
                std::mem::replace(&mut conn.holdings, Holdings::Known(ChunkSet::new(count)));
            let Holdings::Pending { updates, .. } = pending else {
                unreachable!("holdings are pending until the manifest");
            };
            for update in updates {
                if let Err(fail) = self.apply_holdings(conn_id, &update) {
                    failed.push((conn_id, fail));
                    break;
                }
            }
        }
        for (conn_id, fail) in failed {
            self.fail(now, conn_id, fail);
        }

        self.announce(empty_files);
        if self.bundle.as_ref().is_some_and(Bundle::is_complete) {
            self.on_complete(now);
        }
        self.review_serving(now);
        self.fill_all(now);
        Ok(())
    }
}

/// The endgame's next chunk: among the chunks `eligible` accepts, one with the fewest open
/// requests, chosen at random. Returns it with its number of open requests.
fn endgame_pick(
    bundle: &Bundle,
    eligible: impl Fn(u32) -> bool,
    rng: &mut crate::Rng,
) -> Option<(u32, u32)> {
    let mut best: Option<(u32, u32)> = None;
    let mut ties = 0u32;
    for chunk in bundle.have.iter_missing().filter(|&c| eligible(c)) {
        let copies = bundle.requests[chunk as usize];
        match best {
            Some((_, fewest)) if copies > fewest => {}
            Some((_, fewest)) if copies == fewest => {
                // Reservoir sampling breaks ties uniformly.
                ties += 1;
                if rng.random_range(0..ties) == 0 {
                    best = Some((chunk, copies));
                }
            }
            _ => {
                best = Some((chunk, copies));
                ties = 1;
            }
        }
    }
    best
}

/// Takes the next request ID from a session's counter.
fn next_id(counter: &mut u64) -> RequestId {
    *counter += 1;
    RequestId(*counter - 1)
}

fn response_name(response: &Response) -> String {
    match response {
        Response::Data(_) => "Data".to_owned(),
        Response::Reject(_) => "Reject".to_owned(),
        Response::Manifest(_) => "Manifest".to_owned(),
        Response::Other(message) => format!("{:?}", message.message_type()),
    }
}
