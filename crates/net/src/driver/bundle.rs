//! The bundle task: one [`Session`] and its storage.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use quinn::{RecvStream, SendStream};
use ring::rand::{SecureRandom, SystemRandom};
use tempest_core::io::{Closed, ConnInfo, Direction, PathKind};
use tempest_core::{
    Action, ChunkCell, ChunkSet, ConnId, Event, Inbound, LocalPeer, Reply, RequestId, Response,
    Session, StreamId, Time,
};
use tempest_protocol::hash::manifest_hash;
use tempest_protocol::message::{
    GoodbyeReason, Hello, ManifestMessage, PeerRecord, Reject, RejectReason,
};
use tempest_protocol::wire::{Decode, Encode};
use tempest_protocol::{Address, BundleId, Manifest, Message, PeerId, RangeList};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::Instant;

use super::conn::{self, ConnTask, LEAVE_GRACE};
use super::request::{self, Answer};
use super::{Destination, DriverError, NodeConfig, PeerDetail, Status, Summary};
use crate::endpoint::{Connection, Endpoint};
use crate::limit::Throttle;
use crate::storage::{Storage, StorageError};

/// How a bundle task starts.
pub(super) enum Start {
    /// With the manifest, from storage holding `have`.
    Stored { storage: Storage, have: RangeList },
    /// From the bundle ID alone, downloading to `destination`.
    Fetch { destination: Destination },
}

/// What a bundle task hears from the tasks around it and from its handle.
pub(crate) enum Report {
    /// A dial succeeded and the control stream is open.
    Dialed {
        connection: Connection,
        send: SendStream,
        recv: RecvStream,
    },
    /// A dial failed at every address.
    DialFailed { peer: PeerId },
    /// The node routed an incoming connection here, after reading its Hello.
    Accepted {
        connection: Connection,
        send: SendStream,
        recv: RecvStream,
        hello: Hello,
    },
    /// A message on a connection's control stream.
    Control { conn: ConnId, message: Message },
    /// A request the other peer opened a stream for, and where to send the answer.
    Request {
        conn: ConnId,
        stream: StreamId,
        message: Message,
        answer: oneshot::Sender<Answer>,
    },
    /// The other peer reset one of its request streams, or the Data for it could not be read.
    RequestReset { conn: ConnId, stream: StreamId },
    /// The Data for a request stream has been sent.
    DataSent { conn: ConnId, stream: StreamId },
    /// The response to one of this peer's requests, with Data already verified and stored.
    Response {
        conn: ConnId,
        request: RequestId,
        response: Response,
    },
    /// The other peer broke the protocol in a way the session can't see.
    Violation { conn: ConnId, message: String },
    /// QUIC's round-trip time estimate.
    Rtt { conn: ConnId, rtt: Duration },
    /// The connection ended without this side closing it.
    Closed { conn: ConnId, closed: Closed },
    /// Storage failed while writing; the bundle can't go on.
    StorageFailed(StorageError),
    /// Storage for the fetched manifest is laid out, or failed to be.
    LaidOut(Result<Storage, StorageError>),
    /// More peers to dial.
    AddPeers(Vec<PeerRecord>),
    /// Asks for a handle to storage, once laid out.
    Storage(oneshot::Sender<Option<Storage>>),
    /// Asks for every connection's state.
    Peers(oneshot::Sender<Vec<PeerDetail>>),
    /// Asks for the chunk map in at most this many cells, once the manifest is known.
    ChunkMap(u32, oneshot::Sender<Option<Vec<ChunkCell>>>),
    /// Asks for the chunks stored, once the manifest is known.
    Have(oneshot::Sender<Option<RangeList>>),
}

/// Where a bundle task publishes what it does.
pub(super) struct Outputs {
    /// Decision events.
    pub events: broadcast::Sender<Event>,
    /// The bundle's progress.
    pub status: watch::Sender<Status>,
}

