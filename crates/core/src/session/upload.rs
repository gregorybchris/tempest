//! The provider's side: answering requests and deciding whom to serve.

use tempest_protocol::Message;
use tempest_protocol::message::{Reject, RejectReason, Serving};

use super::conn::Incoming;
use super::{Outcome, Session, violation};
use crate::event::EventKind;
use crate::io::{Action, ConnId, Reply, StreamId};
use crate::time::Time;
use crate::upload::{PeerView, QueuedUpload, UploadContext};

impl Session {
    fn reject(&mut self, conn: ConnId, stream: StreamId, reason: RejectReason, message: String) {
        self.actions.push(Action::Respond {
            conn,
            stream,
            reply: Reply::Reject(Reject { reason, message }),
        });
    }

    /// Answers a request the other peer opened a stream for, or queues it for an upload slot.
    pub(super) fn on_request(
        &mut self,
        now: Time,
        id: ConnId,
        stream: StreamId,
        message: Message,
    ) -> Outcome {
        if !matches!(message, Message::Request(_) | Message::ManifestRequest) {
            return Err(violation(format!(
                "{:?} sent on a request stream",
                message.message_type()
            )));
        }
        let max_open = self.config.upload.max_open_requests as usize;
        let conn = self.conns.get_mut(&id).expect("open");
        if conn.open_incoming() >= max_open {
            let message = "too many open requests".to_owned();
            self.reject(id, stream, RejectReason::Busy, message);
            return Ok(());
        }
        if !conn.is_open() {
            // Streams can overtake the opening sequence; answer them once it completes.
            conn.held.push((stream, message));
            return Ok(());
        }

        let Some(bundle) = &self.bundle else {
            let message = "no manifest yet".to_owned();
            self.reject(id, stream, RejectReason::DontHave, message);
            return Ok(());
        };
        let Message::Request(request) = message else {
            self.actions.push(Action::Respond {
                conn: id,
                stream,
                reply: Reply::Manifest,
            });
            return Ok(());
        };
        let (start, count) = (request.start, request.count);
        // While superseeding, only the chunks announced to the requester are offered.
        let offered = conn.offer.as_ref().map_or(&bundle.have, |o| &o.announced);
        let refusal = if count > self.config.upload.max_request_chunks {
            Some((
                RejectReason::Invalid,
                "count exceeds max_request_chunks".to_owned(),
            ))
        } else if let Err(error) = bundle.layout.request_node(start, count) {
            Some((RejectReason::Invalid, error.to_string()))
        } else if !conn.we_serve {
            Some((RejectReason::Busy, "not serving you now".to_owned()))
        } else if !bundle.span(start, count).all(|c| offered.contains(c)) {
            Some((RejectReason::DontHave, String::new()))
        } else {
            None
        };
        if let Some((reason, message)) = refusal {
            self.reject(id, stream, reason, message);
            return Ok(());
        }
        let bytes = bundle.data_len(start, count);
        let incoming = Incoming {
            start,
            count,
            bytes,
            sending: false,
        };
        if conn.incoming.insert(stream, incoming).is_none() {
            self.upload_queue.push((id, stream));
        }
        self.pump_uploads(now);
        Ok(())
    }

    /// The other peer reset one of its request streams.
    pub(super) fn on_reset(&mut self, now: Time, id: ConnId, stream: StreamId) {
        let conn = self.conns.get_mut(&id).expect("open");
        conn.held.retain(|(s, _)| *s != stream);
        if conn.incoming.remove(&stream).is_some_and(|i| i.sending) {
            self.uploading -= 1;
            self.pump_uploads(now);
        }
    }

    /// The driver finished sending Data on a stream.
    pub(super) fn on_data_sent(&mut self, now: Time, id: ConnId, stream: StreamId) {
        let conn = self.conns.get_mut(&id).expect("open");
        let Some(&incoming) = conn.incoming.get(&stream).filter(|i| i.sending) else {
            return;
        };
        conn.incoming.remove(&stream);
        conn.sent.record(now, incoming.bytes);
        conn.exchanged_at = now;
        let peer = conn.peer();
        self.uploading -= 1;
        self.uploaded += incoming.bytes;
        self.emit(
            now,
            EventKind::Uploaded {
                conn: id,
                peer,
                start: incoming.start,
                count: incoming.count,
                bytes: incoming.bytes,
            },
        );
        self.record_sent(now, incoming.start, incoming.count);
        self.pump_uploads(now);
        self.check_linger(now);
    }

