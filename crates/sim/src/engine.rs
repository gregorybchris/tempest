//! The simulation driver: many [`Session`]s, one per peer, over the simulated [`Network`].
//!
//! A run pops events from its [`Queue`] in time order. Each event is something a session must
//! hear about (a connection, a message, a timer) or something the world does (a peer arriving
//! or leaving). After telling a session, the driver carries out the session's actions, which
//! schedule more events, and records its decision events in the [`Log`].
//!
//! Every random choice comes from generators seeded by the run's seed, on separate streams for
//! the world (arrivals, churn and each session's seed), the network, the tracker and the
//! routers. The world is drawn before the run starts and doesn't depend on settings, so every
//! variant of an experiment sees the same arrivals, churn and routers for a given seed (D-073).
//!
//! Beyond the network, the driver models what the client's I/O layer would do: dialing peers
//! behind routers through their relay and then hole punching (`protocol/relays.md`), and the
//! time a peer takes to hash and store each chunk it receives.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use rand::{Rng as _, SeedableRng};
use sha2::{Digest, Sha256};
use tempest_core::content::MemoryContent;
use tempest_core::io::{Closed, ConnInfo, Direction, OutRequest, PathKind};
use tempest_core::{
    Action, Config, ConnId, EventKind, Inbound, LocalPeer, Reply, RequestId, Response, Rng,
    Session, StreamId, Time,
};
use tempest_protocol::message::{
    Data, ManifestMessage, MessageType, PeerRecord, Reject, RejectReason,
};
use tempest_protocol::tree::VerifyError;
use tempest_protocol::{Encode, Hash, Message, PeerId, RangeList};

use crate::bundle::Bundle;
use crate::distribution::{exponential, weibull};
use crate::log::{Entry, LeaveReason, Log, Record, Sink, Wire};
use crate::network::{FlowId, Limit, Link, Network};
use crate::queue::Queue;
use crate::scenario::{Arrivals, Behavior, Payload, Scenario, ScenarioError, Start, Variant};
use crate::strategic::{PEER_SET_FACTOR, Strategic};
use crate::units::Rate;

/// How long a dial to a peer that isn't there takes to fail.
const DIAL_TIMEOUT: Duration = Duration::from_secs(5);

/// Options for one run.
#[derive(Debug, Clone, Copy, Default)]
pub struct RunOptions {
    /// Keep every log entry in [`RunResult::log`], not just the digest.
    pub keep_log: bool,
}

/// What one run did.
#[derive(Debug, Clone)]
pub struct RunResult {
    /// The variant's index in [`Scenario::variants`].
    pub variant: usize,
    /// The seed.
    pub seed: u64,
    /// When the run ended, in simulated time.
    pub end: Time,
    /// Each peer, by index.
    pub peers: Vec<PeerOutcome>,
    /// What was sent, by [`Wire::name`].
    pub traffic: BTreeMap<&'static str, Traffic>,
    /// Payload bytes in the Data that peers started sending. The rest of the traffic is
    /// overhead.
    pub payload_sent: u64,
    /// The number of events the driver handled.
    pub events: u64,
    /// The number of log entries.
    pub log_len: u64,
    /// The SHA-256 of the log, in hex. The same seed gives the same digest.
    pub digest: String,
    /// The log, if [`RunOptions::keep_log`] was set.
    pub log: Vec<Entry>,
    /// How many present peers held each chunk, every [`Scenario::sample_interval`] while the
    /// run lasted.
    pub availability: Vec<Sample>,
}

/// How many present peers held each chunk at one time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// When.
    pub time: Time,
    /// The number of peers present.
    pub peers: u32,
    /// The number of present peers holding the rarest chunk.
    pub rarest: u32,
    /// The mean number of present peers holding a chunk.
    pub mean: f64,
}

/// What one peer did in a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerOutcome {
    /// Its class's index in [`Scenario::peers`].
    pub class: usize,
    /// Whether it started with the whole bundle.
    pub starts: Start,
    /// When it arrived, if it did before the run ended.
    pub arrived: Option<Time>,
    /// When its first connection opened.
    pub first_peer: Option<Time>,
    /// When it first received verified payload.
    pub first_byte: Option<Time>,
    /// When it completed, if it started without the bundle and completed.
    pub completed: Option<Time>,
    /// When it left, if it did.
    pub left: Option<Time>,
    /// Payload bytes it received that passed verification.
    pub downloaded: u64,
    /// Payload bytes it received that failed verification.
    pub bad: u64,
    /// Payload bytes it received of chunks it already had (endgame duplicates).
    pub duplicate: u64,
    /// Payload bytes it finished sending.
    pub uploaded: u64,
    /// With real payloads, whether a peer that completed holds exactly the bundle's bytes.
    pub intact: Option<bool>,
}

/// Messages of one kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Traffic {
    /// How many were sent.
    pub count: u64,
    /// Their total size on the wire.
    pub bytes: u64,
}

/// Runs `variant` of `scenario` with `seed`.
pub fn run(
    scenario: &Scenario,
    variant: &Variant,
    seed: u64,
    options: RunOptions,
) -> Result<RunResult, ScenarioError> {
    run_with_sink(scenario, variant, seed, options, None)
}