/// A connection the session knows about.
struct Conn {
    connection: Connection,
    path: PathKind,
    /// Messages for the control stream.
    control: mpsc::Sender<Message>,
    /// Resolves once the control stream is finished and acknowledged, after `control` is
    /// dropped.
    drained: oneshot::Receiver<()>,
}

/// The bundle task's state.
pub(super) struct BundleTask {
    session: Session,
    epoch: Instant,
    endpoint: Endpoint,
    config: Arc<NodeConfig>,
    throttle: Throttle,
    /// The bundle's storage: from the start when shared, once laid out when downloaded.
    /// Requests wait on it to store what they receive.
    storage: watch::Sender<Option<Storage>>,
    /// Where to download to, until the manifest arrives and storage is laid out there.
    destination: Option<Destination>,
    /// Laying out storage for the fetched manifest, off the task's loop.
    laying_out: Option<JoinHandle<()>>,
    inbox: mpsc::Sender<Report>,
    conns: HashMap<ConnId, Conn>,
    next_conn: u64,
    /// Requests the other peers opened, waiting for the session's answer.
    answers: HashMap<(ConnId, StreamId), oneshot::Sender<Answer>>,
    /// This peer's requests in flight; sending on one cancels it.
    cancels: HashMap<(ConnId, RequestId), oneshot::Sender<()>>,
    /// Hole punches under way, by peer; setting the flag stops one.
    punches: HashMap<PeerId, Arc<AtomicBool>>,
    events: broadcast::Sender<Event>,
    status: watch::Sender<Status>,
    /// Why the bundle stopped, if storage failed.
    failure: Option<StorageError>,
    /// Connections being closed once what is queued for them has been sent.
    leaving: JoinSet<()>,
}

impl BundleTask {
    pub(super) fn new(
        bundle_id: BundleId,
        start: Start,
        endpoint: Endpoint,
        config: Arc<NodeConfig>,
        throttle: Throttle,
        inbox: mpsc::Sender<Report>,
        outputs: Outputs,
    ) -> Result<Self, DriverError> {
        let Outputs { events, status } = outputs;
        let local = LocalPeer {
            peer_id: endpoint.peer_id(),
            addresses: endpoint.port_mapping().advertise(&config.addresses),
            relay: endpoint.relay_address(),
        };
        let settings = config.session.clone();
        let (session, storage, destination) = match start {
            Start::Stored { storage, have } => {
                let manifest = storage.manifest().clone();
                let session = Session::with_manifest(settings, local, manifest, &have, seed())?;
                (session, Some(storage), None)
            }
            Start::Fetch { destination } => {
                let session = Session::new(settings, local, bundle_id, seed())?;
                (session, None, Some(destination))
            }
        };
        Ok(Self {
            session,
            epoch: Instant::now(),
            endpoint,
            config,
            throttle,
            storage: watch::Sender::new(storage),
            destination,
            laying_out: None,
            inbox,
            conns: HashMap::new(),
            next_conn: 0,
            answers: HashMap::new(),
            cancels: HashMap::new(),
            punches: HashMap::new(),
            events,
            status,
            failure: None,
            leaving: JoinSet::new(),
        })
    }

    fn now(&self) -> Time {
        Time::from_duration(self.epoch.elapsed())
    }

