//! The session: one peer's participation in one bundle's swarm.
//!
//! A [`Session`] is a sans-IO state machine. Its driver reports what happened with the `handle_*`
//! methods, each given the current time, then carries out the session's [`Action`]s from
//! [`Session::poll_actions`] and calls [`Session::handle_timer`] at [`Session::next_timer`].
//! Decision events (D-006) come out of [`Session::poll_events`].
//!
//! The session is split by role: this module handles connections and the control stream,
//! `download` sends requests and handles their responses, and `upload` answers the other peers'
//! requests and decides whom to serve.

mod bundle;
mod conn;
mod download;
mod upload;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use rand::SeedableRng;
use rand::seq::IteratorRandom;
use tempest_protocol::message::{
    Candidates, GoodbyeReason, Have, HaveUpdate, Hello, MAX_CANDIDATES, PeerRecord,
};
use tempest_protocol::tree::{VerifyError, verify_data};
use tempest_protocol::{Address, BundleId, Manifest, Message, PeerId, RangeList};

use self::bundle::Bundle;
use self::conn::{Conn, Holdings, buffer_update, have_as_update};
use crate::Rng;
use crate::chunks::ChunkSet;
use crate::config::{Config, ConfigError};
use crate::event::{CloseCause, Event, EventKind};
use crate::io::{
    Action, Closed, ConnId, ConnInfo, Direction, Inbound, OutRequest, RequestId, StreamId,
};
use crate::select::ChunkSelector;
use crate::time::Time;
use crate::upload::UploadStrategy;

/// This peer, as it describes itself to others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalPeer {
    /// This peer's ID.
    pub peer_id: PeerId,
    /// Addresses at which this peer accepts connections, sent in Hello and Candidates.
    pub addresses: Vec<Address>,
    /// The relay address lent to this peer by its home relay, if it has one.
    pub relay: Option<Address>,
}

/// Why a session could not be created.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionError {
    /// A setting is out of range.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// The chunks said to be stored reach past the bundle's last chunk.
    #[error("stored chunks reach past the bundle's last chunk")]
    HaveOutOfRange,
}

/// One peer's participation in one bundle's swarm.
#[derive(Debug)]
pub struct Session {
    config: Config,
    local: LocalPeer,
    bundle_id: BundleId,
    /// Set once the manifest is known.
    bundle: Option<Bundle>,
    conns: BTreeMap<ConnId, Conn>,
    /// Peers this session may dial, with the earliest time to try each.
    known: BTreeMap<PeerId, KnownPeer>,
    dialing: BTreeSet<PeerId>,
    banned: BTreeSet<PeerId>,
    /// The one ManifestRequest in flight, if any.
    manifest_request: Option<(ConnId, RequestId)>,
    next_request: u64,
    /// Accepted chunk requests waiting for an upload slot, in arrival order. Entries whose
    /// request was reset or whose connection closed are skipped.
    upload_queue: Vec<(ConnId, StreamId)>,
    /// Data responses being sent.
    uploading: u32,
    complete: bool,
    stopped: bool,
    downloaded: u64,
    uploaded: u64,
    selector: Box<dyn ChunkSelector>,
    uploader: Box<dyn UploadStrategy>,
    rng: Rng,
    actions: Vec<Action>,
    events: Vec<Event>,
}

#[derive(Debug, Clone)]
struct KnownPeer {
    record: PeerRecord,
    next_attempt: Time,
}

/// Why a connection must close.
#[derive(Debug)]
struct Fail {
    reason: GoodbyeReason,
    message: String,
}

type Outcome = Result<(), Fail>;

fn violation(message: impl Into<String>) -> Fail {
    Fail {
        reason: GoodbyeReason::ProtocolViolation,
        message: message.into(),
    }
}