/// Runs `variant` of `scenario` with `seed`, passing each row of its log to `sink`.
pub fn run_with_sink(
    scenario: &Scenario,
    variant: &Variant,
    seed: u64,
    options: RunOptions,
    sink: Option<&mut dyn Sink>,
) -> Result<RunResult, ScenarioError> {
    let slots = scenario.slots();
    let resolved = scenario.resolve(variant);
    let plain = Variant {
        index: variant.index,
        values: BTreeMap::new(),
    };
    Ok(Simulation::new(&resolved, &plain, &slots, seed, options, sink)?.run())
}

#[derive(Debug)]
enum Event {
    Arrive(usize),
    Leave(usize),
    Announce(usize),
    Timer(usize, Time),
    Connected {
        conn: ConnId,
        info: ConnInfo,
    },
    /// A relayed connection, named by one end, went direct after a hole punch.
    Upgraded {
        conn: ConnId,
    },
    DialFailed {
        peer: usize,
        target: PeerId,
    },
    Inbound {
        conn: ConnId,
        inbound: Inbound,
    },
    Data {
        conn: ConnId,
        request: RequestId,
        payload: u64,
        data: Option<Data>,
        bad: bool,
        /// Whether the receiver has hashed and stored it, so that its session may hear of it.
        stored: bool,
    },
    Closed {
        conn: ConnId,
        closed: Closed,
    },
}

/// One end of a connection.
#[derive(Debug)]
struct End {
    peer: usize,
    other: ConnId,
    /// Whether this end's peer still considers the connection open.
    open: bool,
    /// When the last small message sent from this end arrives, so that later ones arrive after
    /// it.
    last_arrival: Time,
    /// Whether the connection goes through a relay.
    relayed: bool,
}

/// A bulk transfer in the network: a response on a request stream.
#[derive(Debug)]
struct Transfer {
    /// The sender's end.
    conn: ConnId,
    stream: StreamId,
    kind: TransferKind,
}

#[derive(Debug)]
enum TransferKind {
    Data {
        payload: u64,
        data: Option<Data>,
        bad: bool,
    },
    Manifest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Waiting,
    Present,
    Gone,
}

/// How a dial to a peer would connect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Path {
    Direct,
    /// Through the peer's relay, then direct if the hole punch works.
    Relayed {
        punch: bool,
    },
}

/// A peer's router, if it sits behind one.
#[derive(Debug, Clone, Copy)]
struct Router {
    punchable: bool,
    relay: bool,
}

#[derive(Debug)]
struct Peer {
    id: PeerId,
    class: usize,
    behavior: Behavior,
    seed: u64,
    router: Option<Router>,
    /// When the peer finishes hashing what it has received so far.
    hashed: Time,
    /// When the peer finishes storing what it has verified so far.
    stored: Time,
    state: State,
    session: Option<Session>,
    content: Option<MemoryContent>,
    /// The timer currently scheduled for the session.
    timer: Option<Time>,
    outcome: PeerOutcome,
}

struct Simulation<'a, 's> {
    scenario: &'a Scenario,
    bundle: Bundle,
    manifest_bytes: Vec<u8>,
    /// The bundle's bytes, with real payloads.
    original: Option<MemoryContent>,
    /// Each class's config.
    configs: Vec<Config>,
    variant: usize,
    seed: u64,
    now: Time,
    queue: Queue<Event>,
    network: Network<Transfer>,
    net_rng: Rng,
    tracker_rng: Rng,
    peers: Vec<Peer>,
    by_id: BTreeMap<PeerId, usize>,
    /// Present peers, as the tracker knows them.
    tracker: BTreeSet<usize>,
    ends: BTreeMap<ConnId, End>,
    /// Running transfers, by the sender's end and stream.
    uploads: BTreeMap<(ConnId, StreamId), FlowId>,
    next_conn: u64,
    /// Peers that started without the bundle and have neither completed nor left.
    waiting: usize,
    log: Log<'s>,
    traffic: BTreeMap<&'static str, Traffic>,
    payload_sent: u64,
    events: u64,
    /// How many present peers hold each chunk.
    copies: Vec<u32>,
    availability: Vec<Sample>,
}

/// A peer's ID, derived from its index so that it is the same in every run.
fn peer_id(index: usize) -> PeerId {
    Hash::new(Sha256::digest(format!("tempest-sim peer {index}")).into())
}

/// The time to process `bytes` at `rate`: zero if the rate is unlimited.
fn work(bytes: u64, rate: Rate) -> Duration {
    Duration::try_from_secs_f64(bytes as f64 / rate.get()).unwrap_or(Duration::MAX)
}

