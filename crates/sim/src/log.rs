//! The event log of a run: every peer arriving and leaving, every message sent, and every
//! decision event (D-006), in the order they happened.
//!
//! Each entry is flattened into a [`Row`], the form the results database stores. The rows are
//! digested as the log grows, so two runs can be compared without keeping either log: the same
//! seed must give the same digest (D-068). A [`Sink`] receives the rows as they are made.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use sha2::{Digest, Sha256};
use tempest_core::event::{CloseCause, InFlightReason, SelectReason, ServeReason};
use tempest_core::io::{Direction, PathKind};
use tempest_core::{EventKind, Time};
use tempest_protocol::PeerId;
use tempest_protocol::message::MessageType;

/// One entry in the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// When it happened.
    pub time: Time,
    /// The peer it happened to, by its index in the run.
    pub peer: u32,
    /// What happened.
    pub record: Record,
}

/// What happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Record {
    /// The peer joined the swarm.
    Arrived,
    /// The peer left the swarm.
    Left(LeaveReason),
    /// The peer sent something to another.
    Sent {
        /// The receiving peer.
        to: u32,
        /// What was sent.
        wire: Wire,
        /// Its size on the wire.
        bytes: u64,
    },
    /// The peer's relayed connection to another went direct after a hole punch.
    Upgraded {
        /// The other peer.
        to: u32,
    },
    /// The peer's session made a decision or observed something.
    Decision(EventKind),
}

/// Why a peer left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaveReason {
    /// Its session stopped sharing (lingering, D-065).
    Stopped,
    /// Its session length ran out (churn).
    Churn,
}

impl LeaveReason {
    /// A short, stable name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Churn => "churn",
        }
    }
}

/// Something sent on a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wire {
    /// A protocol message.
    Message(MessageType),
    /// A request stream's reset, cancelling the request.
    Reset,
    /// The connection's close, carrying Goodbye's reason.
    Goodbye,
}

impl Wire {
    /// A short name for tables.
    pub fn name(self) -> &'static str {
        match self {
            Self::Message(t) => match t {
                MessageType::Hello => "Hello",
                MessageType::Have => "Have",
                MessageType::HaveUpdate => "HaveUpdate",
                MessageType::Want => "Want",
                MessageType::Serving => "Serving",
                MessageType::Peers => "Peers",
                MessageType::Candidates => "Candidates",
                MessageType::Request => "Request",
                MessageType::Data => "Data",
                MessageType::Reject => "Reject",
                MessageType::ManifestRequest => "ManifestRequest",
                MessageType::Manifest => "Manifest",
            },
            Self::Reset => "Reset",
            Self::Goodbye => "Goodbye",
        }
    }
}

/// A log entry flattened into columns. Fields that don't apply to an entry are `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Row {
    /// The entry's position in the log, from 0.
    pub seq: u64,
    /// When it happened.
    pub time: Time,
    /// The peer it happened to.
    pub peer: u32,
    /// `arrived`, `left`, `sent`, `upgraded`, or a decision event's [`EventKind::name`].
    pub kind: &'static str,
    /// The other peer involved: the receiver of a message, or the peer a decision concerns.
    pub other: Option<u32>,
    /// The connection, as the peer's session names it.
    pub conn: Option<u64>,
    /// What was sent, by [`Wire::name`].
    pub message: Option<&'static str>,
    /// The first chunk of a range.
    pub start: Option<u32>,
    /// How many chunks, requests or peers: the range's length, the new number of requests in
    /// flight, the chunks missing at endgame, or the peers shared.
    pub count: Option<u32>,
    /// Bytes on the wire for a message, or payload bytes for a transfer.
    pub bytes: Option<u64>,
    /// A short name for why: the decision's [`EventKind::reason`], or why a peer left.
    pub reason: Option<&'static str>,
    /// A decision event's readable description.
    pub detail: Option<String>,
}

impl Row {
    /// Fills the row from `entry`, naming peers by their index in `peers`. A decision event's
    /// readable detail is written only if `describe` is set, since formatting every event costs
    /// more than the rest of the log together.
    fn fill(
        &mut self,
        seq: u64,
        entry: &Entry,
        peers: &BTreeMap<PeerId, usize>,
        describe: bool,
    ) {
        let detail = self.detail.take();
        *self = Self {
            seq,
            time: entry.time,
            peer: entry.peer,
            ..Self::default()
        };
        match &entry.record {
            Record::Arrived => self.kind = "arrived",
            Record::Left(reason) => {
                self.kind = "left";
                self.reason = Some(reason.name());
            }
            Record::Sent { to, wire, bytes } => {
                self.kind = "sent";
                self.other = Some(*to);
                self.message = Some(wire.name());
                self.bytes = Some(*bytes);
            }
            Record::Upgraded { to } => {
                self.kind = "upgraded";
                self.other = Some(*to);
            }
            Record::Decision(event) => {
                self.kind = event.name();
                self.reason = event.reason();
                self.fill_event(event, peers);
                if describe {
                    let mut text = detail.unwrap_or_default();
                    text.clear();
                    write!(text, "{event}").expect("writing to a string succeeds");
                    self.detail = Some(text);
                }
            }
        }
    }