impl Session {
    /// A session that starts with nothing but the bundle ID: it first fetches the manifest.
    ///
    /// All of the session's random choices come from `seed`.
    pub fn new(
        config: Config,
        local: LocalPeer,
        bundle_id: BundleId,
        seed: u64,
    ) -> Result<Self, SessionError> {
        config.validate()?;
        Ok(Self {
            selector: config.chunk_selection.build(),
            uploader: config.upload.build(),
            config,
            local,
            bundle_id,
            bundle: None,
            conns: BTreeMap::new(),
            known: BTreeMap::new(),
            dialing: BTreeSet::new(),
            banned: BTreeSet::new(),
            manifest_request: None,
            next_request: 0,
            upload_queue: Vec::new(),
            uploading: 0,
            complete: false,
            stopped: false,
            downloaded: 0,
            uploaded: 0,
            rng: Rng::seed_from_u64(seed),
            actions: Vec::new(),
            events: Vec::new(),
        })
    }

    /// A session that has the manifest and the chunks in `have` (and every empty file's chunk).
    pub fn with_manifest(
        config: Config,
        local: LocalPeer,
        manifest: Arc<Manifest>,
        have: &RangeList,
        seed: u64,
    ) -> Result<Self, SessionError> {
        let mut session = Self::new(config, local, manifest.bundle_id(), seed)?;
        let bundle = Bundle::new(manifest, have).map_err(|_| SessionError::HaveOutOfRange)?;
        session.complete = bundle.is_complete();
        session.bundle = Some(bundle);
        Ok(session)
    }

    // ----- Accessors -----

    /// The bundle's ID.
    pub fn bundle_id(&self) -> BundleId {
        self.bundle_id
    }

    /// The manifest, once known.
    pub fn manifest(&self) -> Option<&Arc<Manifest>> {
        self.bundle.as_ref().map(|b| &b.manifest)
    }

    /// The chunks this peer has, once the manifest is known.
    pub fn have(&self) -> Option<&ChunkSet> {
        self.bundle.as_ref().map(|b| &b.have)
    }

    /// Whether this peer has every chunk.
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// Payload bytes of verified Data received.
    pub fn downloaded(&self) -> u64 {
        self.downloaded
    }

    /// Payload bytes of Data sent.
    pub fn uploaded(&self) -> u64 {
        self.uploaded
    }

    /// The number of connections, open or opening.
    pub fn connection_count(&self) -> usize {
        self.conns.len()
    }

    /// The request this peer sent as `request` on `conn`, if it is still open.
    pub fn request(&self, conn: ConnId, request: RequestId) -> Option<OutRequest> {
        self.conns.get(&conn)?.out.get(&request).copied()
    }

    /// Verifies Data received for the chunk request `request` on `conn` against the manifest.
    ///
    /// Returns `None` if that request is not an open chunk request.
    pub fn verify_data(
        &self,
        conn: ConnId,
        request: RequestId,
        data: &tempest_protocol::message::Data,
    ) -> Option<Result<(), VerifyError>> {
        let bundle = self.bundle.as_ref()?;
        let OutRequest::Chunks { start, count } = self.request(conn, request)? else {
            return None;
        };
        let location = bundle.layout.locate(start)?;
        let file = bundle.layout.files()[location.file];
        let info = bundle.manifest.entries()[file.entry].as_file()?;
        Some(verify_data(
            info.size,
            &info.root,
            location.position,
            count,
            &data.proof,
            &data.chunks,
        ))
    }

    // ----- Driver interface -----