impl<'a, 's> Simulation<'a, 's> {
    fn new(
        scenario: &'a Scenario,
        variant: &Variant,
        slots: &[u32],
        seed: u64,
        options: RunOptions,
        sink: Option<&'s mut dyn Sink>,
    ) -> Result<Self, ScenarioError> {
        let invalid = |field: String, message: String| ScenarioError::Invalid { field, message };
        let (bundle, original) = match scenario.payload {
            Payload::Virtual => (scenario.bundle.build(), None),
            Payload::Real => match scenario.bundle.build_real() {
                Ok((bundle, content)) => (Ok(bundle), Some(content)),
                Err(e) => (Err(e), None),
            },
        };
        let bundle = bundle.map_err(|e| invalid("bundle".into(), e))?;
        let configs = (0..scenario.peers.len())
            .map(|class| {
                let mut config = scenario
                    .config(class, variant)
                    .map_err(|e| invalid(format!("peers[{class}].overrides"), e.to_string()))?;
                if scenario.peers[class].behavior == Behavior::Strategic {
                    config.connections.max = config.connections.max.saturating_mul(PEER_SET_FACTOR);
                }
                Ok(config)
            })
            .collect::<Result<Vec<_>, _>>()?;

        let stream = |n| {
            let mut rng = Rng::seed_from_u64(seed);
            rng.set_stream(n);
            rng
        };
        let mut world = stream(0);
        // Routers draw from their own stream, so that adding them leaves the rest of the world
        // as it was.
        let mut routers = stream(3);
        let bundle_chunks = bundle.chunk_count();
        let mut sim = Self {
            scenario,
            manifest_bytes: bundle.manifest().to_bytes(),
            bundle,
            original,
            configs,
            variant: variant.index,
            seed,
            now: Time::ZERO,
            queue: Queue::new(),
            network: Network::new(),
            net_rng: stream(1),
            tracker_rng: stream(2),
            peers: Vec::new(),
            by_id: BTreeMap::new(),
            tracker: BTreeSet::new(),
            ends: BTreeMap::new(),
            uploads: BTreeMap::new(),
            next_conn: 0,
            waiting: 0,
            log: Log::new(options.keep_log, sink),
            traffic: BTreeMap::new(),
            payload_sent: 0,
            events: 0,
            copies: vec![0; bundle_chunks as usize],
            availability: Vec::new(),
        };

        // A peer's ID comes from its slot, so that it is the same in every variant.
        let mut first_slot = 0usize;
        for (class, spec) in scenario.peers.iter().enumerate() {
            let class_slot = first_slot;
            first_slot += slots[class].max(spec.count) as usize;
            // The time since the start of the class's Poisson arrivals.
            let mut poisson = Duration::ZERO;
            for k in 0..slots[class].max(spec.count) {
                // Every peer draws the same numbers whatever its class uses, and so does every
                // slot that another variant fills, so that changing one class leaves the others'
                // randomness alone.
                let (u_arrival, u_session, session_seed): (f64, f64, u64) =
                    (world.random(), world.random(), world.random());
                let u_router: f64 = routers.random();
                if k >= spec.count {
                    continue;
                }
                let arrive = Time::ZERO
                    + match spec.arrivals {
                        Arrivals::AtOnce { at } => at,
                        Arrivals::Steady { start, interval } => {
                            start.saturating_add(interval.saturating_mul(k))
                        }
                        Arrivals::Poisson { start, interval } => {
                            poisson = poisson.saturating_add(exponential(interval, u_arrival));
                            start.saturating_add(poisson)
                        }
                        Arrivals::FlashCrowd { start, decay } => {
                            start.saturating_add(exponential(decay, u_arrival))
                        }
                    };
                let index = sim.peers.len();
                sim.network.add_node(Link {
                    up: spec.bandwidth.up,
                    down: spec.bandwidth.down,
                    latency: spec.latency.unwrap_or(scenario.network.latency),
                    loss: spec.loss.unwrap_or(scenario.network.loss),
                });
                sim.queue.push(arrive, Event::Arrive(index));
                if let Some(churn) = spec.churn {
                    let leave = arrive + weibull(churn.session, churn.shape, u_session);
                    sim.queue.push(leave, Event::Leave(index));
                }
                if spec.starts == Start::Empty {
                    sim.waiting += 1;
                }
                let id = peer_id(class_slot + k as usize);
                sim.by_id.insert(id, index);
                sim.peers.push(Peer {
                    id,
                    class,
                    behavior: spec.behavior,
                    seed: session_seed,
                    router: spec.nat.map(|nat| Router {
                        punchable: u_router < nat.punchable.get(),
                        relay: nat.relay,
                    }),
                    hashed: Time::ZERO,
                    stored: Time::ZERO,
                    state: State::Waiting,
                    session: None,
                    content: None,
                    timer: None,
                    outcome: PeerOutcome {
                        class,
                        starts: spec.starts,
                        arrived: None,
                        first_peer: None,
                        first_byte: None,
                        completed: None,
                        left: None,
                        downloaded: 0,
                        bad: 0,
                        duplicate: 0,
                        uploaded: 0,
                        intact: None,
                    },
                });
            }
        }
        Ok(sim)
    }

