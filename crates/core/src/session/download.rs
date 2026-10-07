//! The requester's side: fetching the manifest, requesting chunks and handling responses.

use std::sync::Arc;

use tempest_protocol::hash::manifest_hash;
use tempest_protocol::message::{GoodbyeReason, HaveUpdate, ManifestMessage, RejectReason};
use tempest_protocol::{Decode, Manifest, Message, RangeList};

use super::bundle::Bundle;
use super::conn::Holdings;
use super::{Fail, Outcome, Session, violation};
use crate::chunks::ChunkSet;
use crate::event::{Event, EventKind};
use crate::io::{Action, ConnId, OutRequest, RequestId, Response};
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
        conn.out.insert(request, OutRequest::Manifest);
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

    /// Requests chunks from a provider until its share of requests in flight is used.
    pub(super) fn fill(&mut self, now: Time, id: ConnId) {
        let Some(bundle) = &mut self.bundle else {
            return;
        };
        let Some(conn) = self.conns.get_mut(&id) else {
            return;
        };
        if !conn.serving_us || !conn.may_request(now) {
            return;
        }
        let (Some(hello), Holdings::Known(provider)) = (&conn.hello, &conn.holdings) else {
            return;
        };
        let headroom_limit = hello
            .max_open_requests
            .saturating_sub(self.config.request_headroom)
            .max(1);
        let limit = self.config.requests_in_flight.min(headroom_limit) as usize;
        let max_count = self.config.request_chunks.min(hello.max_request_chunks);
        let peer = conn.info.peer;

        while conn.out.len() < limit {
            let ctx = SelectContext {
                have: &bundle.have,
                needed: &bundle.needed,
                provider,
                availability: &bundle.availability,
            };
            let Some((chunk, reason)) = self.selector.select(&ctx, &mut self.rng) else {
                break;
            };
            let (start, count) = bundle.request_around(chunk, max_count, provider);
            bundle.mark_requested(start, count);
            let request = next_id(&mut self.next_request);
            let message = OutRequest::Chunks { start, count };
            conn.out.insert(request, message);
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
    }

    pub(super) fn on_response(
        &mut self,
        now: Time,
        id: ConnId,
        request: RequestId,
        response: Response,
    ) -> Outcome {
        let conn = self.conns.get_mut(&id).expect("open");
        let Some(sent) = conn.out.remove(&request) else {
            // A response racing a cancel, or one already handled.
            return Ok(());
        };
        let peer = conn.peer();
        match (sent, response) {
            (OutRequest::Chunks { start, count }, Response::Data(Ok(()))) => {
                self.on_chunks(now, id, start, count);
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
                conn.retry_at = Some(now + self.config.retry_delay);
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
                    conn.retry_at = Some(now + self.config.retry_delay);
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

    /// Records verified chunks, tells every peer, and keeps requesting.
    fn on_chunks(&mut self, now: Time, id: ConnId, start: u32, count: u32) {
        let bundle = self
            .bundle
            .as_mut()
            .expect("chunk requests need the manifest");
        let added: Vec<u32> = bundle
            .span(start, count)
            .filter(|&chunk| bundle.have.insert(chunk))
            .collect();
        let bytes = bundle.data_len(start, count);
        for conn in self.conns.values_mut() {
            if let Holdings::Known(set) = &conn.holdings {
                let lost = added.iter().filter(|&&c| set.contains(c)).count() as u32;
                conn.interesting -= lost;
            }
        }
        let complete = bundle.is_complete();

        let conn = self.conns.get_mut(&id).expect("open");
        conn.received.record(now, bytes);
        self.downloaded += bytes;
        let peer = conn.peer();
        self.emit(
            now,
            EventKind::Received {
                conn: id,
                peer,
                start,
                count,
                bytes,
            },
        );

        self.announce(RangeList::from_chunks(added));
        if complete && !self.complete {
            self.complete = true;
            self.emit(now, EventKind::Completed);
            self.review_serving(now);
        }
        self.fill(now, id);
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
            self.complete = true;
            self.emit(now, EventKind::Completed);
        }
        self.review_serving(now);
        self.fill_all(now);
        Ok(())
    }
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
