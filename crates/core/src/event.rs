//! Decision events: what a session did and why (D-006).
//!
//! The UIs show these as a live log and the simulator records them for analysis, so each event is
//! structured data with a human-readable [`Display`](fmt::Display) form.

use std::fmt;
use std::time::Duration;

use tempest_protocol::PeerId;
use tempest_protocol::message::{GoodbyeReason, RejectReason};

use crate::io::{ConnId, Direction, PathKind};
use crate::time::Time;

/// Something a session did or observed, at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// When it happened.
    pub time: Time,
    /// What happened.
    pub kind: EventKind,
}

/// What happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    /// A connection was established.
    Connected {
        /// The connection.
        conn: ConnId,
        /// The other peer.
        peer: PeerId,
        /// Which side dialed.
        direction: Direction,
        /// Whether it is direct or relayed.
        path: PathKind,
    },
    /// Both peers sent and received Hello and Have.
    Opened {
        /// The connection.
        conn: ConnId,
        /// The other peer.
        peer: PeerId,
    },
    /// A connection ended.
    Closed {
        /// The connection.
        conn: ConnId,
        /// The other peer.
        peer: PeerId,
        /// Who closed it and why.
        cause: CloseCause,
    },
    /// Dialing a peer failed.
    DialFailed {
        /// The peer.
        peer: PeerId,
    },
    /// The manifest arrived and matched the bundle ID.
    ManifestReceived {
        /// The connection it came on.
        conn: ConnId,
        /// The peer that sent it.
        peer: PeerId,
    },
    /// This peer requested chunks.
    Requested {
        /// The connection.
        conn: ConnId,
        /// The provider.
        peer: PeerId,
        /// The first chunk.
        start: u32,
        /// The number of chunks requested.
        count: u32,
        /// Why these chunks.
        reason: SelectReason,
    },
    /// Verified Data arrived.
    Received {
        /// The connection.
        conn: ConnId,
        /// The provider.
        peer: PeerId,
        /// The first chunk.
        start: u32,
        /// The number of chunks requested.
        count: u32,
        /// The number of payload bytes.
        bytes: u64,
        /// How many of those bytes were of chunks this peer already had, from a duplicate
        /// endgame request.
        duplicate: u64,
    },
    /// A provider rejected one of this peer's requests.
    RequestRejected {
        /// The connection.
        conn: ConnId,
        /// The provider.
        peer: PeerId,
        /// The provider's reason.
        reason: RejectReason,
    },
    /// This peer finished sending Data.
    Uploaded {
        /// The connection.
        conn: ConnId,
        /// The requester.
        peer: PeerId,
        /// The first chunk.
        start: u32,
        /// The number of chunks requested.
        count: u32,
        /// The number of payload bytes.
        bytes: u64,
    },
    /// This peer started or stopped serving another.
    ServingChanged {
        /// The connection.
        conn: ConnId,
        /// The other peer.
        peer: PeerId,
        /// Whether this peer now serves it.
        serving: bool,
        /// Why.
        reason: ServeReason,
    },
    /// This peer refuses a peer for the rest of the session.
    Banned {
        /// The peer.
        peer: PeerId,
    },
    /// This peer has every chunk.
    Completed,
    /// The number of requests this peer keeps open with a provider changed (D-060).
    InFlightChanged {
        /// The connection.
        conn: ConnId,
        /// The provider.
        peer: PeerId,
        /// The new number of requests.
        requests: u32,
        /// Why that many.
        reason: InFlightReason,
    },
    /// Every missing chunk has been requested, so the remaining ones may be requested from more
    /// than one peer (D-061).
    EndgameStarted {
        /// How many chunks are missing.
        missing: u32,
    },
    /// This peer cancelled one of its requests by resetting its stream.
    Cancelled {
        /// The connection.
        conn: ConnId,
        /// The provider.
        peer: PeerId,
        /// The first chunk.
        start: u32,
        /// The number of chunks requested.
        count: u32,
        /// Why.
        reason: CancelReason,
    },
    /// A provider sent no Data for too long while requests were open: they were cancelled, and
    /// it gets one request at a time until it sends Data again.
    Snubbed {
        /// The connection.
        conn: ConnId,
        /// The provider.
        peer: PeerId,
    },
    /// While superseeding, this peer offered chunks to a peer by announcing them to it alone.
    Offered {
        /// The connection.
        conn: ConnId,
        /// The peer.
        peer: PeerId,
        /// The first chunk.
        start: u32,
        /// The number of chunks.
        count: u32,
    },
    /// This peer stopped superseeding and announced every chunk to everyone.
    SuperseedingEnded {
        /// Why.
        reason: SuperseedEnd,
    },
    /// This peer told a connected peer about other peers in the swarm.
    PeersShared {
        /// The connection.
        conn: ConnId,
        /// The peer told.
        peer: PeerId,
        /// How many peers it was told about.
        count: u32,
        /// Whether this was the last Peers before leaving the swarm (D-224).
        farewell: bool,
    },
    /// This peer stopped sharing the bundle and closed every connection (D-065, D-066).
    StoppedSharing {
        /// Why.
        reason: StopReason,
    },
}