    pub(super) async fn run(
        mut self,
        mut inbox: mpsc::Receiver<Report>,
        mut shutdown: watch::Receiver<bool>,
        peers: Vec<PeerRecord>,
    ) -> Result<Summary, DriverError> {
        let now = self.now();
        self.session.add_peers(now, peers);
        loop {
            self.step().await;
            if self.session.is_stopped() {
                break;
            }
            let timer = self.session.next_timer();
            let deadline = timer.map(|t| self.epoch + t.since_epoch());
            tokio::select! {
                report = inbox.recv() => {
                    let report = report.expect("the task holds a sender to its own inbox");
                    self.handle(report);
                }
                () = sleep_until(deadline), if deadline.is_some() => {
                    let now = self.now();
                    self.session.handle_timer(now);
                }
                // A dropped sender means the node is gone too.
                _ = shutdown.wait_for(|stop| *stop) => {
                    let now = self.now();
                    self.session.shutdown(now);
                }
            }
        }

        // Let the last messages, such as the farewell list, reach the peers before returning.
        while self.leaving.join_next().await.is_some() {}
        // Storage still being laid out is cancelled, unless it has just finished.
        if let Some(laying_out) = self.laying_out.take() {
            laying_out.abort();
            let _ = laying_out.await;
            while let Ok(report) = inbox.try_recv() {
                if let Report::LaidOut(Ok(storage)) = report {
                    self.storage.send_replace(Some(storage));
                }
            }
        }
        let closed = match self.storage.send_replace(None) {
            Some(storage) => storage.close().await.map(Some),
            None => Ok(None),
        };
        match (self.failure, closed) {
            (Some(error), _) | (None, Err(error)) => Err(error.into()),
            (None, Ok(checkpoint)) => Ok(Summary {
                complete: self.session.is_complete(),
                downloaded: self.session.downloaded(),
                uploaded: self.session.uploaded(),
                checkpoint,
            }),
        }
    }

    /// Carries out the session's actions, publishes its events and updates the status.
    async fn step(&mut self) {
        loop {
            let actions = self.session.poll_actions();
            if actions.is_empty() {
                break;
            }
            for action in actions {
                self.perform(action).await;
            }
        }
        for event in self.session.poll_events() {
            self.log(&event);
            // Nobody listening is fine.
            let _ = self.events.send(event);
        }
        self.publish_status();
    }

    /// Logs the session's notable events: connections and peers at debug, the bundle's
    /// completion and failures above that. Per-chunk events are left to subscribers.
    fn log(&self, event: &Event) {
        use tempest_core::EventKind as Kind;
        let bundle = self.session.bundle_id();
        match &event.kind {
            Kind::Connected {
                peer,
                direction,
                path,
                ..
            } => tracing::debug!("{bundle}: connected to {peer} ({direction:?}, {path:?})"),
            Kind::Opened { peer, .. } => tracing::debug!("{bundle}: {peer} said hello"),
            Kind::Closed { peer, cause, .. } => {
                tracing::debug!("{bundle}: connection to {peer} closed: {cause:?}");
            }
            Kind::DialFailed { peer } => tracing::debug!("{bundle}: dialing {peer} failed"),
            Kind::ManifestReceived { peer, .. } => {
                tracing::info!("{bundle}: manifest received from {peer}");
            }
            Kind::RequestRejected { peer, reason, .. } => {
                tracing::debug!("{bundle}: {peer} rejected a request: {reason:?}");
            }
            Kind::Banned { peer } => tracing::info!("{bundle}: banned {peer} for the session"),
            Kind::Completed => tracing::info!("{bundle}: complete"),
            Kind::Snubbed { .. } | Kind::ServingChanged { .. } => {
                tracing::debug!("{bundle}: {:?}", event.kind);
            }
            _ => tracing::trace!("{bundle}: {:?}", event.kind),
        }
    }

    pub(super) fn with_status_published(self) -> Self {
        self.publish_status();
        self
    }

    fn publish_status(&self) {
        let session = &self.session;
        let status = Status {
            manifest: session.manifest().cloned(),
            have: session.have().map_or(0, ChunkSet::len),
            chunks: session.have().map_or(0, ChunkSet::capacity),
            downloaded: session.downloaded(),
            uploaded: session.uploaded(),
            connections: session.connection_count(),
            relayed: (self.conns.values())
                .filter(|conn| conn.path == PathKind::Relayed)
                .count(),
            known_peers: session.known_peer_count(),
            complete: session.is_complete(),
        };
        self.status.send_if_modified(|current| {
            let changed = *current != status;
            *current = status;
            changed
        });
    }