    fn fill_event(&mut self, event: &EventKind, peers: &BTreeMap<PeerId, usize>) {
        use EventKind as E;
        let (conn, peer) = match event {
            E::Connected { conn, peer, .. }
            | E::Opened { conn, peer }
            | E::Closed { conn, peer, .. }
            | E::ManifestReceived { conn, peer }
            | E::Requested { conn, peer, .. }
            | E::Received { conn, peer, .. }
            | E::RequestRejected { conn, peer, .. }
            | E::Uploaded { conn, peer, .. }
            | E::ServingChanged { conn, peer, .. }
            | E::InFlightChanged { conn, peer, .. }
            | E::Cancelled { conn, peer, .. }
            | E::Snubbed { conn, peer }
            | E::Offered { conn, peer, .. }
            | E::PeersShared { conn, peer, .. } => (Some(conn.0), Some(peer)),
            E::DialFailed { peer } | E::Banned { peer } => (None, Some(peer)),
            E::Completed
            | E::EndgameStarted { .. }
            | E::SuperseedingEnded { .. }
            | E::StoppedSharing { .. } => (None, None),
        };
        self.conn = conn;
        self.other = peer.and_then(|p| peers.get(p)).map(|&i| i as u32);
        match *event {
            E::Requested { start, count, .. }
            | E::Cancelled { start, count, .. }
            | E::Offered { start, count, .. } => {
                self.start = Some(start);
                self.count = Some(count);
            }
            E::Received {
                start,
                count,
                bytes,
                ..
            }
            | E::Uploaded {
                start,
                count,
                bytes,
                ..
            } => {
                self.start = Some(start);
                self.count = Some(count);
                self.bytes = Some(bytes);
            }
            E::InFlightChanged { requests, .. } => self.count = Some(requests),
            E::EndgameStarted { missing } => self.count = Some(missing),
            E::PeersShared { count, .. } => self.count = Some(count),
            _ => {}
        }
    }

    /// Feeds the row's columns, except its position and detail, to `hasher` unambiguously.
    fn digest(&self, hasher: &mut Sha256) {
        hasher.update(self.time.as_nanos().to_le_bytes());
        hasher.update(self.peer.to_le_bytes());
        text(hasher, self.kind);
        number(hasher, self.other.map(u64::from));
        number(hasher, self.conn);
        name(hasher, self.message);
        number(hasher, self.start.map(u64::from));
        number(hasher, self.count.map(u64::from));
        number(hasher, self.bytes);
        name(hasher, self.reason);
    }
}

fn text(hasher: &mut Sha256, text: &str) {
    hasher.update((text.len() as u64).to_le_bytes());
    hasher.update(text.as_bytes());
}

fn number(hasher: &mut Sha256, value: Option<u64>) {
    match value {
        Some(value) => {
            hasher.update([1]);
            hasher.update(value.to_le_bytes());
        }
        None => hasher.update([0]),
    }
}

fn name(hasher: &mut Sha256, value: Option<&str>) {
    match value {
        Some(value) => {
            hasher.update([1]);
            text(hasher, value);
        }
        None => hasher.update([0]),
    }
}