/// Why a session keeps a number of requests open with a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InFlightReason {
    /// The configured number, before the provider's rate and round-trip time are measured or
    /// because pipelining is not adaptive.
    Initial,
    /// Enough to cover the provider's measured rate × round-trip time.
    Measured {
        /// The rate of receiving from the provider, in bytes per second.
        rate: u64,
        /// The round-trip time.
        rtt: Duration,
    },
    /// The provider's `max_open_requests`, less headroom, allows no more.
    ProviderLimit,
    /// The configured minimum.
    Minimum,
    /// The provider is snubbing this peer.
    Snubbed,
}

/// Why this peer cancelled a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelReason {
    /// Another peer already sent every chunk it asked for, during endgame.
    Duplicate,
    /// The provider sent nothing for too long.
    Snubbed,
}

/// Why superseeding ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuperseedEnd {
    /// Every chunk has been sent once.
    AllSent,
    /// A connected peer has every chunk, so this peer is no longer the only complete copy.
    AnotherCopy,
}

/// Why this peer stopped sharing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// It has uploaded at least the configured ratio of what it downloaded.
    RatioReached,
    /// No peer has wanted anything from it for the configured time.
    NobodyWants,
    /// The recipient of a direct send has every chunk.
    Delivered,
}

/// Who closed a connection, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseCause {
    /// This peer sent a Goodbye.
    Local {
        /// The reason code.
        reason: GoodbyeReason,
        /// The reason phrase.
        message: String,
    },
    /// The other peer sent a Goodbye.
    Remote {
        /// The reason code.
        reason: GoodbyeReason,
        /// The reason phrase.
        message: String,
    },
    /// The connection was lost.
    Lost,
}

/// Why the chunk selection strategy chose a chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectReason {
    /// It is among the rarest chunks this provider has.
    Rarest {
        /// How many connected peers have it.
        holders: u32,
    },
    /// Rarest-first picks the first few chunks at random, to have something to trade quickly.
    RandomFirst,
    /// Picked at random.
    Random,
    /// The lowest-numbered chunk still needed.
    Sequential,
    /// It is the next needed chunk after the playback position.
    NearPlayback {
        /// How many chunks after the position it is.
        ahead: u32,
    },
    /// Endgame: every missing chunk is requested, and this one had the fewest open requests.
    Endgame {
        /// How many requests for it were already open.
        copies: u32,
    },
}

/// Why the upload strategy started or stopped serving a peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServeReason {
    /// BitTorrent: the peer is among the fastest uploaders to this peer.
    Reciprocal {
        /// Its recent upload rate to this peer, in bytes per second.
        rate: u64,
    },
    /// BitTorrent: a slot was free, and nobody faster was waiting for it.
    FreeSlot,
    /// BitTorrent and PropShare: picked at random for an optimistic slot, so newcomers can
    /// start.
    Optimistic,
    /// BitTorrent: other peers uploaded to this peer faster.
    Outpaced,
    /// PropShare: the peer uploaded to this peer recently, and gets a proportional share.
    Proportional {
        /// Its share of what this peer received, in thousandths.
        permille: u32,
    },
    /// PropShare: the peer hasn't uploaded to this peer recently.
    NotContributing,
    /// FairTorrent serves every peer that wants data, answering the one owed most first.
    FairTorrent,
    /// The cooperative strategy serves every peer that wants data.
    Cooperative,
    /// A complete peer serves every peer that wants data, in turn.
    RoundRobin,
    /// While superseeding, every peer that wants data is served the chunks offered to it.
    Superseeding,
    /// The peer no longer wants anything this peer has.
    NotWanted,
}