    fn handle(&mut self, report: Report) {
        let now = self.now();
        let session = &mut self.session;
        match report {
            Report::Dialed {
                connection,
                send,
                recv,
            } => self.connected(connection, send, recv, None),
            Report::DialFailed { peer } => session.handle_dial_failed(now, peer),
            Report::Accepted {
                connection,
                send,
                recv,
                hello,
            } => self.connected(connection, send, recv, Some(hello)),
            Report::Control { conn, message } => {
                session.handle_message(now, conn, Inbound::Control(message));
            }
            Report::Request {
                conn,
                stream,
                message,
                answer,
            } => {
                // A request on a connection the session has closed goes unanswered.
                if self.conns.contains_key(&conn) {
                    self.answers.insert((conn, stream), answer);
                    session.handle_message(now, conn, Inbound::Request { stream, message });
                }
            }
            Report::RequestReset { conn, stream } => {
                self.answers.remove(&(conn, stream));
                session.handle_message(now, conn, Inbound::RequestReset { stream });
            }
            Report::DataSent { conn, stream } => {
                session.handle_message(now, conn, Inbound::DataSent { stream });
            }
            Report::Response {
                conn,
                request,
                response,
            } => {
                self.cancels.remove(&(conn, request));
                if let Some(error) = self.refusal(&response) {
                    self.failure.get_or_insert(error);
                    self.session.shutdown(now);
                    return;
                }
                let session = &mut self.session;
                session.handle_message(now, conn, Inbound::Response { request, response });
            }
            Report::Violation { conn, message } => session.handle_violation(now, conn, message),
            Report::Rtt { conn, rtt } => session.handle_rtt(now, conn, rtt),
            Report::Closed { conn, closed } => {
                self.forget(conn);
                self.session.handle_closed(now, conn, closed);
            }
            Report::StorageFailed(error) => {
                tracing::error!("{}: storage failed: {error}", session.bundle_id());
                self.failure.get_or_insert(error);
                session.shutdown(now);
            }
            Report::LaidOut(result) => {
                self.laying_out = None;
                match result {
                    // Chunks already on disk are verified again as they arrive, and not
                    // rewritten.
                    Ok(storage) => {
                        self.storage.send_replace(Some(storage));
                    }
                    Err(error) => {
                        tracing::error!("{}: cannot lay out storage: {error}", session.bundle_id());
                        self.failure.get_or_insert(error);
                        session.shutdown(now);
                    }
                }
            }
            Report::AddPeers(peers) => session.add_peers(now, peers),
            Report::Storage(reply) => {
                let _ = reply.send(self.storage.borrow().clone());
            }
            Report::Peers(reply) => {
                let peers = session.peers(now).into_iter().filter_map(|status| {
                    let conn = self.conns.get(&status.conn)?;
                    let address = conn.connection.remote_address();
                    Some(PeerDetail { address, status })
                });
                let _ = reply.send(peers.collect());
            }
            Report::ChunkMap(cells, reply) => {
                let _ = reply.send(session.chunk_map(cells));
            }
            Report::Have(reply) => {
                let _ = reply.send(session.have().map(ChunkSet::to_ranges));
            }
        }
    }

    /// Starts a connection task for a new connection and tells the session about it. An
    /// accepted connection's Hello has already been read.
    fn connected(
        &mut self,
        connection: Connection,
        send: SendStream,
        recv: RecvStream,
        hello: Option<Hello>,
    ) {
        let conn = ConnId(self.next_conn);
        self.next_conn += 1;
        let (control, outbox) = mpsc::channel(self.config.control_queue.max(1));
        let (drain, drained) = oneshot::channel();
        let task = ConnTask {
            conn,
            connection: connection.clone(),
            inbox: self.inbox.clone(),
            rtt_interval: self.config.rtt_interval,
            throttle: self.throttle.clone(),
            drained: Some(drain),
        };
        tokio::spawn(task.run(send, recv, outbox));
        self.conns.insert(
            conn,
            Conn {
                connection: connection.clone(),
                path: connection.path(),
                control,
                drained,
            },
        );

        let now = self.now();
        let info = ConnInfo {
            peer: connection.peer(),
            direction: connection.direction(),
            path: connection.path(),
        };
        // Hello and Candidates carry the port mapping's external address and the addresses the
        // home relay lends and observes now.
        let addresses = self
            .endpoint
            .port_mapping()
            .advertise(&self.config.addresses);
        self.session.set_addresses(addresses);
        self.session.set_relay(self.endpoint.relay_address());
        self.session
            .set_external_addresses(self.endpoint.external_addresses());
        self.session.handle_connected(now, conn, info);
        if let Some(hello) = hello {
            debug_assert_eq!(info.direction, Direction::Accepted);
            let inbound = Inbound::Control(Message::Hello(hello));
            self.session.handle_message(now, conn, inbound);
        }
    }