    fn run(mut self) -> RunResult {
        let limit = Time::ZERO + self.scenario.duration;
        let interval = self.scenario.sample_interval;
        let mut next_sample = Time::ZERO + interval;
        while self.waiting > 0 {
            // A flow finishing goes before events at the same time.
            let finish = self.network.next_finish();
            let next = match (finish, self.queue.peek_time()) {
                (Some((at, _)), Some(time)) => Some(at.min(time)),
                (finish, time) => finish.map(|(at, _)| at).or(time),
            };
            let time = match next {
                Some(time) if time <= limit => time,
                _ => {
                    self.now = limit;
                    while next_sample <= limit {
                        self.sample(next_sample);
                        next_sample += interval;
                    }
                    break;
                }
            };
            // Samples see everything that happened before them.
            while next_sample < time {
                self.sample(next_sample);
                next_sample += interval;
            }
            self.now = time;
            self.events += 1;
            match finish {
                Some((at, flow)) if at == time => {
                    let transfer = self.network.end(time, flow);
                    self.transfer_done(transfer);
                }
                _ => {
                    let (_, event) = self.queue.pop().expect("peeked");
                    self.handle(event);
                }
            }
        }
        self.finish()
    }

    fn finish(mut self) -> RunResult {
        for peer in &mut self.peers {
            if let Some(session) = &peer.session {
                peer.outcome.duplicate = session.duplicate();
            }
            if let (Some(content), Some(original), Some(_)) =
                (&peer.content, &self.original, peer.outcome.completed)
            {
                peer.outcome.intact = Some(content.files() == original.files());
            }
        }
        let (digest, log_len, log) = self.log.finish();
        RunResult {
            variant: self.variant,
            seed: self.seed,
            end: self.now,
            peers: self.peers.into_iter().map(|p| p.outcome).collect(),
            traffic: self.traffic,
            payload_sent: self.payload_sent,
            events: self.events,
            log_len,
            digest,
            log,
            availability: self.availability,
        }
    }

    /// Records how many present peers hold each chunk.
    fn sample(&mut self, time: Time) {
        let peers = self
            .peers
            .iter()
            .filter(|p| p.state == State::Present)
            .count();
        let total: u64 = self.copies.iter().map(|&c| u64::from(c)).sum();
        self.availability.push(Sample {
            time,
            peers: peers as u32,
            rarest: self.copies.iter().copied().min().unwrap_or(0),
            mean: total as f64 / self.copies.len().max(1) as f64,
        });
    }

    /// Counts the chunks `peer`'s session holds as one more copy each, or one fewer.
    fn count_held(&mut self, peer: usize, delta: i32) {
        let Some(have) = self.peers[peer].session.as_ref().and_then(|s| s.have()) else {
            return;
        };
        for chunk in have.iter() {
            let copies = &mut self.copies[chunk as usize];
            *copies = copies
                .checked_add_signed(delta)
                .expect("copies stay counted");
        }
    }

    fn record(&mut self, peer: usize, time: Time, record: Record) {
        let entry = Entry {
            time,
            peer: peer as u32,
            record,
        };
        self.log.push(entry, &self.by_id);
    }

    fn present(&self, peer: usize) -> bool {
        self.peers[peer].state == State::Present
    }

    fn session(&mut self, peer: usize) -> &mut Session {
        self.peers[peer]
            .session
            .as_mut()
            .expect("present peers have a session")
    }

    /// The open end `conn` and its peer, if that peer is still here.
    fn open_end(&self, conn: ConnId) -> Option<usize> {
        self.ends
            .get(&conn)
            .filter(|end| end.open)
            .map(|end| end.peer)
            .filter(|&peer| self.present(peer))
    }

    // ----- Events -----

    fn handle(&mut self, event: Event) {
        let now = self.now;
        match event {
            Event::Arrive(peer) => self.arrive(peer),
            Event::Leave(peer) => {
                if self.present(peer) {
                    self.leave(peer, LeaveReason::Churn);
                }
            }
            Event::Announce(peer) => {
                if self.present(peer) {
                    self.announce(peer);
                    self.after(peer);
                }
            }
            Event::Timer(peer, at) => {
                if self.present(peer) && self.peers[peer].timer == Some(at) {
                    self.peers[peer].timer = None;
                    let session = self.session(peer);
                    session.handle_timer(now);
                    assert!(
                        session.next_timer().is_none_or(|t| t > now),
                        "a session's timer must move forward after it fires"
                    );
                    self.after(peer);
                }
            }
            Event::Connected { conn, info } => self.connected(conn, info),
            Event::Upgraded { conn } => self.upgraded(conn),
            Event::DialFailed { peer, target } => {
                if self.present(peer) {
                    self.session(peer).handle_dial_failed(now, target);
                    self.after(peer);
                }
            }
            Event::Inbound { conn, inbound } => {
                let Some(peer) = self.open_end(conn) else {
                    return;
                };
                if let Inbound::RequestReset { stream } = inbound
                    && let Some(flow) = self.uploads.remove(&(conn, stream))
                {
                    self.network.end(now, flow);
                }
                self.session(peer).handle_message(now, conn, inbound);
                self.after(peer);
            }
            Event::Data {
                conn,
                request,
                payload,
                data,
                bad,
                stored,
            } => {
                if stored {
                    self.data_stored(conn, request, payload, data, bad);
                } else {
                    self.data_arrived(conn, request, payload, data, bad);
                }
            }
            Event::Closed { conn, closed } => {
                let Some(peer) = self.open_end(conn) else {
                    return;
                };
                self.close_end(conn);
                self.session(peer).handle_closed(now, conn, closed);
                self.after(peer);
            }
        }
    }

