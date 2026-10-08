//! Peer messages: their types, size limits, encoding and validation (`protocol/messages.md`).
//!
//! A [`Message`] is encoded into a frame with [`Message::encode_frame`]. A receiver splits frames
//! from a stream with [`split_message_frame`] and decodes each with [`Message::decode`], which
//! rejects anything the specification calls a protocol violation.

mod control;
mod goodbye;
mod request;

pub use control::{Candidates, Have, HaveUpdate, Hello, PeerRecord, Peers, Serving, Want};
pub use goodbye::GoodbyeReason;
pub use request::{Data, ManifestMessage, Reject, RejectReason, Request};

use crate::frame::{self, Frame};
use crate::hash::HASH_LEN;
use crate::manifest::MAX_MANIFEST_LEN;
use crate::tree::CHUNK_SIZE;
use crate::wire::{Decode, DecodeError, Reader};

/// The largest frame length of any control message: 16 MiB.
pub const MAX_CONTROL_FRAME: u32 = 16 * 1024 * 1024;

/// The longest Reject or Goodbye message text: 1 KiB.
pub const MAX_REASON_MESSAGE_LEN: u32 = 1024;

/// The largest `count` a Request may have, and so the largest `max_request_chunks`.
pub const MAX_REQUEST_CHUNKS: u32 = 1024;

/// The most hashes a proof can have: one per level of the tallest possible tree.
pub const MAX_PROOF_LEN: u32 = 32;

/// The most PeerRecords one Peers message may carry.
pub const MAX_PEER_RECORDS: usize = 50;

/// The most addresses one Candidates message may carry.
pub const MAX_CANDIDATES: usize = 16;

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
            Self::Manifest => 1 + 4 + MAX_MANIFEST_LEN as u32,
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

/// A peer message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// Opens a connection, naming the bundle.
    Hello(Hello),
    /// Every chunk the sender has.
    Have(Have),
    /// Chunks the sender gained or lost.
    HaveUpdate(HaveUpdate),
    /// Whether the sender wants chunks the receiver has.
    Want(Want),
    /// Whether the sender intends to answer the receiver's Requests.
    Serving(Serving),
    /// Other peers in the swarm.
    Peers(Peers),
    /// Addresses for upgrading a relayed connection to a direct one.
    Candidates(Candidates),
    /// Asks for one aligned node of a file's chunk tree.
    Request(Request),
    /// Chunks and their proof, answering a Request.
    Data(Data),
    /// Refuses a Request or ManifestRequest.
    Reject(Reject),
    /// Asks for the manifest. It has no fields.
    ManifestRequest,
    /// The encoded manifest, answering a ManifestRequest.
    Manifest(ManifestMessage),
}

/// A message that is well-formed but breaks one of its rules. Every variant is a protocol
/// violation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MessageError {
    /// Hello's `max_request_chunks` was not a power of two from 1 to 1,024.
    #[error("max_request_chunks {0} is not a power of two from 1 to {MAX_REQUEST_CHUNKS}")]
    InvalidMaxRequestChunks(u32),
    /// Hello's `max_open_requests` was 0.
    #[error("max_open_requests is 0")]
    ZeroMaxOpenRequests,
    /// A list had more entries than its limit.
    #[error("{field} has {count} entries, more than the limit of {max}")]
    TooMany {
        /// The field's name in the specification.
        field: &'static str,
        /// The number of entries.
        count: usize,
        /// The limit.
        max: usize,
    },
    /// A string or byte field was longer than its limit.
    #[error("{field} is {len} bytes, more than the limit of {max}")]
    TooLong {
        /// The field's name in the specification.
        field: &'static str,
        /// The field's length in bytes.
        len: usize,
        /// The limit.
        max: usize,
    },
}

/// Fails with [`MessageError::TooMany`] if `count` exceeds `max`.
fn check_count(field: &'static str, count: usize, max: usize) -> Result<(), MessageError> {
    if count > max {
        return Err(MessageError::TooMany { field, count, max });
    }
    Ok(())
}

/// Reads a `list<T>` that may have at most `max` entries, failing on the count before decoding
/// any of them.
fn list_at_most<T: Decode>(
    r: &mut Reader<'_>,
    field: &'static str,
    max: usize,
) -> Result<Vec<T>, DecodeError> {
    let count = r.clone().u32()?;
    check_count(field, count as usize, max)?;
    r.list()
}