    /// Starts sending queued Data while upload slots are free, in the upload strategy's order.
    pub(super) fn pump_uploads(&mut self, now: Time) {
        let conns = &self.conns;
        self.upload_queue.retain(|(id, stream)| {
            conns
                .get(id)
                .and_then(|c| c.incoming.get(stream))
                .is_some_and(|i| !i.sending)
        });
        let slots = self.config.upload.parallel_responses;
        if self.upload_queue.is_empty() || self.uploading >= slots {
            return;
        }
        let mut peers = self.peer_views(now);
        while self.uploading < slots && !self.upload_queue.is_empty() {
            let queue: Vec<QueuedUpload> = self
                .upload_queue
                .iter()
                .map(|&(conn, stream)| QueuedUpload {
                    conn,
                    stream,
                    bytes: self.conns[&conn].incoming[&stream].bytes,
                })
                .collect();
            let ctx = UploadContext {
                now,
                complete: self.complete,
                peers: &peers,
            };
            let index = self.uploader.next_upload(&queue, &ctx);
            let (id, stream) = self.upload_queue.remove(index);
            let incoming = self
                .conns
                .get_mut(&id)
                .and_then(|c| c.incoming.get_mut(&stream))
                .expect("queued uploads are pending");
            incoming.sending = true;
            let (start, count) = (incoming.start, incoming.count);
            if let Some(view) = peers.iter_mut().find(|p| p.conn == id) {
                view.in_progress += incoming.bytes;
            }
            self.uploading += 1;
            self.actions.push(Action::Respond {
                conn: id,
                stream,
                reply: Reply::Data { start, count },
            });
        }
    }

    /// What the upload strategy sees of each open connection.
    fn peer_views(&mut self, now: Time) -> Vec<PeerView> {
        self.conns
            .iter_mut()
            .filter(|(_, c)| c.is_open())
            .map(|(&conn, c)| PeerView {
                conn,
                peer: c.peer(),
                wants: c.they_want,
                serving: c.we_serve,
                serves_us: c.serving_us,
                complete: c.is_complete(),
                received: c.received.total(),
                sent: c.sent.total(),
                in_progress: c
                    .incoming
                    .values()
                    .filter(|i| i.sending)
                    .map(|i| i.bytes)
                    .sum(),
                receive_rate: c.received.rate(now),
                send_rate: c.sent.rate(now),
            })
            .collect()
    }

    /// Asks the upload strategy whom to serve, and sends Serving where that changed.
    pub(super) fn review_serving(&mut self, now: Time) {
        let peers = self.peer_views(now);
        let ctx = UploadContext {
            now,
            complete: self.complete,
            peers: &peers,
        };
        let decisions = self.uploader.review(&ctx, &mut self.rng);
        for decision in decisions {
            let Some(conn) = self.conns.get_mut(&decision.conn) else {
                continue;
            };
            if !conn.is_open() || conn.we_serve == decision.serve {
                continue;
            }
            conn.we_serve = decision.serve;
            let peer = conn.peer();
            let serving = decision.serve;
            // Requests waiting for an upload slot are refused, so that a peer no longer served
            // gets no more than what is already being sent.
            let waiting: Vec<StreamId> = if serving {
                Vec::new()
            } else {
                conn.incoming
                    .iter()
                    .filter(|(_, i)| !i.sending)
                    .map(|(&stream, _)| stream)
                    .collect()
            };
            for stream in &waiting {
                conn.incoming.remove(stream);
            }
            self.send(decision.conn, Message::Serving(Serving { serving }));
            for stream in waiting {
                let message = "not serving you now".to_owned();
                self.reject(decision.conn, stream, RejectReason::Busy, message);
            }
            self.emit(
                now,
                EventKind::ServingChanged {
                    conn: decision.conn,
                    peer,
                    serving,
                    reason: decision.reason,
                },
            );
        }
    }
}