impl EventKind {
    /// A short, stable name for the kind of event, for tables and queries.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Connected { .. } => "connected",
            Self::Opened { .. } => "opened",
            Self::Closed { .. } => "closed",
            Self::DialFailed { .. } => "dial_failed",
            Self::ManifestReceived { .. } => "manifest_received",
            Self::Requested { .. } => "requested",
            Self::Received { .. } => "received",
            Self::RequestRejected { .. } => "request_rejected",
            Self::Uploaded { .. } => "uploaded",
            Self::ServingChanged { serving: true, .. } => "started_serving",
            Self::ServingChanged { serving: false, .. } => "stopped_serving",
            Self::Banned { .. } => "banned",
            Self::Completed => "completed",
            Self::InFlightChanged { .. } => "in_flight_changed",
            Self::EndgameStarted { .. } => "endgame_started",
            Self::Cancelled { .. } => "cancelled",
            Self::Snubbed { .. } => "snubbed",
            Self::Offered { .. } => "offered",
            Self::SuperseedingEnded { .. } => "superseeding_ended",
            Self::PeersShared { .. } => "peers_shared",
            Self::StoppedSharing { .. } => "stopped_sharing",
        }
    }

    /// A short, stable name for the event's reason, if it has one.
    pub fn reason(&self) -> Option<&'static str> {
        Some(match self {
            Self::Closed { cause, .. } => cause.name(),
            Self::Requested { reason, .. } => reason.name(),
            Self::RequestRejected { reason, .. } => match reason {
                RejectReason::Busy => "busy",
                RejectReason::DontHave => "dont_have",
                RejectReason::Invalid => "invalid",
            },
            Self::ServingChanged { reason, .. } => reason.name(),
            Self::InFlightChanged { reason, .. } => reason.name(),
            Self::Cancelled { reason, .. } => match reason {
                CancelReason::Duplicate => "duplicate",
                CancelReason::Snubbed => "snubbed",
            },
            Self::SuperseedingEnded { reason } => match reason {
                SuperseedEnd::AllSent => "all_sent",
                SuperseedEnd::AnotherCopy => "another_copy",
            },
            Self::StoppedSharing { reason } => match reason {
                StopReason::RatioReached => "ratio_reached",
                StopReason::NobodyWants => "nobody_wants",
                StopReason::Delivered => "delivered",
            },
            _ => return None,
        })
    }
}

impl CloseCause {
    /// The Goodbye reason's name, or `lost`.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Local { reason, .. } | Self::Remote { reason, .. } => match reason {
                GoodbyeReason::Done => "done",
                GoodbyeReason::ShuttingDown => "shutting_down",
                GoodbyeReason::UnknownBundle => "unknown_bundle",
                GoodbyeReason::Duplicate => "duplicate",
                GoodbyeReason::ProtocolViolation => "protocol_violation",
                GoodbyeReason::BadData => "bad_data",
                GoodbyeReason::TooManyConnections => "too_many_connections",
            },
            Self::Lost => "lost",
        }
    }
}

impl SelectReason {
    /// A short, stable name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Rarest { .. } => "rarest",
            Self::RandomFirst => "random_first",
            Self::Random => "random",
            Self::Sequential => "sequential",
            Self::NearPlayback { .. } => "near_playback",
            Self::Endgame { .. } => "endgame",
        }
    }
}

impl ServeReason {
    /// A short, stable name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Reciprocal { .. } => "reciprocal",
            Self::FreeSlot => "free_slot",
            Self::Optimistic => "optimistic",
            Self::Outpaced => "outpaced",
            Self::Proportional { .. } => "proportional",
            Self::NotContributing => "not_contributing",
            Self::FairTorrent => "fairtorrent",
            Self::Cooperative => "cooperative",
            Self::RoundRobin => "round_robin",
            Self::Superseeding => "superseeding",
            Self::NotWanted => "not_wanted",
        }
    }
}

