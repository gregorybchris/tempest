//! The session: one peer's participation in one bundle's swarm.
//!
//! A [`Session`] is a sans-IO state machine. Its driver reports what happened with the `handle_*`
//! methods, each given the current time, then carries out the session's [`Action`]s from
//! [`Session::poll_actions`] and calls [`Session::handle_timer`] at [`Session::next_timer`].
//! Decision events (D-006) come out of [`Session::poll_events`].
//!
//! The session is split by role: this module handles connections and the control stream,
//! `download` sends requests and handles their responses, `upload` answers the other peers'
//! requests and decides whom to serve, `superseed` hides chunks while superseeding, and
//! `manage` keeps the connection count, peer exchange and lingering in check.

mod bundle;
mod conn;
mod download;
mod manage;
mod superseed;
mod upload;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use rand::SeedableRng;
use tempest_protocol::message::{
    Candidates, GoodbyeReason, Have, HaveUpdate, Hello, MAX_CANDIDATES, PeerRecord,
};
use tempest_protocol::tree::{VerifyError, verify_data};
use tempest_protocol::{Address, BundleId, Manifest, Message, PeerId, RangeList};

use self::bundle::Bundle;
use self::conn::{Conn, Holdings, buffer_update, have_as_update};
use self::superseed::Superseed;
use crate::Rng;
use crate::chunks::ChunkSet;
use crate::config::{Config, ConfigError, WhenComplete};
use crate::event::{CloseCause, Event, EventKind};
use crate::io::{
    Action, Closed, ConnId, ConnInfo, Direction, Inbound, OutRequest, PathKind, RequestId, StreamId,
};
use crate::select::ChunkSelector;
use crate::time::Time;
use crate::upload::{RoundRobin, UploadStrategy};

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
    /// Addresses at which others see this peer, listed first in Candidates.
    external: Vec<Address>,
    bundle_id: BundleId,
    /// Set once the manifest is known.
    bundle: Option<Bundle>,
    conns: BTreeMap<ConnId, Conn>,
    /// Peers this session may dial, with the earliest time to try each.
    known: BTreeMap<PeerId, KnownPeer>,
    dialing: BTreeSet<PeerId>,
    /// Peers the driver is dialing directly to replace a relayed connection.
    punching: BTreeSet<PeerId>,
    banned: BTreeSet<PeerId>,
    /// The earliest time a known peer may be dialed, as of the last dialing decision. It may be
    /// stale, but only too early, which costs one wasted timer.
    next_dial: Option<Time>,
    /// When a session at its connection limit next checks for an idle connection to drop.
    next_conn_review: Time,
    /// The one ManifestRequest in flight, if any.
    manifest_request: Option<(ConnId, RequestId)>,
    next_request: u64,
    /// The chunk a media player is at, for the streaming strategy.
    position: u32,
    /// Whether endgame has started, so that it is announced once.
    endgame: bool,
    /// Accepted chunk requests waiting for an upload slot, in arrival order. Entries whose
    /// request was reset or whose connection closed are skipped.
    upload_queue: Vec<(ConnId, StreamId)>,
    /// Data responses being sent.
    uploading: u32,
    complete: bool,
    /// Whether this peer had every chunk from the start, as the origin of a bundle does.
    started_complete: bool,
    /// Whether a peer that lacked chunks while connected has received all of them.
    delivered: bool,
    /// Since when no connected peer has wanted anything from this peer.
    idle_since: Option<Time>,
    stopped: bool,
    downloaded: u64,
    duplicate: u64,
    uploaded: u64,
    superseed: Option<Superseed>,
    selector: Box<dyn ChunkSelector>,
    uploader: Box<dyn UploadStrategy>,
    /// Whether the driver chose the upload strategy (see [`Session::set_upload_strategy`]).
    custom_uploader: bool,
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
            selector: config.strategies.chunk_selection.build(&config.selection),
            uploader: config.strategies.upload.build(&config.upload),
            custom_uploader: false,
            config,
            local,
            external: Vec::new(),
            bundle_id,
            bundle: None,
            conns: BTreeMap::new(),
            known: BTreeMap::new(),
            dialing: BTreeSet::new(),
            punching: BTreeSet::new(),
            banned: BTreeSet::new(),
            next_dial: None,
            next_conn_review: Time::ZERO,
            manifest_request: None,
            next_request: 0,
            position: 0,
            endgame: false,
            upload_queue: Vec::new(),
            uploading: 0,
            complete: false,
            started_complete: false,
            delivered: false,
            idle_since: None,
            stopped: false,
            downloaded: 0,
            duplicate: 0,
            uploaded: 0,
            superseed: None,
            rng: Rng::seed_from_u64(seed),
            actions: Vec::new(),
            events: Vec::new(),
        })
    }

    /// A session that has the manifest and the chunks in `have` (and every empty file's chunk).
    ///
    /// If that is every chunk and superseeding is configured, the session superseeds.
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
        session.started_complete = session.complete;
        if session.complete && session.config.strategies.superseed {
            session.superseed = Some(Superseed::new(&bundle));
        }
        session.bundle = Some(bundle);
        session.uploader = session.upload_strategy();
        Ok(session)
    }

    /// Replaces the upload strategy used while downloading with one the settings can't name,
    /// such as the simulator's strategic client. Call it before the session opens connections.
    ///
    /// Once complete, the session still switches as `strategies.when_complete` says; with
    /// `unchanged`, it keeps this strategy.
    pub fn set_upload_strategy(&mut self, strategy: Box<dyn UploadStrategy>) {
        self.uploader = strategy;
        self.custom_uploader = true;
    }

    // ----- Accessors -----

    /// The settings.
    pub fn config(&self) -> &Config {
        &self.config
    }

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

    /// Whether this peer has stopped sharing: it was shut down or finished lingering.
    pub fn is_stopped(&self) -> bool {
        self.stopped
    }

    /// Whether this peer is superseeding.
    pub fn is_superseeding(&self) -> bool {
        self.superseed.is_some()
    }

    /// Payload bytes of verified Data received, including duplicates.
    pub fn downloaded(&self) -> u64 {
        self.downloaded
    }

    /// Payload bytes of verified Data received for chunks this peer already had, from
    /// duplicate endgame requests.
    pub fn duplicate(&self) -> u64 {
        self.duplicate
    }

    /// Payload bytes of Data sent.
    pub fn uploaded(&self) -> u64 {
        self.uploaded
    }

    /// The number of connections, open or opening.
    pub fn connection_count(&self) -> usize {
        self.conns.len()
    }

    /// The number of peers this session may dial.
    pub fn known_peer_count(&self) -> usize {
        self.known.len()
    }

    /// The request this peer sent as `request` on `conn`, if it is still open.
    pub fn request(&self, conn: ConnId, request: RequestId) -> Option<OutRequest> {
        Some(self.conns.get(&conn)?.out.get(&request)?.request)
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
        if self.stopped {
            return None;
        }
        let per_conn = self.conns.values().flat_map(|c| {
            let snub = c
                .chunk_requests()
                .next()
                .map(|_| c.progress_at + self.config.requests.snub_timeout);
            [c.retry_at, snub, c.next_peers]
        });
        let review = self.uploader.next_review();
        let dial = self.next_dial.filter(|_| self.may_dial());
        let conn_review =
            (self.at_connection_limit() && !self.complete).then_some(self.next_conn_review);
        let linger = self.linger_deadline();
        per_conn
            .flatten()
            .chain([review, dial, conn_review, linger].into_iter().flatten())
            .min()
    }

    /// Reports that time has reached `now`.
    pub fn handle_timer(&mut self, now: Time) {
        if self.stopped {
            return;
        }
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
        self.check_snubs(now);
        self.request_manifest(now);
        if self.uploader.next_review().is_some_and(|t| t <= now) {
            self.review_serving(now);
        }
        self.share_peers(now);
        self.review_connections(now);
        self.maybe_dial(now);
        self.check_linger(now);
    }

    /// Adds peers this session may dial, such as from a tracker, a link or peer exchange.
    ///
    /// Once `connections.max_known_peers` are known, each new peer replaces a random known one
    /// that isn't connected or being dialed.
    pub fn add_peers(&mut self, now: Time, peers: impl IntoIterator<Item = PeerRecord>) {
        for record in peers {
            if record.peer_id == self.local.peer_id || self.banned.contains(&record.peer_id) {
                continue;
            }
            if let Some(known) = self.known.get_mut(&record.peer_id) {
                known.record = record;
                continue;
            }
            if self.known.len() >= self.config.connections.max_known_peers as usize
                && !self.forget_a_known_peer()
            {
                continue;
            }
            self.known.insert(
                record.peer_id,
                KnownPeer {
                    record,
                    next_attempt: now,
                },
            );
        }
        self.maybe_dial(now);
    }

    /// Sets the relay address this peer advertises from now on, as its home relay lends one or
    /// it is lost.
    pub fn set_relay(&mut self, relay: Option<Address>) {
        self.local.relay = relay;
    }

    /// Sets the addresses at which others see this peer, such as its address as its home relay
    /// observes it. Candidates lists them before [`LocalPeer::addresses`].
    pub fn set_external_addresses(&mut self, addresses: Vec<Address>) {
        self.external = addresses;
    }

    /// Reports that a [`Action::Dial`] failed.
    pub fn handle_dial_failed(&mut self, now: Time, peer: PeerId) {
        self.dialing.remove(&peer);
        if let Some(known) = self.known.get_mut(&peer) {
            known.next_attempt = now + self.config.connections.redial_delay;
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
        let mut made_room = false;
        let refusal = if self.stopped {
            Some((GoodbyeReason::ShuttingDown, ""))
        } else if self.banned.contains(&info.peer) {
            Some((GoodbyeReason::BadData, "banned for sending bad data"))
        } else if info.peer == self.local.peer_id {
            Some((GoodbyeReason::Done, "connected to itself"))
        } else if self.at_connection_limit() && !self.is_upgrade(info) {
            made_room = self.make_room(now);
            (!made_room).then_some((GoodbyeReason::TooManyConnections, ""))
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

        let mut state = Conn::new(info, &self.config, now);
        if let Some(bundle) = &self.bundle {
            state.holdings = Holdings::Known(ChunkSet::new(bundle.layout.chunk_count()));
        }
        state.offer = self.new_offer();
        self.conns.insert(conn, state);
        if info.direction == Direction::Dialed {
            self.send_opening(conn);
        }
        if made_room {
            self.after_removal(now);
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

    /// Reports a protocol violation only the driver can see, such as a frame that doesn't decode
    /// or a request stream that ends without a response. The session closes the connection with
    /// `protocol_violation`.
    pub fn handle_violation(&mut self, now: Time, conn: ConnId, message: impl Into<String>) {
        if self.conns.contains_key(&conn) {
            self.fail(now, conn, violation(message));
        }
    }

    /// Reports the transport's round-trip time for a connection, which replaces the session's
    /// own estimate from request timing.
    pub fn handle_rtt(&mut self, now: Time, conn: ConnId, rtt: Duration) {
        if let Some(state) = self.conns.get_mut(&conn) {
            state.rtt.report(rtt);
            self.fill(now, conn);
        }
    }

    /// Moves the playback position, which the streaming strategy requests from.
    pub fn set_playback_position(&mut self, now: Time, chunk: u32) {
        self.position = chunk;
        self.fill_all(now);
    }

    /// Closes every connection with `shutting_down` and stops dialing or accepting.
    pub fn shutdown(&mut self, now: Time) {
        self.stop(now, GoodbyeReason::ShuttingDown, "");
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
            max_request_chunks: self.config.upload.max_request_chunks,
            max_open_requests: self.config.upload.max_open_requests,
            addresses: self.local.addresses.clone(),
            relay: self.local.relay,
        };
        let state = self.conns.get_mut(&conn).expect("open");
        let ranges = match (&state.offer, &self.bundle) {
            (Some(offer), _) => offer.announced.to_ranges(),
            (None, Some(bundle)) => bundle.have.to_ranges(),
            (None, None) => RangeList::new(),
        };
        state.sent_opening = true;
        self.send(conn, Message::Hello(hello));
        self.send(conn, Message::Have(Have { ranges }));
    }

    /// Stops sharing: closes every connection and stops dialing or accepting.
    fn stop(&mut self, now: Time, reason: GoodbyeReason, message: &str) {
        self.stopped = true;
        let conns: Vec<ConnId> = self.conns.keys().copied().collect();
        for conn in conns {
            self.close(now, conn, reason, message.to_owned());
        }
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
        if self.close_quietly(now, conn, reason, message) {
            self.after_removal(now);
        }
    }

    /// Closes a connection without reacting to its removal. Returns whether it was open.
    fn close_quietly(
        &mut self,
        now: Time,
        conn: ConnId,
        reason: GoodbyeReason,
        message: String,
    ) -> bool {
        let Some(state) = self.remove_conn(now, conn) else {
            return false;
        };
        self.actions.push(Action::Close {
            conn,
            reason,
            message: message.clone(),
        });
        let peer = state.peer();
        let cause = CloseCause::Local { reason, message };
        self.emit(now, EventKind::Closed { conn, peer, cause });
        true
    }

    /// Forgets a connection, releasing everything it held.
    fn remove_conn(&mut self, now: Time, conn: ConnId) -> Option<Conn> {
        let state = self.conns.remove(&conn)?;
        if let Some(bundle) = &mut self.bundle {
            for (_, start, count) in state.chunk_requests() {
                bundle.release(start, count);
            }
            if let Holdings::Known(set) = &state.holdings {
                for chunk in set.iter() {
                    bundle.availability.decrement(chunk);
                }
            }
        }
        if self.manifest_request.is_some_and(|(c, _)| c == conn) {
            self.manifest_request = None;
        }
        self.uploading -= state.incoming.values().filter(|i| i.sending).count() as u32;
        let peer = state.peer();
        if let Some(known) = self.known.get_mut(&peer) {
            known.next_attempt = now + self.config.connections.redial_delay;
        }
        if !self.conns.values().any(|c| c.peer() == peer) {
            // A hole punch that outlives every connection with the peer just runs out.
            self.punching.remove(&peer);
        }
        Some(state)
    }

    /// Reacts to a connection going away: its requests and upload slots are free for others.
    fn after_removal(&mut self, now: Time) {
        if self.stopped {
            return;
        }
        self.request_manifest(now);
        self.review_serving(now);
        self.pump_uploads(now);
        self.fill_all(now);
        self.maybe_dial(now);
        self.update_idle(now);
        self.check_linger(now);
    }

    /// Whether a new connection is the first direct one with a peer this peer is connected to
    /// only through relays, as a hole punch makes. It is let in over the connection limit,
    /// since it replaces a connection.
    fn is_upgrade(&self, info: ConnInfo) -> bool {
        let mut paths = (self.conns.values())
            .filter(|c| c.peer() == info.peer)
            .map(|c| c.info.path)
            .peekable();
        info.path == PathKind::Direct
            && paths.peek().is_some()
            && paths.all(|path| path == PathKind::Relayed)
    }

    fn at_connection_limit(&self) -> bool {
        self.conns.len() + self.dialing.len() >= self.config.connections.max as usize
    }

    /// Connections this peer is dialing or has dialed and not yet opened.
    fn dials_pending(&self) -> usize {
        let unopened = self
            .conns
            .values()
            .filter(|c| c.info.direction == Direction::Dialed && !c.is_open())
            .count();
        self.dialing.len() + unopened
    }

    fn may_dial(&self) -> bool {
        !self.stopped
            && !self.complete
            && !self.at_connection_limit()
            && self.dials_pending() < self.config.connections.max_dialing as usize
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
        use rand::seq::IteratorRandom;

        if !self.may_dial() {
            return;
        }
        let room = (self.config.connections.max as usize - self.conns.len() - self.dialing.len())
            .min(self.config.connections.max_dialing as usize - self.dials_pending());
        let (eligible, later): (Vec<_>, Vec<_>) = self
            .dial_candidates()
            .map(|(&peer, known)| (peer, known.next_attempt))
            .partition(|&(_, at)| at <= now);
        let chosen = eligible.iter().map(|&(peer, _)| peer);
        let chosen = chosen.choose_multiple(&mut self.rng, room);
        let leftover = eligible.len() > chosen.len();
        for peer in chosen {
            self.dialing.insert(peer);
            self.actions
                .push(Action::Dial(self.known[&peer].record.clone()));
        }
        // With eligible peers left over, a limit is reached and dialing waits for room.
        self.next_dial = if leftover {
            Some(now)
        } else {
            later.iter().map(|&(_, at)| at).min()
        };
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

    /// The upload strategy for this peer's state: superseeding, complete or downloading.
    fn upload_strategy(&self) -> Box<dyn UploadStrategy> {
        if self.superseed.is_some() {
            Box::new(RoundRobin::superseeding())
        } else if self.complete && self.config.strategies.when_complete == WhenComplete::RoundRobin
        {
            Box::new(RoundRobin::new())
        } else {
            self.config.strategies.upload.build(&self.config.upload)
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
                self.update_idle(now);
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
            Message::Candidates(candidates) => {
                self.on_candidates(id, candidates.addresses);
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
        if self.config.connections.peer_exchange {
            conn.next_peers = Some(now);
        }
        let peer = conn.peer();
        self.emit(now, EventKind::Opened { conn: id, peer });
        self.resolve_duplicates(now, peer);
        if !self.conns.contains_key(&id) {
            return Ok(());
        }
        self.after_opening(id);
        self.update_want(id);
        let held = std::mem::take(&mut self.conns.get_mut(&id).expect("open").held);
        for (stream, message) in held {
            self.on_request(now, id, stream, message)?;
        }
        self.request_manifest(now);
        self.review_serving(now);
        self.fill(now, id);
        self.share_peers(now);
        self.after_holdings(now, id);
        self.maybe_dial(now);
        Ok(())
    }

    fn on_have_update(&mut self, now: Time, id: ConnId, update: HaveUpdate) -> Outcome {
        self.on_holdings(id, update)?;
        self.update_want(id);
        if self.bundle.is_none() {
            self.request_manifest(now);
        }
        self.fill(now, id);
        self.after_holdings(now, id);
        Ok(())
    }

    /// Applies a Have or HaveUpdate, or buffers it until the manifest arrives.
    fn on_holdings(&mut self, id: ConnId, update: HaveUpdate) -> Outcome {
        let max = self.config.connections.max_pending_ranges as usize;
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
            bundle.availability.increment(chunk);
            conn.interesting += u32::from(!bundle.have.contains(chunk));
        }
        for chunk in update.removed.chunks() {
            set.remove(chunk);
            bundle.availability.decrement(chunk);
            conn.interesting -= u32::from(!bundle.have.contains(chunk));
        }
        if !set.is_full() {
            conn.was_incomplete = true;
        }
        Ok(())
    }

    /// Reacts to what an open connection's peer has: superseeding offers, delivery of a direct
    /// send, and closing connections between two complete peers. May close the connection.
    fn after_holdings(&mut self, now: Time, id: ConnId) {
        let Some(conn) = self.conns.get(&id) else {
            return;
        };
        if !conn.is_open() || self.bundle.is_none() {
            return;
        }
        let peer_complete = conn.is_complete();
        if peer_complete && conn.was_incomplete {
            self.delivered = true;
        }
        if self.superseed.is_some() {
            if peer_complete {
                self.end_superseeding(now, crate::event::SuperseedEnd::AnotherCopy);
            } else {
                self.offer_next(now, id);
            }
        }
        if self.complete && peer_complete {
            let message = "both peers have every chunk".to_owned();
            self.close(now, id, GoodbyeReason::Done, message);
        }
        self.check_linger(now);
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

    // ----- Upgrading to a direct connection -----

    /// Takes the first step of upgrading a newly opened connection (`protocol/relays.md`,
    /// Upgrading to a direct connection): the registered peer of a relayed connection, the one
    /// whose relay address was dialed, sends Candidates. A direct connection ends any hole punch
    /// with its peer.
    fn after_opening(&mut self, id: ConnId) {
        let conn = &self.conns[&id];
        let (peer, info) = (conn.peer(), conn.info);
        match (info.path, info.direction) {
            (PathKind::Direct, _) => {
                if self.punching.remove(&peer) {
                    self.actions.push(Action::StopPunching { peer });
                }
            }
            (PathKind::Relayed, Direction::Accepted) if !self.has_direct(peer) => {
                self.send_candidates(id);
            }
            (PathKind::Relayed, _) => {}
        }
    }

    /// Replies to the other peer's first Candidates on a connection, and has the driver dial
    /// them if the connection is relayed: both peers have now sent and received Candidates.
    fn on_candidates(&mut self, id: ConnId, addresses: Vec<Address>) {
        let conn = self.conns.get_mut(&id).expect("open");
        if conn.received_candidates {
            return;
        }
        conn.received_candidates = true;
        if !conn.sent_candidates {
            self.send_candidates(id);
        }
        let conn = &self.conns[&id];
        let peer = conn.peer();
        if conn.info.path == PathKind::Relayed && !self.has_direct(peer) {
            self.punching.insert(peer);
            self.actions.push(Action::Punch {
                conn: id,
                peer,
                addresses,
            });
        }
    }

    fn send_candidates(&mut self, id: ConnId) {
        self.conns.get_mut(&id).expect("open").sent_candidates = true;
        let addresses = self.candidates();
        self.send(id, Message::Candidates(Candidates { addresses }));
    }

    /// The addresses at which this peer might be reached directly: the external ones first,
    /// then those it accepts connections at, without repeats.
    fn candidates(&self) -> Vec<Address> {
        let mut addresses: Vec<Address> = Vec::new();
        for &address in self.external.iter().chain(&self.local.addresses) {
            if !addresses.contains(&address) {
                addresses.push(address);
            }
        }
        addresses.truncate(MAX_CANDIDATES);
        addresses
    }

    /// Whether an open direct connection with `peer` exists.
    fn has_direct(&self, peer: PeerId) -> bool {
        self.conns
            .values()
            .any(|c| c.peer() == peer && c.info.path == PathKind::Direct && c.is_open())
    }
}