/// Fails with [`MessageError::TooLong`] if `len` exceeds `max`.
pub(crate) fn check_len(field: &'static str, len: usize, max: usize) -> Result<(), MessageError> {
    if len > max {
        return Err(MessageError::TooLong { field, len, max });
    }
    Ok(())
}

impl Message {
    /// The message's type.
    pub fn message_type(&self) -> MessageType {
        match self {
            Self::Hello(_) => MessageType::Hello,
            Self::Have(_) => MessageType::Have,
            Self::HaveUpdate(_) => MessageType::HaveUpdate,
            Self::Want(_) => MessageType::Want,
            Self::Serving(_) => MessageType::Serving,
            Self::Peers(_) => MessageType::Peers,
            Self::Candidates(_) => MessageType::Candidates,
            Self::Request(_) => MessageType::Request,
            Self::Data(_) => MessageType::Data,
            Self::Reject(_) => MessageType::Reject,
            Self::ManifestRequest => MessageType::ManifestRequest,
            Self::Manifest(_) => MessageType::Manifest,
        }
    }

    /// Checks the message's rules that its encoding alone doesn't enforce: the limits on its
    /// lists and strings, and Hello's request limits.
    ///
    /// [`Message::decode`] applies these checks. Encoding doesn't, so a sender that builds
    /// messages from untrusted values should call this first.
    pub fn validate(&self) -> Result<(), MessageError> {
        match self {
            Self::Hello(m) => m.validate(),
            Self::Peers(m) => m.validate(),
            Self::Candidates(m) => m.validate(),
            Self::Data(m) => m.validate(),
            Self::Reject(m) => m.validate(),
            Self::Manifest(m) => m.validate(),
            Self::Have(_)
            | Self::HaveUpdate(_)
            | Self::Want(_)
            | Self::Serving(_)
            | Self::Request(_)
            | Self::ManifestRequest => Ok(()),
        }
    }

    /// Encodes the message as a frame, ready to send on a stream.
    ///
    /// # Panics
    ///
    /// If the frame would be 4 GiB or longer, which no valid message is.
    pub fn encode_frame(&self) -> Vec<u8> {
        // Room for the bulk of the large messages, so their bytes are copied once.
        let body_len = match self {
            Self::Data(m) => 4 + m.proof.len() * HASH_LEN + 4 + m.chunks.len(),
            Self::Manifest(m) => 4 + m.manifest.len(),
            _ => 64,
        };
        frame::encode_frame_with(self.message_type().code(), body_len, |w| match self {
            Self::Hello(m) => m.encode_body(w),
            Self::Have(m) => m.encode_body(w),
            Self::HaveUpdate(m) => m.encode_body(w),
            Self::Want(m) => m.encode_body(w),
            Self::Serving(m) => m.encode_body(w),
            Self::Peers(m) => m.encode_body(w),
            Self::Candidates(m) => m.encode_body(w),
            Self::Request(m) => m.encode_body(w),
            Self::Data(m) => m.encode_body(w),
            Self::Reject(m) => m.encode_body(w),
            Self::ManifestRequest => {}
            Self::Manifest(m) => m.encode_body(w),
        })
    }