impl InFlightReason {
    /// A short, stable name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Initial => "initial",
            Self::Measured { .. } => "measured",
            Self::ProviderLimit => "provider_limit",
            Self::Minimum => "minimum",
            Self::Snubbed => "snubbed",
        }
    }
}

impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.time, self.kind)
    }
}

/// The first eight hex digits of a peer ID, enough to tell peers apart in a log.
fn short(peer: &PeerId) -> String {
    peer.to_string()[..8].to_owned()
}

impl fmt::Display for EventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connected {
                conn,
                peer,
                direction,
                path,
            } => {
                let how = match direction {
                    Direction::Dialed => "dialed",
                    Direction::Accepted => "accepted",
                };
                let path = match path {
                    PathKind::Direct => "direct",
                    PathKind::Relayed => "relayed",
                };
                write!(f, "{conn}: {how} {} ({path})", short(peer))
            }
            Self::Opened { conn, peer } => write!(f, "{conn}: opened with {}", short(peer)),
            Self::Closed { conn, peer, cause } => {
                write!(f, "{conn}: closed with {}: ", short(peer))?;
                match cause {
                    CloseCause::Local { reason, message } => {
                        write!(f, "we sent {reason:?}")?;
                        if !message.is_empty() {
                            write!(f, " ({message})")?;
                        }
                        Ok(())
                    }
                    CloseCause::Remote { reason, message } => {
                        write!(f, "they sent {reason:?}")?;
                        if !message.is_empty() {
                            write!(f, " ({message})")?;
                        }
                        Ok(())
                    }
                    CloseCause::Lost => write!(f, "connection lost"),
                }
            }
            Self::DialFailed { peer } => write!(f, "could not reach {}", short(peer)),
            Self::ManifestReceived { conn, peer } => {
                write!(f, "{conn}: got the manifest from {}", short(peer))
            }
            Self::Requested {
                conn,
                peer,
                start,
                count,
                reason,
            } => write!(
                f,
                "{conn}: requested chunks {start}..{} from {} because {reason}",
                start + count,
                short(peer)
            ),
            Self::Received {
                conn,
                peer,
                start,
                count,
                bytes,
                duplicate,
            } => {
                write!(
                    f,
                    "{conn}: received chunks {start}..{} ({bytes} bytes",
                    start + count
                )?;
                if *duplicate > 0 {
                    write!(f, ", {duplicate} already had")?;
                }
                write!(f, ") from {}", short(peer))
            }
            Self::RequestRejected { conn, peer, reason } => {
                write!(f, "{conn}: {} rejected a request: {reason:?}", short(peer))
            }
            Self::Uploaded {
                conn,
                peer,
                start,
                count,
                bytes,
            } => write!(
                f,
                "{conn}: sent chunks {start}..{} ({bytes} bytes) to {}",
                start + count,
                short(peer)
            ),
            Self::ServingChanged {
                conn,
                peer,
                serving,
                reason,
            } => {
                let verb = if *serving {
                    "serving"
                } else {
                    "stopped serving"
                };
                write!(f, "{conn}: {verb} {} because {reason}", short(peer))
            }
            Self::Banned { peer } => write!(f, "banned {} for sending bad data", short(peer)),
            Self::Completed => write!(f, "download complete"),
            Self::InFlightChanged {
                conn,
                peer,
                requests,
                reason,
            } => write!(
                f,
                "{conn}: keeping {requests} requests open with {} because {reason}",
                short(peer)
            ),
            Self::EndgameStarted { missing } => write!(
                f,
                "endgame: all {missing} missing chunks are requested, so they may be requested again elsewhere"
            ),
            Self::Cancelled {
                conn,
                peer,
                start,
                count,
                reason,
            } => write!(
                f,
                "{conn}: cancelled the request for chunks {start}..{} from {} because {reason}",
                start + count,
                short(peer)
            ),
            Self::Snubbed { conn, peer } => write!(
                f,
                "{conn}: {} sent nothing for too long; its requests go elsewhere",
                short(peer)
            ),
            Self::Offered {
                conn,
                peer,
                start,
                count,
            } => write!(
                f,
                "{conn}: superseeding: offered chunks {start}..{} to {} alone",
                start + count,
                short(peer)
            ),
            Self::SuperseedingEnded { reason } => {
                write!(f, "stopped superseeding because {reason}")
            }
            Self::PeersShared {
                conn,
                peer,
                count,
                farewell,
            } => {
                write!(f, "{conn}: told {} about {count} peers", short(peer))?;
                if *farewell {
                    write!(f, " before leaving")?;
                }
                Ok(())
            }
            Self::StoppedSharing { reason } => write!(f, "stopped sharing because {reason}"),
        }
    }
}

