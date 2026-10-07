//! Control messages, sent on the control stream (`protocol/messages.md`, Control messages).

use super::{MAX_CANDIDATES, MAX_PEER_RECORDS, MAX_REQUEST_CHUNKS, MessageError, check_count};
use crate::address::Address;
use crate::hash::PeerId;
use crate::manifest::BundleId;
use crate::range::RangeList;
use crate::wire::{Decode, DecodeError, Encode, Reader, Writer};

/// Opens a connection: names the bundle and describes the sender.
///
/// ```text
/// Hello {
///   bundle_id:          hash
///   capabilities:       list<string>
///   client:             string
///   max_request_chunks: u32
///   max_open_requests:  u32
///   addresses:          list<Address>
///   relay:              list<Address>   // zero or one entries
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    /// The bundle this connection is about.
    pub bundle_id: BundleId,
    /// Optional features the sender supports. This version defines none.
    pub capabilities: Vec<String>,
    /// The sender's client name and version. May be empty.
    pub client: String,
    /// The largest Request `count` the sender accepts: a power of two from 1 to 1,024.
    pub max_request_chunks: u32,
    /// The most Requests the sender accepts open at once: at least 1.
    pub max_open_requests: u32,
    /// Addresses at which the sender accepts connections. May be empty.
    pub addresses: Vec<Address>,
    /// The relay address lent to the sender by its home relay, if it has one.
    pub relay: Option<Address>,
}

impl Hello {
    pub(super) fn validate(&self) -> Result<(), MessageError> {
        let chunks = self.max_request_chunks;
        if !chunks.is_power_of_two() || chunks > MAX_REQUEST_CHUNKS {
            return Err(MessageError::InvalidMaxRequestChunks(chunks));
        }
        if self.max_open_requests == 0 {
            return Err(MessageError::ZeroMaxOpenRequests);
        }
        Ok(())
    }

    pub(super) fn encode_body(&self, w: &mut Writer) {
        w.hash(&self.bundle_id);
        w.list(&self.capabilities);
        w.string(&self.client);
        w.u32(self.max_request_chunks);
        w.u32(self.max_open_requests);
        w.list(&self.addresses);
        encode_relay(w, self.relay);
    }

    /// Reads Hello's known fields, leaving any bytes after them unread.
    pub(super) fn decode_body(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            bundle_id: r.hash()?,
            capabilities: r.list()?,
            client: r.string()?.to_owned(),
            max_request_chunks: r.u32()?,
            max_open_requests: r.u32()?,
            addresses: r.list()?,
            relay: decode_relay(r)?,
        })
    }
}

/// Every chunk the sender has and is willing to provide. Sent once, in the opening sequence.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Have {
    /// The chunks.
    pub ranges: RangeList,
}

impl Have {
    pub(super) fn encode_body(&self, w: &mut Writer) {
        self.ranges.encode(w);
    }

    pub(super) fn decode_body(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            ranges: RangeList::decode(r)?,
        })
    }
}

/// A change to the set of chunks described by the sender's Have and earlier HaveUpdates.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HaveUpdate {
    /// Chunks the sender now has.
    pub added: RangeList,
    /// Chunks the sender no longer has.
    pub removed: RangeList,
}

impl HaveUpdate {
    pub(super) fn encode_body(&self, w: &mut Writer) {
        self.added.encode(w);
        self.removed.encode(w);
    }

    pub(super) fn decode_body(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            added: RangeList::decode(r)?,
            removed: RangeList::decode(r)?,
        })
    }
}

/// Whether the sender wants chunks the receiver has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Want {
    /// Yes: you have chunks I'd like to request.
    pub wanted: bool,
}

impl Want {
    pub(super) fn encode_body(&self, w: &mut Writer) {
        w.bool(self.wanted);
    }

    pub(super) fn decode_body(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self { wanted: r.bool()? })
    }
}

/// Whether the sender intends to answer the receiver's Requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Serving {
    /// Yes: if you send Requests now, I intend to answer them with Data.
    pub serving: bool,
}

impl Serving {
    pub(super) fn encode_body(&self, w: &mut Writer) {
        w.bool(self.serving);
    }

    pub(super) fn decode_body(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self { serving: r.bool()? })
    }
}

/// Other peers in the swarm: at most 50 records.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Peers {
    /// The peers.
    pub peers: Vec<PeerRecord>,
}

impl Peers {
    pub(super) fn validate(&self) -> Result<(), MessageError> {
        check_count("peers", self.peers.len(), MAX_PEER_RECORDS)
    }

    pub(super) fn encode_body(&self, w: &mut Writer) {
        w.list(&self.peers);
    }

    pub(super) fn decode_body(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self { peers: r.list()? })
    }
}

/// One peer in a Peers message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRecord {
    /// The peer's Ed25519 public key.
    pub peer_id: PeerId,
    /// The peer's direct addresses. May be empty, and never holds a relay address.
    pub addresses: Vec<Address>,
    /// The peer's relay address, if it has one.
    pub relay: Option<Address>,
}

impl Encode for PeerRecord {
    fn encode(&self, w: &mut Writer) {
        w.hash(&self.peer_id);
        w.list(&self.addresses);
        encode_relay(w, self.relay);
    }
}

impl Decode for PeerRecord {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            peer_id: r.hash()?,
            addresses: r.list()?,
            relay: decode_relay(r)?,
        })
    }
}

/// Addresses at which the sender might be reached directly: at most 16.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Candidates {
    /// The addresses.
    pub addresses: Vec<Address>,
}

impl Candidates {
    pub(super) fn validate(&self) -> Result<(), MessageError> {
        check_count("addresses", self.addresses.len(), MAX_CANDIDATES)
    }

    pub(super) fn encode_body(&self, w: &mut Writer) {
        w.list(&self.addresses);
    }

    pub(super) fn decode_body(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            addresses: r.list()?,
        })
    }
}

/// Encodes a relay address as a `list<Address>` of zero or one entries.
fn encode_relay(w: &mut Writer, relay: Option<Address>) {
    w.list(relay.as_slice());
}

/// Decodes a `list<Address>` that must have zero or one entries.
fn decode_relay(r: &mut Reader<'_>) -> Result<Option<Address>, DecodeError> {
    let mut relay: Vec<Address> = r.list()?;
    check_count("relay", relay.len(), 1)?;
    Ok(relay.pop())
}
