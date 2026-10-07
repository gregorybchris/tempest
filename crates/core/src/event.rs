//! Decision events: what a session did and why (D-006).
//!
//! The UIs show these as a live log and the simulator records them for analysis, so each event is
//! structured data with a human-readable [`Display`](fmt::Display) form.

use std::fmt;

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
}

/// Why the upload strategy started or stopped serving a peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServeReason {
    /// The cooperative strategy serves every peer that wants data.
    Cooperative,
    /// The peer no longer wants anything this peer has.
    NotWanted,
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
            } => write!(
                f,
                "{conn}: received chunks {start}..{} ({bytes} bytes) from {}",
                start + count,
                short(peer)
            ),
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
        }
    }
}

impl fmt::Display for ServeReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cooperative => write!(f, "the cooperative strategy serves everyone who asks"),
            Self::NotWanted => write!(f, "they no longer want anything we have"),
        }
    }
}