/// Feeds what a decision event holds beyond its row's columns to `hasher`, so the digest covers
/// everything its detail says without formatting it. Every field is named, so a new one fails
/// to compile here until it is digested or deliberately skipped.
fn digest_details(hasher: &mut Sha256, event: &EventKind) {
    use EventKind as E;
    match event {
        E::Connected {
            conn: _,
            peer: _,
            direction,
            path,
        } => {
            let dialed = matches!(direction, Direction::Dialed);
            let relayed = matches!(path, PathKind::Relayed);
            hasher.update([u8::from(dialed), u8::from(relayed)]);
        }
        E::Closed {
            conn: _,
            peer: _,
            cause,
        } => match cause {
            CloseCause::Local { reason: _, message } => {
                hasher.update([0]);
                text(hasher, message);
            }
            CloseCause::Remote { reason: _, message } => {
                hasher.update([1]);
                text(hasher, message);
            }
            CloseCause::Lost => hasher.update([2]),
        },
        E::Requested {
            conn: _,
            peer: _,
            start: _,
            count: _,
            reason,
        } => match *reason {
            SelectReason::Rarest { holders: n }
            | SelectReason::NearPlayback { ahead: n }
            | SelectReason::Endgame { copies: n } => number(hasher, Some(u64::from(n))),
            SelectReason::RandomFirst | SelectReason::Random | SelectReason::Sequential => {
                number(hasher, None);
            }
        },
        E::Received {
            conn: _,
            peer: _,
            start: _,
            count: _,
            bytes: _,
            duplicate,
        } => number(hasher, Some(*duplicate)),
        E::ServingChanged {
            conn: _,
            peer: _,
            serving: _,
            reason,
        } => match *reason {
            ServeReason::Reciprocal { rate } => number(hasher, Some(rate)),
            ServeReason::Proportional { permille } => number(hasher, Some(u64::from(permille))),
            _ => number(hasher, None),
        },
        E::InFlightChanged {
            conn: _,
            peer: _,
            requests: _,
            reason,
        } => match *reason {
            InFlightReason::Measured { rate, rtt } => {
                number(hasher, Some(rate));
                number(hasher, Some(rtt.as_nanos() as u64));
            }
            _ => number(hasher, None),
        },
        E::PeersShared {
            conn: _,
            peer: _,
            count: _,
            farewell,
        } => hasher.update([u8::from(*farewell)]),
        E::Opened { .. }
        | E::DialFailed { .. }
        | E::ManifestReceived { .. }
        | E::RequestRejected { .. }
        | E::Uploaded { .. }
        | E::Banned { .. }
        | E::Completed
        | E::EndgameStarted { .. }
        | E::Cancelled { .. }
        | E::Snubbed { .. }
        | E::Offered { .. }
        | E::SuperseedingEnded { .. }
        | E::StoppedSharing { .. } => {}
    }
}

/// Receives a run's log as it is written.
pub trait Sink {
    /// Takes the next row.
    fn row(&mut self, row: &Row);
}

/// The log of one run.
pub(crate) struct Log<'s> {
    hasher: Sha256,
    row: Row,
    count: u64,
    entries: Option<Vec<Entry>>,
    sink: Option<&'s mut dyn Sink>,
}

impl<'s> Log<'s> {
    /// An empty log that keeps its entries only if `keep` is set, and passes its rows to
    /// `sink`.
    pub(crate) fn new(keep: bool, sink: Option<&'s mut dyn Sink>) -> Self {
        Self {
            hasher: Sha256::new(),
            row: Row::default(),
            count: 0,
            entries: keep.then(Vec::new),
            sink,
        }
    }

    /// Adds an entry, naming peers by their index in `peers`.
    pub(crate) fn push(&mut self, entry: Entry, peers: &BTreeMap<PeerId, usize>) {
        self.row
            .fill(self.count, &entry, peers, self.sink.is_some());
        self.row.digest(&mut self.hasher);
        if let Record::Decision(event) = &entry.record {
            digest_details(&mut self.hasher, event);
        }
        if let Some(sink) = &mut self.sink {
            sink.row(&self.row);
        }
        self.count += 1;
        if let Some(entries) = &mut self.entries {
            entries.push(entry);
        }
    }

    /// Ends the log: its digest, length and the entries kept.
    pub(crate) fn finish(self) -> (String, u64, Vec<Entry>) {
        let digest = self
            .hasher
            .finalize()
            .iter()
            .fold(String::new(), |mut hex, byte| {
                let _ = write!(hex, "{byte:02x}");
                hex
            });
        (digest, self.count, self.entries.unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use tempest_core::ConnId;
    use tempest_protocol::Hash;

    use super::*;

    fn received(duplicate: u64) -> Entry {
        Entry {
            time: Time::ZERO,
            peer: 0,
            record: Record::Decision(EventKind::Received {
                conn: ConnId(1),
                peer: Hash::new([7; 32]),
                start: 0,
                count: 1,
                bytes: 100,
                duplicate,
            }),
        }
    }

    fn digest(entry: Entry) -> String {
        let mut log = Log::new(false, None);
        log.push(entry, &BTreeMap::new());
        log.finish().0
    }

    #[test]
    fn a_log_without_a_sink_digests_details_it_never_formats() {
        let mut log = Log::new(false, None);
        log.push(received(0), &BTreeMap::new());
        assert_eq!(log.row.detail, None, "nothing reads the detail");
        // A field only the detail shows still changes the digest.
        assert_ne!(digest(received(0)), digest(received(5)));
    }

    #[test]
    fn a_sink_gets_each_decisions_detail() {
        struct Last(Option<String>);
        impl Sink for Last {
            fn row(&mut self, row: &Row) {
                self.0.clone_from(&row.detail);
            }
        }
        let mut last = Last(None);
        let mut log = Log::new(false, Some(&mut last));
        log.push(received(0), &BTreeMap::new());
        drop(log);
        assert!(last.0.is_some_and(|text| !text.is_empty()));
    }
}