    fn arrive(&mut self, index: usize) {
        let now = self.now;
        let config = self.configs[self.peers[index].class].clone();
        let peer = &mut self.peers[index];
        let local = LocalPeer {
            peer_id: peer.id,
            addresses: Vec::new(),
            relay: None,
        };
        let complete = peer.outcome.starts == Start::Complete;
        let session = if complete {
            let have = RangeList::from_chunks(0..self.bundle.chunk_count());
            let manifest = self.bundle.manifest().clone();
            Session::with_manifest(config, local, manifest, &have, peer.seed)
        } else {
            Session::new(config, local, self.bundle.manifest().bundle_id(), peer.seed)
        };
        let mut session = session.expect("scenario configs are validated");
        if peer.behavior == Behavior::Strategic && !complete {
            let settings = &self.configs[peer.class].upload;
            session.set_upload_strategy(Box::new(Strategic::new(settings)));
        }
        peer.session = Some(session);
        peer.content = self.original.as_ref().map(|original| {
            if complete {
                original.clone()
            } else {
                original.empty_copy()
            }
        });
        peer.state = State::Present;
        peer.outcome.arrived = Some(now);
        self.count_held(index, 1);
        self.record(index, now, Record::Arrived);
        self.announce(index);
        self.after(index);
    }

    /// Announces to the tracker, which answers with up to `tracker.peers` others at random.
    fn announce(&mut self, peer: usize) {
        let now = self.now;
        let mut others: Vec<usize> = self
            .tracker
            .iter()
            .copied()
            .filter(|&p| p != peer)
            .collect();
        let want = (self.scenario.tracker.peers as usize).min(others.len());
        for i in 0..want {
            let j = self.tracker_rng.random_range(i..others.len());
            others.swap(i, j);
        }
        let records: Vec<PeerRecord> = others[..want]
            .iter()
            .map(|&p| PeerRecord {
                peer_id: self.peers[p].id,
                addresses: Vec::new(),
                relay: None,
            })
            .collect();
        self.tracker.insert(peer);
        self.session(peer).add_peers(now, records);
        self.queue
            .push(now + self.scenario.tracker.interval, Event::Announce(peer));
    }

    fn leave(&mut self, peer: usize, reason: LeaveReason) {
        let now = self.now;
        if !self.session(peer).is_stopped() {
            self.session(peer).shutdown(now);
            self.flush(peer);
        }
        self.count_held(peer, -1);
        let state = &mut self.peers[peer];
        state.state = State::Gone;
        state.timer = None;
        state.outcome.left = Some(now);
        if let Some(session) = state.session.take() {
            state.outcome.duplicate = session.duplicate();
        }
        if state.outcome.starts == Start::Empty && state.outcome.completed.is_none() {
            self.waiting -= 1;
        }
        self.tracker.remove(&peer);
        self.record(peer, now, Record::Left(reason));
    }

    fn connected(&mut self, conn: ConnId, info: ConnInfo) {
        let now = self.now;
        let Some(end) = self.ends.get(&conn) else {
            return;
        };
        let (peer, other) = (end.peer, end.other);
        if !end.open {
            return;
        }
        if !self.present(peer) {
            // The peer left while the connection was being set up: the other end times out.
            self.close_end(conn);
            let other_peer = self.ends.get(&other).map(|e| e.peer);
            if let Some(other_peer) = other_peer {
                let at = now + self.network.latency(peer, other_peer) + self.detour(conn);
                self.queue.push(
                    at,
                    Event::Closed {
                        conn: other,
                        closed: Closed::Lost,
                    },
                );
            }
            return;
        }
        let detour = self.detour(conn).saturating_mul(2);
        let rtt = self
            .network
            .rtt(peer, self.ends[&other].peer)
            .saturating_add(detour);
        let session = self.session(peer);
        session.handle_connected(now, conn, info);
        session.handle_rtt(now, conn, rtt);
        self.after(peer);
    }

    /// Data arrived: the receiver hashes it, then stores it if it passes, each in turn after
    /// what it received before. Its session hears of it once that is done.
    fn data_arrived(
        &mut self,
        conn: ConnId,
        request: RequestId,
        payload: u64,
        data: Option<Data>,
        bad: bool,
    ) {
        let now = self.now;
        let Some(peer) = self.open_end(conn) else {
            return;
        };
        let class = &self.scenario.peers[self.peers[peer].class];
        let (hashing, disk) = (class.hashing, class.disk);
        let state = &mut self.peers[peer];
        state.hashed = state.hashed.max(now) + work(payload, hashing);
        let mut done = state.hashed;
        if !bad {
            state.stored = state.stored.max(done) + work(payload, disk);
            done = state.stored;
        }
        if done == now {
            self.data_stored(conn, request, payload, data, bad);
        } else {
            let event = Event::Data {
                conn,
                request,
                payload,
                data,
                bad,
                stored: true,
            };
            self.queue.push(done, event);
        }
    }

