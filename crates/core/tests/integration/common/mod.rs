//! A small test driver: sessions joined by a simple timed network.
//!
//! Each node has a one-way latency, added at both ends of a link, and optionally an upload rate:
//! Data leaves a node one response at a time at that rate. Everything else is instant. With no
//! latency and no upload rate, messages are delivered one at a time in the order they were sent,
//! and time only moves to fire timers. Each peer stores real bytes in a [`MemoryContent`] and
//! verifies every Data it receives.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::time::Duration;

use tempest_core::content::MemoryContent;
use tempest_core::io::{Closed, ConnInfo, Direction, OutRequest, PathKind};
use tempest_core::{
    Action, Config, ConnId, Event, EventKind, Inbound, LocalPeer, Reply, RequestId, Response,
    Session, StreamId, Time,
};
use tempest_protocol::message::{Data, ManifestMessage, PeerRecord};
use tempest_protocol::{Hash, PeerId, RangeList};

pub const CHUNK: usize = 16 * 1024;

pub fn peer_id(n: u8) -> PeerId {
    Hash::new([n; 32])
}

pub fn record(peer: PeerId) -> PeerRecord {
    PeerRecord {
        peer_id: peer,
        addresses: Vec::new(),
        relay: None,
    }
}

/// Deterministic file contents.
pub fn file_bytes(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

/// A bundle with one file of each size in `sizes`.
pub fn bundle(sizes: &[usize]) -> MemoryContent {
    let files = sizes
        .iter()
        .enumerate()
        .map(|(i, &size)| (format!("file-{i}"), file_bytes(i as u64, size)))
        .collect();
    MemoryContent::new("bundle", files)
}

pub fn at(seconds: f64) -> Time {
    Time::ZERO + Duration::from_secs_f64(seconds)
}

pub fn far_future() -> Time {
    at(3600.0)
}

pub struct Node {
    pub id: PeerId,
    pub session: Session,
    pub content: MemoryContent,
    pub events: Vec<Event>,
    /// Flip a byte of every Data this peer sends.
    pub corrupt: bool,
    /// Never send the Data this peer's session asks to send.
    pub mute: bool,
    /// Added to every message to or from this node.
    pub latency: Duration,
    /// Bytes per second this node sends Data at, or unlimited.
    pub upload_rate: Option<u64>,
    /// When this node's uplink finishes the Data it is sending.
    uplink_free: Time,
}

impl Node {
    /// When this node completed, if it did.
    pub fn completed_at(&self) -> Option<Time> {
        self.events
            .iter()
            .find(|e| e.kind == EventKind::Completed)
            .map(|e| e.time)
    }

    /// The events matching `pick`.
    pub fn events_where(&self, pick: impl Fn(&EventKind) -> bool) -> Vec<&Event> {
        self.events.iter().filter(|e| pick(&e.kind)).collect()
    }
}

enum Delivery {
    Connected(usize, ConnId, ConnInfo),
    Inbound(usize, ConnId, Inbound),
    Data {
        to: usize,
        conn: ConnId,
        request: RequestId,
        data: Data,
    },
    Closed(usize, ConnId, Closed),
    DialFailed(usize, PeerId),
}

pub struct Swarm {
    pub now: Time,
    pub nodes: Vec<Node>,
    /// Each connection end, by its node and ID, mapped to the other end.
    links: BTreeMap<(usize, ConnId), (usize, ConnId)>,
    /// Deliveries by time, then by the order they were scheduled.
    queue: BTreeMap<(Time, u64), Delivery>,
    scheduled: u64,
    next_conn: u64,
}

impl Swarm {
    pub fn new() -> Self {
        Self {
            now: Time::ZERO,
            nodes: Vec::new(),
            links: BTreeMap::new(),
            queue: BTreeMap::new(),
            scheduled: 0,
            next_conn: 0,
        }
    }

    /// Adds a peer with every chunk of `content`.
    pub fn add_complete(&mut self, n: u8, config: Config, content: &MemoryContent) -> usize {
        let id = peer_id(n);
        let have = RangeList::from_chunks(
            0..tempest_protocol::ChunkLayout::new(content.manifest()).chunk_count(),
        );
        let session = Session::with_manifest(
            config,
            local(id),
            content.manifest().clone(),
            &have,
            u64::from(n),
        )
        .unwrap();
        self.push(id, session, content.clone())
    }

    /// Adds a peer that knows only the bundle ID.
    pub fn add_empty(&mut self, n: u8, config: Config, content: &MemoryContent) -> usize {
        let id = peer_id(n);
        let bundle_id = content.manifest().bundle_id();
        let session = Session::new(config, local(id), bundle_id, u64::from(n)).unwrap();
        self.push(id, session, content.empty_copy())
    }

    fn push(&mut self, id: PeerId, session: Session, content: MemoryContent) -> usize {
        self.nodes.push(Node {
            id,
            session,
            content,
            events: Vec::new(),
            corrupt: false,
            mute: false,
            latency: Duration::ZERO,
            upload_rate: None,
            uplink_free: Time::ZERO,
        });
        self.nodes.len() - 1
    }

    /// Tells `node` about the peers `others`.
    pub fn introduce(&mut self, node: usize, others: &[usize]) {
        let records: Vec<_> = others.iter().map(|&o| record(self.nodes[o].id)).collect();
        self.nodes[node].session.add_peers(self.now, records);
    }

    /// Tells every node in `group` about every other.
    pub fn introduce_all(&mut self, group: &[usize]) {
        for &node in group {
            let others: Vec<usize> = group.iter().copied().filter(|&o| o != node).collect();
            self.introduce(node, &others);
        }
    }

    /// Runs until nothing is in flight and no timer is due before `until`.
    pub fn run(&mut self, until: Time) {
        let mut steps = 0u64;
        loop {
            steps += 1;
            assert!(steps < 10_000_000, "the swarm did not settle");
            if self.perform_actions() {
                continue;
            }
            let delivery = self.queue.first_key_value().map(|(&(t, _), _)| t);
            let timer = self
                .nodes
                .iter()
                .filter_map(|n| n.session.next_timer())
                .min();
            let next = match (delivery, timer) {
                (Some(d), Some(t)) => d.min(t),
                (Some(d), None) => d,
                (None, Some(t)) => t,
                (None, None) => break,
            };
            if next > until {
                break;
            }
            self.now = self.now.max(next);
            if delivery == Some(next) {
                let (_, delivery) = self.queue.pop_first().expect("present");
                self.deliver(delivery);
                continue;
            }
            for node in &mut self.nodes {
                if node.session.next_timer().is_some_and(|t| t <= self.now) {
                    node.session.handle_timer(self.now);
                }
            }
        }
        for node in &mut self.nodes {
            node.events.extend(node.session.poll_events());
        }
    }

    fn schedule(&mut self, at: Time, delivery: Delivery) {
        self.queue.insert((at, self.scheduled), delivery);
        self.scheduled += 1;
    }

    fn perform_actions(&mut self) -> bool {
        let mut any = false;
        for i in 0..self.nodes.len() {
            let actions = self.nodes[i].session.poll_actions();
            let events = self.nodes[i].session.poll_events();
            self.nodes[i].events.extend(events);
            for action in actions {
                any = true;
                self.perform(i, action);
            }
        }
        any
    }

    fn other_end(&self, node: usize, conn: ConnId) -> Option<(usize, ConnId)> {
        self.links.get(&(node, conn)).copied()
    }

    fn latency(&self, a: usize, b: usize) -> Duration {
        self.nodes[a].latency + self.nodes[b].latency
    }

    fn perform(&mut self, i: usize, action: Action) {
        let now = self.now;
        match action {
            Action::Dial(record) => {
                let Some(j) = self.nodes.iter().position(|n| n.id == record.peer_id) else {
                    self.schedule(now, Delivery::DialFailed(i, record.peer_id));
                    return;
                };
                let (a, b) = (ConnId(self.next_conn), ConnId(self.next_conn + 1));
                self.next_conn += 2;
                self.links.insert((i, a), (j, b));
                self.links.insert((j, b), (i, a));
                let info = |peer, direction| ConnInfo {
                    peer,
                    direction,
                    path: PathKind::Direct,
                };
                let (pi, pj) = (self.nodes[i].id, self.nodes[j].id);
                let arrive = now + self.latency(i, j);
                self.schedule(
                    arrive,
                    Delivery::Connected(i, a, info(pj, Direction::Dialed)),
                );
                self.schedule(
                    arrive,
                    Delivery::Connected(j, b, info(pi, Direction::Accepted)),
                );
            }
            Action::Send { conn, message } => {
                if let Some((j, c)) = self.other_end(i, conn) {
                    let arrive = now + self.latency(i, j);
                    self.schedule(arrive, Delivery::Inbound(j, c, Inbound::Control(message)));
                }
            }
            Action::Request {
                conn,
                request,
                message,
            } => {
                if let Some((j, c)) = self.other_end(i, conn) {
                    let inbound = Inbound::Request {
                        stream: StreamId(request.0),
                        message: message.message(),
                    };
                    let arrive = now + self.latency(i, j);
                    self.schedule(arrive, Delivery::Inbound(j, c, inbound));
                }
            }
            Action::Cancel { conn, request } => {
                if let Some((j, c)) = self.other_end(i, conn) {
                    let inbound = Inbound::RequestReset {
                        stream: StreamId(request.0),
                    };
                    let arrive = now + self.latency(i, j);
                    self.schedule(arrive, Delivery::Inbound(j, c, inbound));
                }
            }
            Action::Respond {
                conn,
                stream,
                reply,
            } => {
                let Some((j, c)) = self.other_end(i, conn) else {
                    return;
                };
                let request = RequestId(stream.0);
                let arrive = now + self.latency(i, j);
                let response = match reply {
                    Reply::Data { .. } if self.nodes[i].mute => return,
                    Reply::Data { start, count } => {
                        let mut data = self.nodes[i].content.data(start, count);
                        if self.nodes[i].corrupt
                            && let Some(byte) = data.chunks.first_mut()
                        {
                            *byte ^= 1;
                        }
                        let node = &mut self.nodes[i];
                        let sent = match node.upload_rate {
                            Some(rate) => {
                                let begin = node.uplink_free.max(now);
                                let secs = data.chunks.len() as f64 / rate as f64;
                                begin + Duration::from_secs_f64(secs)
                            }
                            None => now,
                        };
                        node.uplink_free = sent;
                        // The sender learns it finished sending no later than the data
                        // arrives.
                        self.schedule(
                            sent,
                            Delivery::Inbound(i, conn, Inbound::DataSent { stream }),
                        );
                        let arrive = sent + self.latency(i, j);
                        self.schedule(
                            arrive,
                            Delivery::Data {
                                to: j,
                                conn: c,
                                request,
                                data,
                            },
                        );
                        return;
                    }
                    Reply::Manifest => {
                        let sent = Inbound::DataSent { stream };
                        self.schedule(now, Delivery::Inbound(i, conn, sent));
                        Response::Manifest(ManifestMessage {
                            manifest: self.nodes[i].content.manifest_bytes(),
                        })
                    }
                    Reply::Reject(reject) => Response::Reject(reject),
                };
                let inbound = Inbound::Response { request, response };
                self.schedule(arrive, Delivery::Inbound(j, c, inbound));
            }
            Action::Close {
                conn,
                reason,
                message,
            } => {
                // The closing end stops hearing from the other at once. The other end keeps
                // its link until the Goodbye arrives, so messages sent before it still land.
                if let Some((j, c)) = self.links.remove(&(i, conn)) {
                    let closed = Closed::Goodbye { reason, message };
                    let arrive = now + self.latency(i, j);
                    self.schedule(arrive, Delivery::Closed(j, c, closed));
                }
            }
            Action::StoreManifest { manifest, .. } => {
                assert_eq!(&manifest, self.nodes[i].content.manifest());
            }
            // Every connection here is direct.
            Action::Punch { .. } | Action::StopPunching { .. } => {}
        }
    }

    fn deliver(&mut self, delivery: Delivery) {
        let now = self.now;
        match delivery {
            Delivery::Connected(i, conn, info) => {
                if self.links.contains_key(&(i, conn)) {
                    self.nodes[i].session.handle_connected(now, conn, info);
                }
            }
            Delivery::Inbound(i, conn, inbound) => {
                if self.links.contains_key(&(i, conn)) {
                    self.nodes[i].session.handle_message(now, conn, inbound);
                }
            }
            Delivery::Data {
                to,
                conn,
                request,
                data,
            } => {
                if !self.links.contains_key(&(to, conn)) {
                    return;
                }
                let node = &mut self.nodes[to];
                let Some(check) = node.session.verify_data(conn, request, &data) else {
                    return;
                };
                if check.is_ok() {
                    let Some(OutRequest::Chunks { start, count }) =
                        node.session.request(conn, request)
                    else {
                        unreachable!("verified requests are chunk requests");
                    };
                    node.content.write(start, count, &data);
                }
                let inbound = Inbound::Response {
                    request,
                    response: Response::Data(check),
                };
                node.session.handle_message(now, conn, inbound);
            }
            Delivery::Closed(i, conn, closed) => {
                if self.links.remove(&(i, conn)).is_some() {
                    self.nodes[i].session.handle_closed(now, conn, closed);
                }
            }
            Delivery::DialFailed(i, peer) => self.nodes[i].session.handle_dial_failed(now, peer),
        }
    }
}

pub fn local(peer_id: PeerId) -> LocalPeer {
    LocalPeer {
        peer_id,
        addresses: Vec::new(),
        relay: None,
    }
}
