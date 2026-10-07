//! The event log of a run: every peer arriving and leaving, every message sent, and every
//! decision event (D-006), in the order they happened.
//!
//! The log is digested as it grows, so two runs can be compared without keeping either log:
//! the same seed must give the same digest (D-068).

use std::fmt::Write as _;

use sha2::{Digest, Sha256};
use tempest_core::{EventKind, Time};
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

/// The log of one run.
#[derive(Debug)]
pub struct Log {
    hasher: Sha256,
    line: String,
    count: u64,
    entries: Option<Vec<Entry>>,
}

impl Log {
    /// An empty log that keeps its entries only if `keep` is set.
    pub fn new(keep: bool) -> Self {
        Self {
            hasher: Sha256::new(),
            line: String::new(),
            count: 0,
            entries: keep.then(Vec::new),
        }
    }

    /// Adds an entry.
    pub fn push(&mut self, entry: Entry) {
        self.line.clear();
        writeln!(self.line, "{entry:?}").expect("writing to a string succeeds");
        self.hasher.update(self.line.as_bytes());
        self.count += 1;
        if let Some(entries) = &mut self.entries {
            entries.push(entry);
        }
    }

    /// Ends the log: its digest, length and the entries kept.
    pub fn finish(self) -> (String, u64, Vec<Entry>) {
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