    /// Takes the actions the driver should carry out, in order.
    pub fn poll_actions(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.actions)
    }

    /// Takes the decision events recorded since the last call, in order.
    pub fn poll_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    /// When the session next needs [`Session::handle_timer`], if ever.
    pub fn next_timer(&self) -> Option<Time> {
        let retries = self.conns.values().filter_map(|c| c.retry_at);
        let review = self.uploader.next_review();
        let dial = if self.may_dial() {
            self.dial_candidates().map(|(_, k)| k.next_attempt).min()
        } else {
            None
        };
        retries.chain(review).chain(dial).min()
    }

    /// Reports that time has reached `now`.
    pub fn handle_timer(&mut self, now: Time) {
        let due: Vec<ConnId> = self
            .conns
            .iter_mut()
            .filter(|(_, c)| c.retry_at.is_some_and(|t| t <= now))
            .map(|(&id, c)| {
                c.retry_at = None;
                id
            })
            .collect();
        for id in due {
            self.fill(now, id);
        }
        self.request_manifest(now);
        if self.uploader.next_review().is_some_and(|t| t <= now) {
            self.review_serving(now);
        }
        self.maybe_dial(now);
    }

    /// Adds peers this session may dial, such as from a tracker, a link or peer exchange.
    pub fn add_peers(&mut self, now: Time, peers: impl IntoIterator<Item = PeerRecord>) {
        for record in peers {
            if record.peer_id == self.local.peer_id || self.banned.contains(&record.peer_id) {
                continue;
            }
            self.known
                .entry(record.peer_id)
                .and_modify(|k| k.record = record.clone())
                .or_insert(KnownPeer {
                    record,
                    next_attempt: now,
                });
        }
        self.maybe_dial(now);
    }

    /// Reports that a [`Action::Dial`] failed.
    pub fn handle_dial_failed(&mut self, now: Time, peer: PeerId) {
        self.dialing.remove(&peer);
        if let Some(known) = self.known.get_mut(&peer) {
            known.next_attempt = now + self.config.redial_delay;
        }
        self.emit(now, EventKind::DialFailed { peer });
        self.maybe_dial(now);
    }

    /// Reports a new connection for this bundle, dialed or accepted.
    pub fn handle_connected(&mut self, now: Time, conn: ConnId, info: ConnInfo) {
        debug_assert!(!self.conns.contains_key(&conn), "connection IDs are unique");
        self.dialing.remove(&info.peer);
        self.emit(
            now,
            EventKind::Connected {
                conn,
                peer: info.peer,
                direction: info.direction,
                path: info.path,
            },
        );
        let refusal = if self.stopped {
            Some((GoodbyeReason::ShuttingDown, ""))
        } else if self.banned.contains(&info.peer) {
            Some((GoodbyeReason::BadData, "banned for sending bad data"))
        } else if info.peer == self.local.peer_id {
            Some((GoodbyeReason::Done, "connected to itself"))
        } else if self.conns.len() >= self.config.max_connections as usize {
            Some((GoodbyeReason::TooManyConnections, ""))
        } else {
            None
        };
        if let Some((reason, message)) = refusal {
            self.actions.push(Action::Close {
                conn,
                reason,
                message: message.to_owned(),
            });
            let cause = CloseCause::Local {
                reason,
                message: message.to_owned(),
            };
            let peer = info.peer;
            self.emit(now, EventKind::Closed { conn, peer, cause });
            return;
        }

        let mut state = Conn::new(info, self.config.rate_window);
        if let Some(bundle) = &self.bundle {
            state.holdings = Holdings::Known(ChunkSet::new(bundle.layout.chunk_count()));
        }
        self.conns.insert(conn, state);
        if info.direction == Direction::Dialed {
            self.send_opening(conn);
        }
    }

    /// Reports something that arrived on a connection.
    pub fn handle_message(&mut self, now: Time, conn: ConnId, inbound: Inbound) {
        if !self.conns.contains_key(&conn) {
            // The session closed it; anything still arriving is moot.
            return;
        }
        let outcome = match inbound {
            Inbound::Control(message) => self.on_control(now, conn, message),
            Inbound::Request { stream, message } => self.on_request(now, conn, stream, message),
            Inbound::Response { request, response } => {
                self.on_response(now, conn, request, response)
            }
            Inbound::RequestReset { stream } => {
                self.on_reset(now, conn, stream);
                Ok(())
            }
            Inbound::DataSent { stream } => {
                self.on_data_sent(now, conn, stream);
                Ok(())
            }
        };
        if let Err(fail) = outcome {
            self.fail(now, conn, fail);
        }
    }

    /// Reports that a connection ended without this session closing it.
    pub fn handle_closed(&mut self, now: Time, conn: ConnId, closed: Closed) {
        let Some(state) = self.remove_conn(now, conn) else {
            return;
        };
        let cause = match closed {
            Closed::Goodbye { reason, message } => CloseCause::Remote { reason, message },
            Closed::Lost => CloseCause::Lost,
        };
        let peer = state.peer();
        self.emit(now, EventKind::Closed { conn, peer, cause });
        self.after_removal(now);
    }

    /// Closes every connection with `shutting_down` and stops dialing or accepting.
    pub fn shutdown(&mut self, now: Time) {
        self.stopped = true;
        let conns: Vec<ConnId> = self.conns.keys().copied().collect();
        for conn in conns {
            self.close(now, conn, GoodbyeReason::ShuttingDown, String::new());
        }
    }

    // ----- Connections -----

    fn emit(&mut self, time: Time, kind: EventKind) {
        self.events.push(Event { time, kind });
    }

    fn send(&mut self, conn: ConnId, message: Message) {
        self.actions.push(Action::Send { conn, message });
    }

    fn send_opening(&mut self, conn: ConnId) {
        let hello = Hello {
            bundle_id: self.bundle_id,
            capabilities: Vec::new(),
            client: self.config.client.clone(),
            max_request_chunks: self.config.max_request_chunks,
            max_open_requests: self.config.max_open_requests,
            addresses: self.local.addresses.clone(),
            relay: self.local.relay,
        };
        let ranges = self
            .bundle
            .as_ref()
            .map(|b| b.have.to_ranges())
            .unwrap_or_default();
        self.send(conn, Message::Hello(hello));
        self.send(conn, Message::Have(Have { ranges }));
        self.conns.get_mut(&conn).expect("open").sent_opening = true;
    }

    /// Closes a connection because of `fail`, banning the peer for bad data (D-067).
    fn fail(&mut self, now: Time, conn: ConnId, fail: Fail) {
        if fail.reason == GoodbyeReason::BadData
            && let Some(state) = self.conns.get(&conn)
        {
            let peer = state.peer();
            if self.banned.insert(peer) {
                self.known.remove(&peer);
                self.emit(now, EventKind::Banned { peer });
            }
        }
        self.close(now, conn, fail.reason, fail.message);
    }

    fn close(&mut self, now: Time, conn: ConnId, reason: GoodbyeReason, message: String) {
        let Some(state) = self.remove_conn(now, conn) else {
            return;
        };
        self.actions.push(Action::Close {
            conn,
            reason,
            message: message.clone(),
        });
        let peer = state.peer();
        let cause = CloseCause::Local { reason, message };
        self.emit(now, EventKind::Closed { conn, peer, cause });
        self.after_removal(now);
    }

    /// Forgets a connection, releasing everything it held.
    fn remove_conn(&mut self, now: Time, conn: ConnId) -> Option<Conn> {
        let state = self.conns.remove(&conn)?;
        if let Some(bundle) = &mut self.bundle {
            for request in state.out.values() {
                if let OutRequest::Chunks { start, count } = *request {
                    bundle.release(start, count);
                }
            }
            if let Holdings::Known(set) = &state.holdings {
                for chunk in set.iter() {
                    bundle.availability[chunk as usize] -= 1;
                }
            }
        }
        if self.manifest_request.is_some_and(|(c, _)| c == conn) {
            self.manifest_request = None;
        }
        self.uploading -= state.incoming.values().filter(|i| i.sending).count() as u32;
        if let Some(known) = self.known.get_mut(&state.peer()) {
            known.next_attempt = now + self.config.redial_delay;
        }
        Some(state)
    }

    /// Reacts to a connection going away: its requests and upload slots are free for others.
    fn after_removal(&mut self, now: Time) {
        self.request_manifest(now);
        self.review_serving(now);
        self.pump_uploads(now);
        self.fill_all(now);
        self.maybe_dial(now);
    }

    fn may_dial(&self) -> bool {
        !self.stopped
            && !self.complete
            && self.conns.len() + self.dialing.len() < self.config.max_connections as usize
    }

    /// Known peers that could be dialed, ignoring when.
    fn dial_candidates(&self) -> impl Iterator<Item = (&PeerId, &KnownPeer)> {
        let connected: BTreeSet<PeerId> = self.conns.values().map(Conn::peer).collect();
        self.known.iter().filter(move |(peer, _)| {
            !connected.contains(peer) && !self.dialing.contains(peer) && !self.banned.contains(peer)
        })
    }

    /// Dials random known peers until the connection limit. Complete peers wait to be dialed.
    fn maybe_dial(&mut self, now: Time) {
        if !self.may_dial() {
            return;
        }
        let room = self.config.max_connections as usize - self.conns.len() - self.dialing.len();
        let eligible: Vec<PeerId> = self
            .dial_candidates()
            .filter(|(_, k)| k.next_attempt <= now)
            .map(|(&peer, _)| peer)
            .collect();
        for peer in eligible.into_iter().choose_multiple(&mut self.rng, room) {
            self.dialing.insert(peer);
            self.actions
                .push(Action::Dial(self.known[&peer].record.clone()));
        }
    }

    /// Keeps one of several open connections with the same peer. Only the peer with the lower
    /// ID decides (`protocol/connections.md`, Duplicate connections).
    fn resolve_duplicates(&mut self, now: Time, peer: PeerId) {
        if self.local.peer_id >= peer {
            return;
        }
        let mut open: Vec<_> = self
            .conns
            .iter()
            .filter(|(_, c)| c.peer() == peer)
            .filter_map(|(&id, c)| Some((c.info.path, c.opened_at?, id)))
            .collect();
        open.sort();
        for &(_, _, id) in open.iter().skip(1) {
            let message = "keeping another connection".to_owned();
            self.close(now, id, GoodbyeReason::Duplicate, message);
        }
    }

    // ----- Control stream -----

    fn on_control(&mut self, now: Time, id: ConnId, message: Message) -> Outcome {
        let conn = self.conns.get_mut(&id).expect("open");
        if conn.hello.is_none() {
            let Message::Hello(hello) = message else {
                return Err(violation("the first control message was not Hello"));
            };
            return self.on_hello(id, hello);
        }
        if !conn.is_open() {
            let Message::Have(have) = message else {
                return Err(violation("the second control message was not Have"));
            };
            return self.on_have(now, id, have);
        }
        match message {
            Message::HaveUpdate(update) => self.on_have_update(now, id, update),
            Message::Want(want) => {
                conn.they_want = want.wanted;
                self.review_serving(now);
                Ok(())
            }
            Message::Serving(serving) => {
                conn.serving_us = serving.serving;
                self.fill(now, id);
                Ok(())
            }
            Message::Peers(peers) => {
                self.add_peers(now, peers.peers);
                Ok(())
            }
            Message::Candidates(_) => {
                // Hole punching is the driver's job; the session only owes a reply.
                if !conn.sent_candidates {
                    conn.sent_candidates = true;
                    let mut addresses = self.local.addresses.clone();
                    addresses.truncate(MAX_CANDIDATES);
                    self.send(id, Message::Candidates(Candidates { addresses }));
                }
                Ok(())
            }
            Message::Hello(_) | Message::Have(_) => {
                Err(violation("Hello or Have sent after the opening"))
            }
            other => Err(violation(format!(
                "{:?} sent on the control stream",
                other.message_type()
            ))),
        }
    }

    fn on_hello(&mut self, id: ConnId, hello: Hello) -> Outcome {
        let conn = self.conns.get_mut(&id).expect("open");
        if hello.bundle_id != self.bundle_id {
            return Err(match conn.info.direction {
                Direction::Accepted => Fail {
                    reason: GoodbyeReason::UnknownBundle,
                    message: String::new(),
                },
                Direction::Dialed => violation("Hello named a different bundle"),
            });
        }
        conn.hello = Some(hello);
        if !conn.sent_opening {
            self.send_opening(id);
        }
        Ok(())
    }

    fn on_have(&mut self, now: Time, id: ConnId, have: Have) -> Outcome {
        self.on_holdings(id, have_as_update(have.ranges))?;
        let conn = self.conns.get_mut(&id).expect("open");
        conn.opened_at = Some(now);
        let peer = conn.peer();
        self.emit(now, EventKind::Opened { conn: id, peer });
        self.resolve_duplicates(now, peer);
        if !self.conns.contains_key(&id) {
            return Ok(());
        }
        self.update_want(id);
        let held = std::mem::take(&mut self.conns.get_mut(&id).expect("open").held);
        for (stream, message) in held {
            self.on_request(now, id, stream, message)?;
        }
        self.request_manifest(now);
        self.review_serving(now);
        self.fill(now, id);
        Ok(())
    }

    fn on_have_update(&mut self, now: Time, id: ConnId, update: HaveUpdate) -> Outcome {
        self.on_holdings(id, update)?;
        self.update_want(id);
        if self.bundle.is_none() {
            self.request_manifest(now);
        }
        self.fill(now, id);
        Ok(())
    }

    /// Applies a Have or HaveUpdate, or buffers it until the manifest arrives.
    fn on_holdings(&mut self, id: ConnId, update: HaveUpdate) -> Outcome {
        let max = self.config.max_pending_ranges;
        let conn = self.conns.get_mut(&id).expect("open");
        if !update.added.is_empty() {
            // A peer that has chunks has the manifest.
            conn.lacks_manifest = false;
        }
        if self.bundle.is_none() {
            if !buffer_update(&mut conn.holdings, update, max) {
                return Err(Fail {
                    reason: GoodbyeReason::Done,
                    message: "too many Have ranges before the manifest".to_owned(),
                });
            }
            return Ok(());
        }
        self.apply_holdings(id, &update)
    }

    /// Applies a Have or HaveUpdate to a connection's known holdings, checking the rules of
    /// `protocol/messages.md` (Common structures, HaveUpdate).
    fn apply_holdings(&mut self, id: ConnId, update: &HaveUpdate) -> Outcome {
        let bundle = self.bundle.as_mut().expect("manifest known");
        let conn = self.conns.get_mut(&id).expect("open");
        let Holdings::Known(set) = &mut conn.holdings else {
            unreachable!("holdings are known once the manifest is");
        };
        let count = bundle.layout.chunk_count();
        if !update.added.fits(count) || !update.removed.fits(count) {
            return Err(violation("ranges reach past the bundle's last chunk"));
        }
        if update.added.chunks().any(|c| set.contains(c)) {
            return Err(violation("HaveUpdate added a chunk already held"));
        }
        if update.removed.chunks().any(|c| !set.contains(c)) {
            return Err(violation("HaveUpdate removed a chunk not held"));
        }
        for chunk in update.added.chunks() {
            set.insert(chunk);
            bundle.availability[chunk as usize] += 1;
            conn.interesting += u32::from(!bundle.have.contains(chunk));
        }
        for chunk in update.removed.chunks() {
            set.remove(chunk);
            bundle.availability[chunk as usize] -= 1;
            conn.interesting -= u32::from(!bundle.have.contains(chunk));
        }
        Ok(())
    }

    /// Sends Want when whether this peer wants the other peer's chunks changes.
    fn update_want(&mut self, id: ConnId) {
        let Some(conn) = self.conns.get_mut(&id) else {
            return;
        };
        if self.bundle.is_none() || !conn.is_open() {
            return;
        }
        let wanted = conn.interesting > 0;
        if wanted != conn.we_want {
            conn.we_want = wanted;
            let want = tempest_protocol::message::Want { wanted };
            self.send(id, Message::Want(want));
        }
    }
}