    /// Data has been hashed and, if it passed, stored: the receiver's session hears of it.
    fn data_stored(
        &mut self,
        conn: ConnId,
        request: RequestId,
        payload: u64,
        data: Option<Data>,
        bad: bool,
    ) {
        let now = self.now;
        let Some(peer) = self.open_end(conn) else {
            return;
        };
        let state = &mut self.peers[peer];
        let session = state.session.as_mut().expect("present");
        let Some(OutRequest::Chunks { start, count }) = session.request(conn, request) else {
            // Cancelled: the session no longer expects it.
            return;
        };
        let check = match (&data, &mut state.content) {
            (Some(data), Some(content)) => {
                let check = session
                    .verify_data(conn, request, data)
                    .expect("the request is open");
                if check.is_ok() {
                    content.write(start, count, data);
                }
                check
            }
            _ if bad => Err(VerifyError::RootMismatch),
            _ => Ok(()),
        };
        if check.is_ok() {
            state.outcome.downloaded += payload;
        } else {
            state.outcome.bad += payload;
        }
        let missing: Vec<u32> = match session.have() {
            Some(have) => (start..start + count)
                .filter(|&c| !have.contains(c))
                .collect(),
            None => Vec::new(),
        };
        let response = Response::Data(check);
        session.handle_message(now, conn, Inbound::Response { request, response });
        if let Some(have) = session.have() {
            for chunk in missing.into_iter().filter(|&c| have.contains(c)) {
                self.copies[chunk as usize] += 1;
            }
        }
        self.after(peer);
    }

    fn transfer_done(&mut self, transfer: Transfer) {
        let now = self.now;
        let Transfer { conn, stream, kind } = transfer;
        self.uploads.remove(&(conn, stream));
        let Some(end) = self.ends.get(&conn) else {
            return;
        };
        let (sender, other) = (end.peer, end.other);
        let receiver = self.ends[&other].peer;
        let detour = self.detour(conn);
        let arrive = now
            + self
                .network
                .delay_via(sender, receiver, detour, &mut self.net_rng);
        let request = RequestId(stream.0);
        match kind {
            TransferKind::Data { payload, data, bad } => {
                self.peers[sender].outcome.uploaded += payload;
                let sent = Inbound::DataSent { stream };
                self.queue.push(
                    now,
                    Event::Inbound {
                        conn,
                        inbound: sent,
                    },
                );
                let event = Event::Data {
                    conn: other,
                    request,
                    payload,
                    data,
                    bad,
                    stored: false,
                };
                self.queue.push(arrive, event);
            }
            TransferKind::Manifest => {
                let response = Response::Manifest(ManifestMessage {
                    manifest: self.manifest_bytes.clone(),
                });
                let inbound = Inbound::Response { request, response };
                self.queue.push(
                    arrive,
                    Event::Inbound {
                        conn: other,
                        inbound,
                    },
                );
            }
        }
    }

    // ----- Sessions' actions -----

    /// Carries out a session's actions and records its events, then reschedules its timer or,
    /// if it stopped sharing, has it leave.
    fn after(&mut self, peer: usize) {
        self.flush(peer);
        if self.session(peer).is_stopped() {
            self.leave(peer, LeaveReason::Stopped);
            return;
        }
        let next = self.session(peer).next_timer();
        let state = &mut self.peers[peer];
        if next != state.timer {
            state.timer = next;
            if let Some(at) = next {
                self.queue.push(at.max(self.now), Event::Timer(peer, at));
            }
        }
    }

    /// Carries out a session's actions and records its events.
    fn flush(&mut self, peer: usize) {
        let session = self.session(peer);
        let actions = session.poll_actions();
        let events = session.poll_events();
        for action in actions {
            self.perform(peer, action);
        }
        for event in events {
            let outcome = &mut self.peers[peer].outcome;
            match event.kind {
                EventKind::Completed if outcome.starts == Start::Empty => {
                    outcome.completed = Some(event.time);
                    self.waiting -= 1;
                }
                EventKind::Opened { .. } => {
                    outcome.first_peer.get_or_insert(event.time);
                }
                EventKind::Received { bytes, .. } if bytes > 0 => {
                    outcome.first_byte.get_or_insert(event.time);
                }
                _ => {}
            }
            self.record(peer, event.time, Record::Decision(event.kind));
        }
    }

