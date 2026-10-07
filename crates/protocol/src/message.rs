//! Peer message types and their frame size limits (`protocol/messages.md`).

use crate::frame::{self, Frame};
use crate::hash::HASH_LEN;
use crate::tree::CHUNK_SIZE;
use crate::wire::DecodeError;

/// The largest frame length of any control message: 16 MiB.
pub const MAX_CONTROL_FRAME: u32 = 16 * 1024 * 1024;

/// The largest manifest: 64 MiB.
pub const MAX_MANIFEST_LEN: u32 = 64 * 1024 * 1024;

/// The longest Reject or Goodbye message text: 1 KiB.
pub const MAX_REASON_MESSAGE_LEN: u32 = 1024;

/// The largest `count` a Request may have, and so the largest `max_request_chunks`.
pub const MAX_REQUEST_CHUNKS: u32 = 1024;

/// The most hashes a proof can have: one per level of the tallest possible tree.
pub const MAX_PROOF_LEN: u32 = 32;

/// A peer message type, with its code on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum MessageType {
    /// Opens a connection, naming the bundle.
    Hello = 0x01,
    /// Every chunk the sender has.
    Have = 0x02,
    /// Chunks the sender gained or lost.
    HaveUpdate = 0x03,
    /// Whether the sender wants chunks the receiver has.
    Want = 0x04,
    /// Whether the sender intends to answer the receiver's Requests.
    Serving = 0x05,
    /// Other peers in the swarm.
    Peers = 0x06,
    /// Addresses for upgrading a relayed connection to a direct one.
    Candidates = 0x08,
    /// Asks for one aligned node of a file's chunk tree.
    Request = 0x10,
    /// Chunks and their proof, answering a Request.
    Data = 0x11,
    /// Refuses a Request or ManifestRequest.
    Reject = 0x12,
    /// Asks for the manifest.
    ManifestRequest = 0x13,
    /// The encoded manifest, answering a ManifestRequest.
    Manifest = 0x14,
}

impl MessageType {
    /// The message type with wire code `code`, if there is one. Code `0x07` is retired.
    pub fn from_code(code: u8) -> Option<Self> {
        use MessageType::*;
        Some(match code {
            0x01 => Hello,
            0x02 => Have,
            0x03 => HaveUpdate,
            0x04 => Want,
            0x05 => Serving,
            0x06 => Peers,
            0x08 => Candidates,
            0x10 => Request,
            0x11 => Data,
            0x12 => Reject,
            0x13 => ManifestRequest,
            0x14 => Manifest,
            _ => return None,
        })
    }

    /// The type's wire code.
    pub fn code(self) -> u8 {
        self as u8
    }

    /// Whether the message travels on the control stream (rather than a request stream).
    pub fn is_control(self) -> bool {
        self.code() < 0x10
    }

    /// The largest allowed frame `length` (type byte plus body) for this type.
    ///
    /// For Data this allows the largest possible Request. A requester that knows the `count` it
    /// asked for can apply the tighter [`max_data_frame_length`].
    pub fn max_frame_length(self) -> u32 {
        match self {
            Self::Data => max_data_frame_length(MAX_REQUEST_CHUNKS),
            Self::Manifest => 1 + 4 + MAX_MANIFEST_LEN,
            Self::Reject => 1 + 2 + 4 + MAX_REASON_MESSAGE_LEN,
            Self::Request => 1 + 4 + 4,
            Self::ManifestRequest => 1,
            _ => MAX_CONTROL_FRAME,
        }
    }
}

/// The largest frame `length` of Data answering a Request for `count` chunks.
pub const fn max_data_frame_length(count: u32) -> u32 {
    1 + 4 + MAX_PROOF_LEN * HASH_LEN as u32 + 4 + count * CHUNK_SIZE as u32
}

/// Reads one peer message frame from the front of `buf`, applying the size limit for its type.
///
/// See [`frame::split_frame`] for the return value.
pub fn split_message_frame(buf: &[u8]) -> Result<Option<(Frame<'_>, usize)>, DecodeError> {
    frame::split_frame(buf, |code| {
        MessageType::from_code(code)
            .map(MessageType::max_frame_length)
            .ok_or(DecodeError::UnknownMessageType(code))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip() {
        for code in 0..=u8::MAX {
            if let Some(t) = MessageType::from_code(code) {
                assert_eq!(t.code(), code);
            }
        }
        assert_eq!(MessageType::from_code(0x07), None);
    }

    #[test]
    fn limits_match_the_spec() {
        assert_eq!(MessageType::Hello.max_frame_length(), 16 << 20);
        assert_eq!(
            MessageType::Data.max_frame_length(),
            1 + 4 + 1024 + 4 + (16 << 20)
        );
        assert_eq!(MessageType::Manifest.max_frame_length(), 5 + (64 << 20));
        assert_eq!(MessageType::Reject.max_frame_length(), 1031);
        assert_eq!(MessageType::Request.max_frame_length(), 9);
        assert_eq!(MessageType::ManifestRequest.max_frame_length(), 1);
    }

    #[test]
    fn oversized_request_frame_is_rejected() {
        assert_eq!(
            split_message_frame(&[0, 0, 0, 10, 0x10]),
            Err(DecodeError::FrameTooLong { length: 10, max: 9 })
        );
    }
}