    /// Drops what the driver keeps for a connection. Its tasks end as the connection closes.
    fn forget(&mut self, conn: ConnId) {
        self.conns.remove(&conn);
        self.answers.retain(|&(c, _), _| c != conn);
        self.cancels.retain(|&(c, _), _| c != conn);
    }

    async fn perform(&mut self, action: Action) {
        match action {
            Action::Dial(record) => {
                let endpoint = self.endpoint.clone();
                let inbox = self.inbox.clone();
                let timeout = self.config.dial_timeout;
                tokio::spawn(conn::dial(endpoint, record, timeout, inbox));
            }
            Action::Send { conn, message } => {
                let Some(entry) = self.conns.get(&conn) else {
                    return;
                };
                // A closed outbox means the connection task has ended, and it reports why.
                if let Err(TrySendError::Full(_)) = entry.control.try_send(message) {
                    // The other peer isn't reading its control stream.
                    entry
                        .connection
                        .close(GoodbyeReason::Done, "control stream backed up");
                    self.forget(conn);
                    let now = self.now();
                    self.session.handle_closed(now, conn, Closed::Lost);
                }
            }
            Action::Request {
                conn,
                request,
                message,
            } => {
                let Some(entry) = self.conns.get(&conn) else {
                    return;
                };
                let (cancel, cancelled) = oneshot::channel();
                self.cancels.insert((conn, request), cancel);
                let task = request::Outgoing {
                    conn,
                    request,
                    message,
                    connection: entry.connection.clone(),
                    storage: self.storage.subscribe(),
                    throttle: self.throttle.clone(),
                    inbox: self.inbox.clone(),
                };
                tokio::spawn(task.run(cancelled));
            }
            Action::Cancel { conn, request } => {
                if let Some(cancel) = self.cancels.remove(&(conn, request)) {
                    let _ = cancel.send(());
                }
            }
            Action::Respond {
                conn,
                stream,
                reply,
            } => {
                let Some(answer) = self.answers.remove(&(conn, stream)) else {
                    return;
                };
                let _ = answer.send(self.answer(reply));
            }
            Action::Close {
                conn,
                reason,
                message,
            } => {
                if let Some(Conn {
                    connection,
                    control,
                    drained,
                    ..
                }) = self.conns.remove(&conn)
                {
                    self.close_after_sending(connection, control, drained, reason, message);
                }
                self.forget(conn);
            }
            Action::Punch {
                conn,
                peer,
                addresses,
            } => self.punch(conn, peer, addresses),
            Action::StopPunching { peer } => {
                if let Some(stop) = self.punches.remove(&peer) {
                    stop.store(true, Ordering::Relaxed);
                }
            }
            Action::StoreManifest { manifest, .. } => {
                let Some(destination) = self.destination.take() else {
                    return;
                };
                let config = self.config.storage.clone();
                let inbox = self.inbox.clone();
                // Laying out may create many files or hash existing ones: the loop goes on
                // meanwhile, and requests wait for storage to store what they receive.
                self.laying_out = Some(tokio::spawn(async move {
                    let created = match destination.resolve(&manifest).await {
                        Ok(dir) => Storage::create_download(dir, manifest, config).await,
                        Err(error) => Err(error),
                    };
                    let _ = inbox
                        .send(Report::LaidOut(created.map(|(storage, _)| storage)))
                        .await;
                }));
            }
        }
    }