    /// Decodes a frame's message, rejecting anything that is a protocol violation.
    ///
    /// The frame's type must be known and its length within the type's limit. Its body must hold
    /// exactly the type's fields, except that bytes after Hello's known fields are ignored. The
    /// message must then pass [`Message::validate`].
    ///
    /// Rules that need more context, such as whether ranges lie within the bundle or a Request
    /// names a node of a file's tree, are left to the caller.
    pub fn decode(frame: Frame<'_>) -> Result<Self, DecodeError> {
        let message_type = MessageType::from_code(frame.message_type)
            .ok_or(DecodeError::UnknownMessageType(frame.message_type))?;
        let max = message_type.max_frame_length();
        let length = frame.body.len() + 1;
        if length > max as usize {
            return Err(DecodeError::FrameTooLong {
                length: u32::try_from(length).unwrap_or(u32::MAX),
                max,
            });
        }

        let mut r = Reader::new(frame.body);
        let message = match message_type {
            MessageType::Hello => Self::Hello(Hello::decode_body(&mut r)?),
            MessageType::Have => Self::Have(Have::decode_body(&mut r)?),
            MessageType::HaveUpdate => Self::HaveUpdate(HaveUpdate::decode_body(&mut r)?),
            MessageType::Want => Self::Want(Want::decode_body(&mut r)?),
            MessageType::Serving => Self::Serving(Serving::decode_body(&mut r)?),
            MessageType::Peers => Self::Peers(Peers::decode_body(&mut r)?),
            MessageType::Candidates => Self::Candidates(Candidates::decode_body(&mut r)?),
            MessageType::Request => Self::Request(Request::decode_body(&mut r)?),
            MessageType::Data => Self::Data(Data::decode_body(&mut r)?),
            MessageType::Reject => Self::Reject(Reject::decode_body(&mut r)?),
            MessageType::ManifestRequest => Self::ManifestRequest,
            MessageType::Manifest => Self::Manifest(ManifestMessage::decode_body(&mut r)?),
        };
        // Future versions may add fields to Hello, so its tail is ignored.
        if message_type != MessageType::Hello {
            r.finish()?;
        }
        message.validate()?;
        Ok(message)
    }
}

macro_rules! impl_from {
    ($($variant:ident($ty:ty)),* $(,)?) => {
        $(impl From<$ty> for Message {
            fn from(m: $ty) -> Self {
                Self::$variant(m)
            }
        })*
    };
}