impl fmt::Display for SelectReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rarest { holders: 1 } => write!(f, "only 1 connected peer has them"),
            Self::Rarest { holders } => write!(f, "only {holders} connected peers have them"),
            Self::RandomFirst => write!(f, "the first chunks are picked at random"),
            Self::Random => write!(f, "they were picked at random"),
            Self::Sequential => write!(f, "they are the next needed in order"),
            Self::NearPlayback { ahead } => {
                write!(f, "they are {ahead} chunks after the playback position")
            }
            Self::Endgame { copies } => write!(
                f,
                "endgame: the {copies} request(s) already open for them may finish late"
            ),
        }
    }
}

impl fmt::Display for ServeReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reciprocal { rate } => {
                write!(f, "they upload to us at {}, among the fastest", Rate(*rate))
            }
            Self::FreeSlot => write!(f, "an upload slot was free"),
            Self::Optimistic => write!(
                f,
                "they were picked at random for the optimistic slot, so newcomers can start"
            ),
            Self::Outpaced => write!(f, "other peers upload to us faster"),
            Self::Proportional { permille } => write!(
                f,
                "they sent {}.{}% of what we received recently, and get that share",
                permille / 10,
                permille % 10
            ),
            Self::NotContributing => write!(f, "they haven't uploaded to us recently"),
            Self::FairTorrent => write!(
                f,
                "FairTorrent serves everyone, answering whom we owe most first"
            ),
            Self::Cooperative => write!(f, "the cooperative strategy serves everyone who asks"),
            Self::RoundRobin => write!(f, "we are complete and serve everyone in turn"),
            Self::Superseeding => write!(
                f,
                "we are superseeding and serve each peer the chunks offered to it"
            ),
            Self::NotWanted => write!(f, "they no longer want anything we have"),
        }
    }
}

impl fmt::Display for InFlightReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Initial => write!(f, "that is the configured starting point"),
            Self::Measured { rate, rtt } => write!(
                f,
                "that covers their rate of {} × a round trip of {:.0?}",
                Rate(*rate),
                rtt
            ),
            Self::ProviderLimit => write!(f, "their max_open_requests allows no more"),
            Self::Minimum => write!(f, "that is the configured minimum"),
            Self::Snubbed => write!(f, "they are snubbing us"),
        }
    }
}

impl fmt::Display for CancelReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Duplicate => write!(f, "another peer sent them first"),
            Self::Snubbed => write!(f, "the provider sent nothing for too long"),
        }
    }
}

impl fmt::Display for SuperseedEnd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AllSent => write!(f, "every chunk has been sent once"),
            Self::AnotherCopy => write!(f, "another peer has every chunk"),
        }
    }
}

impl fmt::Display for StopReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RatioReached => write!(f, "we uploaded as much as we were asked to"),
            Self::NobodyWants => write!(f, "nobody wanted anything from us for a while"),
            Self::Delivered => write!(f, "the recipient has every chunk"),
        }
    }
}

/// A rate in bytes per second, written with a decimal unit.
struct Rate(u64);

impl fmt::Display for Rate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let units = [(1e9, "GB/s"), (1e6, "MB/s"), (1e3, "kB/s")];
        let rate = self.0 as f64;
        match units.iter().find(|(size, _)| rate >= *size) {
            Some((size, unit)) => write!(f, "{:.1} {unit}", rate / size),
            None => write!(f, "{} B/s", self.0),
        }
    }
}
