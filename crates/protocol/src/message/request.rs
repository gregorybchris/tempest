//! Request messages, sent on request streams (`protocol/messages.md`, Request messages).

use super::{MAX_PROOF_LEN, MAX_REASON_MESSAGE_LEN, MessageError, check_count, check_len};
use crate::hash::Hash;
use crate::manifest::MAX_MANIFEST_LEN;
use crate::wire::{DecodeError, Reader, Writer};

/// Asks for the aligned range of chunks `[start, start + count)`.
///
/// Any values decode. Whether they name one node of one file's chunk tree depends on the
/// manifest and the provider's limits; see [`ChunkLayout::request_node`]. A provider answers an
/// invalid Request with Reject ([`RejectReason::Invalid`]) rather than treating it as a protocol
/// violation.
///
/// [`ChunkLayout::request_node`]: crate::layout::ChunkLayout::request_node
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    /// The first chunk index.
    pub start: u32,
    /// The number of chunks: a power of two.
    pub count: u32,
}

impl Request {
    pub(super) fn encode_body(&self, w: &mut Writer) {
        w.u32(self.start);
        w.u32(self.count);
    }

    pub(super) fn decode_body(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            start: r.u32()?,
            count: r.u32()?,
        })
    }
}

/// Chunks and their proof, answering a Request.
///
/// Verify it against the manifest with [`verify_data`](crate::tree::verify_data).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Data {
    /// Sibling hashes from the range's node up to the root, lowest first: at most 32.
    pub proof: Vec<Hash>,
    /// The requested chunks that exist, concatenated.
    pub chunks: Vec<u8>,
}

impl Data {
    pub(super) fn validate(&self) -> Result<(), MessageError> {
        check_count("proof", self.proof.len(), MAX_PROOF_LEN as usize)
    }

    pub(super) fn encode_body(&self, w: &mut Writer) {
        w.list(&self.proof);
        w.bytes(&self.chunks);
    }

    pub(super) fn decode_body(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            proof: r.list()?,
            chunks: r.bytes()?.to_vec(),
        })
    }
}

/// Refuses a Request or ManifestRequest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reject {
    /// Why the request was refused.
    pub reason: RejectReason,
    /// Human-readable detail. May be empty; at most 1 KiB.
    pub message: String,
}

impl Reject {
    pub(super) fn validate(&self) -> Result<(), MessageError> {
        check_len(
            "message",
            self.message.len(),
            MAX_REASON_MESSAGE_LEN as usize,
        )
    }

    pub(super) fn encode_body(&self, w: &mut Writer) {
        w.u16(self.reason.code());
        w.string(&self.message);
    }

    pub(super) fn decode_body(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            reason: RejectReason::from_code(r.u16()?),
            message: r.string()?.to_owned(),
        })
    }
}

/// Why a provider refused a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum RejectReason {
    /// The provider isn't serving the requester right now, or the requester has too many open
    /// requests.
    Busy = 0,
    /// The provider doesn't have (all of) the requested chunks, or the manifest.
    DontHave = 1,
    /// The request breaks a rule in the specification.
    Invalid = 2,
}

impl RejectReason {
    /// The reason with wire code `code`. An unknown code means [`RejectReason::Busy`].
    pub fn from_code(code: u16) -> Self {
        match code {
            1 => Self::DontHave,
            2 => Self::Invalid,
            _ => Self::Busy,
        }
    }

    /// The reason's wire code.
    pub fn code(self) -> u16 {
        self as u16
    }
}

/// The Manifest message: the encoded manifest, answering a ManifestRequest.
///
/// The bytes are kept as received. Before decoding them, the requester must check that they hash
/// to the bundle ID ([`manifest_hash`](crate::hash::manifest_hash)).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ManifestMessage {
    /// The encoded manifest: at most 64 MiB.
    pub manifest: Vec<u8>,
}

impl ManifestMessage {
    pub(super) fn validate(&self) -> Result<(), MessageError> {
        check_len("manifest", self.manifest.len(), MAX_MANIFEST_LEN)
    }

    pub(super) fn encode_body(&self, w: &mut Writer) {
        w.bytes(&self.manifest);
    }

    pub(super) fn decode_body(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            manifest: r.bytes()?.to_vec(),
        })
    }
}