    /// Closes a connection with a Goodbye once its control stream has sent what is queued and
    /// the other peer has acknowledged it, within a bound. Closing at once would drop the
    /// unsent data, such as the farewell Peers (D-224). A protocol violation closes at once.
    fn close_after_sending(
        &mut self,
        connection: Connection,
        control: mpsc::Sender<Message>,
        drained: oneshot::Receiver<()>,
        reason: GoodbyeReason,
        message: String,
    ) {
        // Dropping the sender ends the writer, which finishes the stream.
        drop(control);
        if reason == GoodbyeReason::ProtocolViolation {
            connection.close(reason, &message);
            return;
        }
        self.leaving.spawn(async move {
            let _ = tokio::time::timeout(2 * LEAVE_GRACE, drained).await;
            connection.close(reason, &message);
        });
    }

    /// Starts a hole punch with `peer`, connected through `conn`. The address the peer's packets
    /// reach this one's home relay from is dialed first, since it is usually the best
    /// (`protocol/relays.md`, Upgrading to a direct connection).
    fn punch(&mut self, conn: ConnId, peer: PeerId, addresses: Vec<Address>) {
        let Some(time) = self.config.punch_time else {
            return;
        };
        let sender = (self.conns.get(&conn)).and_then(|entry| {
            self.endpoint
                .relayed_sender(entry.connection.remote_address())
        });
        let mut targets: Vec<Address> = sender.into_iter().collect();
        for address in addresses {
            if !targets.contains(&address) {
                targets.push(address);
            }
        }
        // Forget punches that have ended, and stop any other with the same peer.
        self.punches.retain(|_, stop| Arc::strong_count(stop) > 1);
        let stop = Arc::new(AtomicBool::new(false));
        if let Some(old) = self.punches.insert(peer, stop.clone()) {
            old.store(true, Ordering::Relaxed);
        }
        conn::punch(&self.endpoint, peer, targets, time, &self.inbox, stop);
    }

    /// What to send for the session's reply.
    fn answer(&self, reply: Reply) -> Answer {
        let storage = self.storage.borrow();
        match (reply, &*storage) {
            (Reply::Data { start, count }, Some(storage)) => Answer::Data {
                bytes: self.session.data_len(start, count).unwrap_or(0),
                storage: storage.clone(),
                start,
                count,
            },
            (Reply::Manifest, Some(storage)) => Answer::Manifest(ManifestMessage {
                manifest: storage.manifest_bytes().to_vec(),
            }),
            // Storage is still being laid out.
            (Reply::Manifest, None) if self.session.manifest().is_some() => {
                let manifest = self.session.manifest().expect("checked").to_bytes();
                Answer::Manifest(ManifestMessage { manifest })
            }
            (Reply::Reject(reject), _) => Answer::Message(Message::Reject(reject)),
            // The session answers with Data only for chunks it has, which are stored, unless
            // laying storage out failed.
            (Reply::Data { .. } | Reply::Manifest, None) => {
                Answer::Message(Message::Reject(Reject {
                    reason: RejectReason::DontHave,
                    message: String::new(),
                }))
            }
        }
    }

    /// Why a response can't be taken, if it is a manifest this bundle's storage won't take. A
    /// manifest's size is checked here, before the session sets up its state for every chunk.
    fn refusal(&self, response: &Response) -> Option<StorageError> {
        let Response::Manifest(message) = response else {
            return None;
        };
        // Anything else wrong with it is the session's to judge.
        if self.session.manifest().is_some()
            || manifest_hash(&message.manifest) != self.session.bundle_id()
        {
            return None;
        }
        let size = Manifest::from_bytes(&message.manifest).ok()?.total_size();
        let max = self.config.storage.max_bundle_size;
        (size > max).then_some(StorageError::TooLarge { size, max })
    }
}

/// Sleeps until `deadline`, or forever without one.
async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// A seed for the session's random choices, from the operating system.
fn seed() -> u64 {
    let mut bytes = [0; 8];
    SystemRandom::new()
        .fill(&mut bytes)
        .expect("the system's random number generator works");
    u64::from_le_bytes(bytes)
}