    fn perform(&mut self, peer: usize, action: Action) {
        match action {
            Action::Dial(record) => self.dial(peer, record.peer_id),
            Action::Send { conn, message } => {
                let wire = Wire::Message(message.message_type());
                let bytes = message.encode_frame().len();
                self.send_small(conn, wire, bytes, Inbound::Control(message));
            }
            Action::Request {
                conn,
                request,
                message,
            } => {
                let message = message.message();
                let wire = Wire::Message(message.message_type());
                let bytes = message.encode_frame().len();
                let stream = StreamId(request.0);
                self.send_small(conn, wire, bytes, Inbound::Request { stream, message });
            }
            Action::Cancel { conn, request } => {
                let stream = StreamId(request.0);
                self.send_small(conn, Wire::Reset, 0, Inbound::RequestReset { stream });
            }
            Action::Respond {
                conn,
                stream,
                reply,
            } => self.respond(peer, conn, stream, reply),
            Action::Close {
                conn,
                reason,
                message,
            } => self.close(conn, Closed::Goodbye { reason, message }),
            // The simulator already holds the manifest.
            Action::StoreManifest { .. } => {}
            // The simulator models hole punching itself, as an upgrade of the relayed connection.
            Action::Punch { .. } | Action::StopPunching { .. } => {}
        }
    }

    /// How `peer` can reach `target`, if it can: directly unless `target` is behind a router,
    /// otherwise through `target`'s relay if it has one.
    fn path(&self, peer: usize, target: usize) -> Option<Path> {
        let Some(theirs) = self.peers[target].router else {
            return Some(Path::Direct);
        };
        if !theirs.relay {
            return None;
        }
        // A directly reachable dialer is dialed back; otherwise both routers must allow it.
        let punch = self.peers[peer]
            .router
            .is_none_or(|ours| ours.punchable && theirs.punchable);
        Some(Path::Relayed { punch })
    }

    /// The extra one-way latency of the path of the connection with end `conn`.
    fn detour(&self, conn: ConnId) -> Duration {
        match self.ends.get(&conn) {
            Some(end) if end.relayed => self.scenario.relay.latency.saturating_mul(2),
            _ => Duration::ZERO,
        }
    }

    fn dial(&mut self, peer: usize, target: PeerId) {
        let now = self.now;
        let found = self
            .by_id
            .get(&target)
            .copied()
            .filter(|&p| self.present(p));
        let Some((other, path)) = found.and_then(|other| Some((other, self.path(peer, other)?)))
        else {
            self.queue
                .push(now + DIAL_TIMEOUT, Event::DialFailed { peer, target });
            return;
        };
        let (a, b) = (ConnId(self.next_conn), ConnId(self.next_conn + 1));
        self.next_conn += 2;
        let relayed = path != Path::Direct;
        for (conn, owner, other_end) in [(a, peer, b), (b, other, a)] {
            self.ends.insert(
                conn,
                End {
                    peer: owner,
                    other: other_end,
                    open: true,
                    last_arrival: Time::ZERO,
                    relayed,
                },
            );
        }
        // The handshake takes a round trip, after which both ends know of the connection.
        let direct_rtt = self.network.rtt(peer, other);
        let rtt = direct_rtt.saturating_add(self.detour(a).saturating_mul(2));
        let at = now + rtt;
        if path == (Path::Relayed { punch: true }) {
            // The peers exchange Candidates through the relay, then dial each other directly.
            let upgrade = at + rtt + direct_rtt;
            self.queue.push(upgrade, Event::Upgraded { conn: a });
        }
        let path = if relayed {
            PathKind::Relayed
        } else {
            PathKind::Direct
        };
        let info = |peer, direction| ConnInfo {
            peer,
            direction,
            path,
        };
        let dialer = info(target, Direction::Dialed);
        let acceptor = info(self.peers[peer].id, Direction::Accepted);
        self.queue.push(
            at,
            Event::Connected {
                conn: a,
                info: dialer,
            },
        );
        self.queue.push(
            at,
            Event::Connected {
                conn: b,
                info: acceptor,
            },
        );
    }

    /// A relayed connection goes direct: its messages stop detouring, and its transfers stop
    /// sharing the relay's limit. The client would open a new connection and close the relayed
    /// one; carrying on with the same one is the simulator's shortcut.
    fn upgraded(&mut self, conn: ConnId) {
        let now = self.now;
        let Some(other) = self.ends.get(&conn).filter(|e| e.open).map(|e| e.other) else {
            return;
        };
        if !self.ends.get(&other).is_some_and(|e| e.open) {
            return;
        }
        let mut peers = Vec::new();
        for (end, other_end) in [(conn, other), (other, conn)] {
            let state = self.ends.get_mut(&end).expect("open");
            state.relayed = false;
            let (peer, to) = (state.peer, self.ends[&other_end].peer);
            self.network.unlimit(now, peer, end.0);
            self.record(peer, now, Record::Upgraded { to: to as u32 });
            peers.push((peer, end, to));
        }
        // Each session measures the shorter round trip.
        for (peer, end, to) in peers {
            if self.present(peer) {
                let rtt = self.network.rtt(peer, to);
                self.session(peer).handle_rtt(now, end, rtt);
                self.after(peer);
            }
        }
    }

    fn count(&mut self, from: usize, to: usize, wire: Wire, bytes: usize) {
        let traffic = self.traffic.entry(wire.name()).or_default();
        traffic.count += 1;
        traffic.bytes += bytes as u64;
        let to = to as u32;
        let bytes = bytes as u64;
        self.record(from, self.now, Record::Sent { to, wire, bytes });
    }