impl_from!(
    Hello(Hello),
    Have(Have),
    HaveUpdate(HaveUpdate),
    Want(Want),
    Serving(Serving),
    Peers(Peers),
    Candidates(Candidates),
    Request(Request),
    Data(Data),
    Reject(Reject),
    Manifest(ManifestMessage),
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::address::Address;
    use crate::hash::Hash;
    use crate::range::RangeList;
    use crate::wire::{Encode, Writer};

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

    fn addr(s: &str) -> Address {
        s.parse::<std::net::SocketAddr>().unwrap().into()
    }

    fn hello() -> Hello {
        Hello {
            bundle_id: Hash::new([7; 32]),
            capabilities: vec!["x".into()],
            client: "test".into(),
            max_request_chunks: 64,
            max_open_requests: 8,
            addresses: vec![addr("203.0.113.5:4433")],
            relay: Some(addr("[2001:db8::1]:9")),
        }
    }

    /// One message of every type.
    fn samples() -> Vec<Message> {
        vec![
            hello().into(),
            Have {
                ranges: RangeList::from_chunks([0, 1, 5]),
            }
            .into(),
            HaveUpdate {
                added: RangeList::from_chunks([3]),
                removed: RangeList::from_chunks([5]),
            }
            .into(),
            Want { wanted: true }.into(),
            Serving { serving: false }.into(),
            Peers {
                peers: vec![PeerRecord {
                    peer_id: Hash::new([1; 32]),
                    addresses: vec![addr("198.51.100.1:1")],
                    relay: None,
                }],
            }
            .into(),
            Candidates {
                addresses: vec![addr("198.51.100.2:2")],
            }
            .into(),
            Request { start: 4, count: 2 }.into(),
            Data {
                proof: vec![Hash::ZERO],
                chunks: vec![1, 2, 3],
            }
            .into(),
            Reject {
                reason: RejectReason::DontHave,
                message: "no".into(),
            }
            .into(),
            Message::ManifestRequest,
            ManifestMessage {
                manifest: vec![1, 2],
            }
            .into(),
        ]
    }

    fn decode_bytes(bytes: &[u8]) -> Result<Message, DecodeError> {
        let (frame, used) = split_message_frame(bytes)?.expect("a whole frame");
        assert_eq!(used, bytes.len());
        Message::decode(frame)
    }

    /// Re-frames `bytes`' body after applying `edit`.
    fn reframe(bytes: &[u8], edit: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
        let mut body = bytes[frame::HEADER_LEN..].to_vec();
        edit(&mut body);
        frame::encode_frame(bytes[4], &body)
    }

    fn invalid(error: MessageError) -> Result<Message, DecodeError> {
        Err(DecodeError::InvalidMessage(error))
    }

    #[test]
    fn every_type_round_trips() {
        for message in samples() {
            let bytes = message.encode_frame();
            assert_eq!(bytes[4], message.message_type().code());
            assert_eq!(decode_bytes(&bytes), Ok(message));
        }
    }

    #[test]
    fn short_and_long_bodies_are_rejected_except_hello_tails() {
        for message in samples() {
            let bytes = message.encode_frame();
            let long = reframe(&bytes, |body| body.push(0));
            if let Message::Hello(_) = message {
                assert_eq!(decode_bytes(&long), Ok(message));
                continue;
            }
            if message.message_type().max_frame_length() as usize >= long.len() - 4 {
                assert_eq!(decode_bytes(&long), Err(DecodeError::TrailingBytes(1)));
            }
            if bytes.len() > frame::HEADER_LEN {
                let short = reframe(&bytes, |body| {
                    body.pop();
                });
                assert!(decode_bytes(&short).is_err(), "{message:?}");
            }
        }
    }

    #[test]
    fn decode_checks_the_frame_limit() {
        let frame = Frame {
            message_type: MessageType::Request.code(),
            body: &[0; 9],
        };
        assert_eq!(
            Message::decode(frame),
            Err(DecodeError::FrameTooLong { length: 10, max: 9 })
        );
    }

    #[test]
    fn hello_request_limits_are_checked() {
        for chunks in [1, 2, 512, 1024] {
            let m = Hello {
                max_request_chunks: chunks,
                ..hello()
            };
            assert_eq!(Message::from(m).validate(), Ok(()));
        }
        for chunks in [0, 3, 100, 2048, u32::MAX] {
            let m = Hello {
                max_request_chunks: chunks,
                ..hello()
            };
            let bytes = Message::from(m).encode_frame();
            assert_eq!(
                decode_bytes(&bytes),
                invalid(MessageError::InvalidMaxRequestChunks(chunks))
            );
        }
        let m = Hello {
            max_open_requests: 0,
            ..hello()
        };
        let bytes = Message::from(m).encode_frame();
        assert_eq!(
            decode_bytes(&bytes),
            invalid(MessageError::ZeroMaxOpenRequests)
        );
    }

    /// Encodes a Hello or PeerRecord's fields up to `relay`, then a relay list of `n` entries.
    fn with_relays(prefix: impl FnOnce(&mut Writer), n: u32) -> Vec<u8> {
        let mut w = Writer::new();
        prefix(&mut w);
        w.u32(n);
        for _ in 0..n {
            addr("192.0.2.1:1").encode(&mut w);
        }
        w.into_bytes()
    }

    #[test]
    fn at_most_one_relay_address() {
        let too_many = || {
            invalid(MessageError::TooMany {
                field: "relay",
                count: 2,
                max: 1,
            })
        };
        let hello = with_relays(
            |w| {
                w.hash(&Hash::ZERO);
                w.u32(0);
                w.string("");
                w.u32(1);
                w.u32(1);
                w.u32(0);
            },
            2,
        );
        let bytes = frame::encode_frame(MessageType::Hello.code(), &hello);
        assert_eq!(decode_bytes(&bytes), too_many());

        let mut peers = Writer::new();
        peers.u32(1);
        peers.raw(&with_relays(
            |w| {
                w.hash(&Hash::ZERO);
                w.u32(0);
            },
            2,
        ));
        let bytes = frame::encode_frame(MessageType::Peers.code(), &peers.into_bytes());
        assert_eq!(decode_bytes(&bytes), too_many());
    }

    #[test]
    fn list_and_length_limits_are_checked() {
        let record = PeerRecord {
            peer_id: Hash::ZERO,
            addresses: vec![],
            relay: None,
        };
        let candidate = addr("192.0.2.1:1");
        let cases: [(Message, Message, MessageError); 4] = [
            (
                Peers {
                    peers: vec![record.clone(); 50],
                }
                .into(),
                Peers {
                    peers: vec![record; 51],
                }
                .into(),
                MessageError::TooMany {
                    field: "peers",
                    count: 51,
                    max: 50,
                },
            ),
            (
                Candidates {
                    addresses: vec![candidate; 16],
                }
                .into(),
                Candidates {
                    addresses: vec![candidate; 17],
                }
                .into(),
                MessageError::TooMany {
                    field: "addresses",
                    count: 17,
                    max: 16,
                },
            ),
            (
                Data {
                    proof: vec![Hash::ZERO; 32],
                    chunks: vec![],
                }
                .into(),
                Data {
                    proof: vec![Hash::ZERO; 33],
                    chunks: vec![],
                }
                .into(),
                MessageError::TooMany {
                    field: "proof",
                    count: 33,
                    max: 32,
                },
            ),
            (
                Reject {
                    reason: RejectReason::Busy,
                    message: "é".repeat(512),
                }
                .into(),
                Reject {
                    reason: RejectReason::Busy,
                    message: "a".repeat(1025),
                }
                .into(),
                MessageError::TooLong {
                    field: "message",
                    len: 1025,
                    max: 1024,
                },
            ),
        ];
        for (at_limit, over_limit, error) in cases {
            assert_eq!(decode_bytes(&at_limit.encode_frame()), Ok(at_limit));
            let bytes = over_limit.encode_frame();
            // The over-limit Reject is also too long a frame, which split_message_frame catches
            // first, so decode its body directly.
            let frame = Frame {
                message_type: bytes[4],
                body: &bytes[frame::HEADER_LEN..],
            };
            let result = Message::decode(frame);
            if over_limit.message_type() == MessageType::Reject {
                assert!(matches!(result, Err(DecodeError::FrameTooLong { .. })));
            } else {
                assert_eq!(result, invalid(error.clone()));
            }
            assert_eq!(over_limit.validate(), Err(error));
        }
    }

    #[test]
    fn list_limits_are_checked_before_the_entries() {
        // Counts over the limit fail on the count alone, though no entries follow.
        let mut peers = Writer::new();
        peers.u32(u32::MAX);
        let bytes = frame::encode_frame(MessageType::Peers.code(), &peers.into_bytes());
        assert_eq!(
            decode_bytes(&bytes),
            invalid(MessageError::TooMany {
                field: "peers",
                count: u32::MAX as usize,
                max: MAX_PEER_RECORDS,
            })
        );
        let bytes = frame::encode_frame(MessageType::Candidates.code(), &[0, 0, 0, 17]);
        assert_eq!(
            decode_bytes(&bytes),
            invalid(MessageError::TooMany {
                field: "addresses",
                count: 17,
                max: MAX_CANDIDATES,
            })
        );
    }

    #[test]
    fn frames_encode_like_their_bodies() {
        for message in samples() {
            let bytes = message.encode_frame();
            let body = &bytes[frame::HEADER_LEN..];
            assert_eq!(bytes, frame::encode_frame(bytes[4], body));
        }
    }

    #[test]
    fn manifest_size_is_limited() {
        let at_limit = ManifestMessage {
            manifest: vec![0; MAX_MANIFEST_LEN],
        };
        assert_eq!(Message::from(at_limit).validate(), Ok(()));
        let over_limit = ManifestMessage {
            manifest: vec![0; MAX_MANIFEST_LEN + 1],
        };
        assert_eq!(
            Message::from(over_limit).validate(),
            Err(MessageError::TooLong {
                field: "manifest",
                len: MAX_MANIFEST_LEN + 1,
                max: MAX_MANIFEST_LEN,
            })
        );
    }

    #[test]
    fn unknown_reject_reasons_mean_busy() {
        let bytes = frame::encode_frame(MessageType::Reject.code(), &[0, 9, 0, 0, 0, 0]);
        assert_eq!(
            decode_bytes(&bytes),
            Ok(Reject {
                reason: RejectReason::Busy,
                message: String::new(),
            }
            .into())
        );
        for code in 0..=2 {
            assert_eq!(RejectReason::from_code(code).code(), code);
        }
    }

    #[test]
    fn requests_decode_whatever_their_values() {
        let bytes = Message::from(Request { start: 3, count: 6 }).encode_frame();
        assert_eq!(
            decode_bytes(&bytes),
            Ok(Request { start: 3, count: 6 }.into())
        );
    }

    #[test]
    fn oversized_request_frame_is_rejected() {
        assert_eq!(
            split_message_frame(&[0, 0, 0, 10, 0x10]),
            Err(DecodeError::FrameTooLong { length: 10, max: 9 })
        );
    }
}
