//! What a driver tells a [`Session`](crate::Session), and what the session asks the driver to do.
//!
//! The session never sees chunk bytes. When it answers a Request, it asks the driver to send Data
//! for a range, and the driver reads the chunks and proof from its storage. When Data arrives, the
//! driver verifies it (with [`Session::verify_data`](crate::Session::verify_data) or its own
//! check), stores the chunks, and reports only the outcome. This keeps storage I/O in the driver,
//! and lets the simulator model large bundles without materializing their bytes.

use std::fmt;
use std::sync::Arc;

use tempest_protocol::message::{GoodbyeReason, ManifestMessage, PeerRecord, Reject};
use tempest_protocol::tree::VerifyError;
use tempest_protocol::{Address, Manifest, Message, PeerId};

/// A connection, named by the driver. IDs must not be reused within a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConnId(pub u64);

/// A request stream the other peer opened, named by the driver. IDs must be unique per
/// connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StreamId(pub u64);

/// A request stream this peer opened, named by the session. IDs are unique within a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RequestId(pub u64);

impl fmt::Display for ConnId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "c{}", self.0)
    }
}

/// Which side opened a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// This peer dialed: it speaks first.
    Dialed,
    /// The other peer dialed.
    Accepted,
}

/// How a connection reaches the other peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PathKind {
    /// Directly.
    Direct,
    /// Through a relay.
    Relayed,
}

/// Something that arrived on an open connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inbound {
    /// A message on the control stream.
    Control(Message),
    /// The first message on a request stream the other peer opened. Anything but a Request or
    /// ManifestRequest is a protocol violation.
    Request {
        /// The stream.
        stream: StreamId,
        /// The message.
        message: Message,
    },
    /// The response on a request stream this peer opened.
    Response {
        /// The request it answers.
        request: RequestId,
        /// The response.
        response: Response,
    },
    /// The other peer reset a request stream it opened: it no longer wants an answer.
    RequestReset {
        /// The stream.
        stream: StreamId,
    },
    /// The driver has finished sending the Data the session asked for on a stream.
    DataSent {
        /// The stream.
        stream: StreamId,
    },
}

/// A response to one of this peer's requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    /// Data, with the outcome of verifying it. The driver keeps the chunks only if it passed.
    ///
    /// Data answering a ManifestRequest is a protocol violation whatever the outcome.
    Data(Result<(), VerifyError>),
    /// A Reject.
    Reject(Reject),
    /// A Manifest. The session checks it against the bundle ID.
    Manifest(ManifestMessage),
    /// Any other message, which is a protocol violation.
    Other(Message),
}

/// A request this peer sends on a new request stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutRequest {
    /// A Request for the aligned chunks `[start, start + count)`.
    Chunks {
        /// The first chunk.
        start: u32,
        /// The number of chunks.
        count: u32,
    },
    /// A ManifestRequest.
    Manifest,
}

impl OutRequest {
    /// The message to send.
    pub fn message(self) -> Message {
        match self {
            Self::Chunks { start, count } => {
                Message::Request(tempest_protocol::message::Request { start, count })
            }
            Self::Manifest => Message::ManifestRequest,
        }
    }
}

/// The response this peer sends on a request stream the other peer opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// Data for the aligned chunks `[start, start + count)`, built by the driver from storage.
    /// The driver reports [`Inbound::DataSent`] once it has sent it.
    Data {
        /// The first chunk.
        start: u32,
        /// The number of chunks requested.
        count: u32,
    },
    /// The Manifest, built by the driver from storage.
    Manifest,
    /// A Reject.
    Reject(Reject),
}

/// Something the session asks its driver to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Connect to a peer for this bundle, then report [`Session::handle_connected`] or
    /// [`Session::handle_dial_failed`].
    ///
    /// [`Session::handle_connected`]: crate::Session::handle_connected
    /// [`Session::handle_dial_failed`]: crate::Session::handle_dial_failed
    Dial(PeerRecord),
    /// Send a message on a connection's control stream.
    Send {
        /// The connection.
        conn: ConnId,
        /// The message.
        message: Message,
    },
    /// Open a request stream, send the request and finish the stream's sending side.
    Request {
        /// The connection.
        conn: ConnId,
        /// The session's name for the stream.
        request: RequestId,
        /// What to send.
        message: OutRequest,
    },
    /// Reset a request stream this peer opened. The session expects no response on it.
    Cancel {
        /// The connection.
        conn: ConnId,
        /// The stream.
        request: RequestId,
    },
    /// Send a response on a request stream the other peer opened, then finish the stream.
    Respond {
        /// The connection.
        conn: ConnId,
        /// The stream.
        stream: StreamId,
        /// What to send.
        reply: Reply,
    },
    /// Close a connection with a Goodbye. The session has already forgotten it.
    Close {
        /// The connection.
        conn: ConnId,
        /// The reason code.
        reason: GoodbyeReason,
        /// A human-readable reason phrase.
        message: String,
    },
    /// Dial `peer` directly at each of `addresses` for a few seconds, to replace the relayed
    /// connection `conn` (`protocol/relays.md`, Upgrading to a direct connection). Each dial
    /// that succeeds is reported with [`Session::handle_connected`]; failures are not reported.
    ///
    /// [`Session::handle_connected`]: crate::Session::handle_connected
    Punch {
        /// The relayed connection, through which the driver may know another address to dial.
        conn: ConnId,
        /// The other peer.
        peer: PeerId,
        /// The other peer's candidate addresses.
        addresses: Vec<Address>,
    },
    /// Stop starting dials for a [`Action::Punch`] with `peer`: a direct connection has opened.
    StopPunching {
        /// The other peer.
        peer: PeerId,
    },
    /// The manifest arrived and matched the bundle ID: store it. Data can be verified from now on.
    StoreManifest {
        /// The manifest.
        manifest: Arc<Manifest>,
        /// Its encoding, as received.
        bytes: Arc<[u8]>,
    },
}

/// Who a connection is with, as the driver established it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnInfo {
    /// The other peer's ID, from its TLS raw public key.
    pub peer: PeerId,
    /// Which side dialed.
    pub direction: Direction,
    /// Whether the connection is direct or relayed.
    pub path: PathKind,
}

/// Why a connection ended without this session closing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Closed {
    /// The other peer sent a Goodbye.
    Goodbye {
        /// The reason code.
        reason: GoodbyeReason,
        /// The reason phrase.
        message: String,
    },
    /// The connection was lost, for example by timing out.
    Lost,
}