    /// Sends a small message from end `conn`: it arrives after everything sent before it on the
    /// connection.
    fn send_small(&mut self, conn: ConnId, wire: Wire, bytes: usize, inbound: Inbound) {
        let Some(end) = self.ends.get(&conn).filter(|e| e.open) else {
            return;
        };
        let (from, other, last) = (end.peer, end.other, end.last_arrival);
        let to = self.ends[&other].peer;
        self.count(from, to, wire, bytes);
        let sent = self.now + self.network.send_time(from, to, bytes);
        let detour = self.detour(conn);
        let delay = self.network.delay_via(from, to, detour, &mut self.net_rng);
        let arrive = (sent + delay).max(last);
        self.ends.get_mut(&conn).expect("open").last_arrival = arrive;
        self.queue.push(
            arrive,
            Event::Inbound {
                conn: other,
                inbound,
            },
        );
    }

    fn respond(&mut self, peer: usize, conn: ConnId, stream: StreamId, reply: Reply) {
        let request = RequestId(stream.0);
        let kind = match reply {
            Reply::Data { .. } if self.peers[peer].behavior == Behavior::FreeRider => {
                let reject = Reject {
                    reason: RejectReason::Busy,
                    message: String::new(),
                };
                self.reject(conn, request, reject);
                // The free rider's own session believes it sent the Data.
                let sent = Inbound::DataSent { stream };
                self.queue.push(
                    self.now,
                    Event::Inbound {
                        conn,
                        inbound: sent,
                    },
                );
                return;
            }
            Reply::Data { start, count } => {
                let bad = self.peers[peer].behavior == Behavior::BadData;
                let (payload, frame) = self.bundle.data_len(start, count);
                let data = self.peers[peer].content.as_ref().map(|content| {
                    let mut data = content.data(start, count);
                    if bad && let Some(byte) = data.chunks.first_mut() {
                        *byte ^= 1;
                    }
                    data
                });
                (
                    TransferKind::Data { payload, data, bad },
                    MessageType::Data,
                    frame,
                )
            }
            Reply::Manifest => (
                TransferKind::Manifest,
                MessageType::Manifest,
                self.bundle.manifest_frame_len(),
            ),
            Reply::Reject(reject) => {
                self.reject(conn, request, reject);
                return;
            }
        };
        let (kind, message_type, bytes) = kind;
        let Some(end) = self.ends.get(&conn).filter(|e| e.open) else {
            return;
        };
        let (from, to) = (end.peer, self.ends[&end.other].peer);
        let limit = end.relayed.then(|| Limit {
            group: conn.0,
            rate: self.scenario.relay.bandwidth.get(),
        });
        self.count(from, to, Wire::Message(message_type), bytes);
        if let TransferKind::Data { payload, .. } = kind {
            self.payload_sent += payload;
        }
        let transfer = Transfer { conn, stream, kind };
        let flow = self
            .network
            .start_limited(self.now, from, to, bytes as u64, limit, transfer);
        self.uploads.insert((conn, stream), flow);
    }

    fn reject(&mut self, conn: ConnId, request: RequestId, reject: Reject) {
        let bytes = Message::Reject(reject.clone()).encode_frame().len();
        let response = Response::Reject(reject);
        let wire = Wire::Message(MessageType::Reject);
        self.send_small(conn, wire, bytes, Inbound::Response { request, response });
    }

    /// Closes a connection from end `conn`: transfers both ways stop, and the other end hears
    /// of it after the messages sent before it.
    fn close(&mut self, conn: ConnId, closed: Closed) {
        let Some(end) = self.ends.get(&conn).filter(|e| e.open) else {
            return;
        };
        let (from, other, last) = (end.peer, end.other, end.last_arrival);
        let to = self.ends[&other].peer;
        let bytes = match &closed {
            Closed::Goodbye { message, .. } => message.len(),
            Closed::Lost => 0,
        };
        self.count(from, to, Wire::Goodbye, bytes);
        let detour = self.detour(conn);
        let delay = self.network.delay_via(from, to, detour, &mut self.net_rng);
        let arrive = (self.now + delay).max(last);
        self.queue.push(
            arrive,
            Event::Closed {
                conn: other,
                closed,
            },
        );
        self.close_end(conn);
        self.abort_uploads(other);
    }

    /// Marks end `conn` closed and stops its transfers. Once both ends are closed, the
    /// connection is forgotten.
    fn close_end(&mut self, conn: ConnId) {
        let Some(end) = self.ends.get_mut(&conn) else {
            return;
        };
        end.open = false;
        let other = end.other;
        self.abort_uploads(conn);
        if self.ends.get(&other).is_none_or(|e| !e.open) {
            self.ends.remove(&conn);
            self.ends.remove(&other);
        }
    }

    /// Stops every transfer sent from end `conn`.
    fn abort_uploads(&mut self, conn: ConnId) {
        let streams: Vec<(ConnId, StreamId)> = self
            .uploads
            .range((conn, StreamId(0))..=(conn, StreamId(u64::MAX)))
            .map(|(&key, _)| key)
            .collect();
        for key in streams {
            let flow = self.uploads.remove(&key).expect("listed");
            self.network.end(self.now, flow);
        }
    }
}
