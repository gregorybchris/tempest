//! The relay connection's messages and relay identities (`protocol/relays.md`).
//!
//! A relay connection carries two kinds of traffic:
//!
//! - Control messages ([`RelayMessage`]) on the control stream, framed like peer messages but
//!   with their own type table and size limits. Split frames with [`split_relay_frame`].
//! - [`RelayDatagram`]s (Forward and Forwarded) as unframed QUIC datagrams.

mod datagram;
mod identity;

pub use datagram::RelayDatagram;
pub use identity::{DEFAULT_RELAY_PORT, ParseRelayIdentityError, RelayHost, RelayIdentity};

use crate::address::Address;
use crate::frame::{self, Frame};
use crate::message::{MAX_REASON_MESSAGE_LEN, MessageError, check_len};
use crate::wire::{Decode, DecodeError, Encode, Reader, Writer};

/// The ALPN identifier of a relay connection.
pub const RELAY_ALPN: &[u8] = b"tempest-relay/1";

/// The smallest QUIC datagram payload both sides of a relay connection must accept: enough for a
/// 1,200-byte peer packet plus the Forward or Forwarded wrapper.
pub const MIN_RELAY_DATAGRAM_LEN: usize = 1300;

/// The longest encoded [`Address`]: an IPv6 address.
const MAX_ADDRESS_LEN: u32 = 1 + 16 + 2;

/// A relay control message type, with its code on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum RelayMessageType {
    /// The peer asks to register.
    Register = 0x01,
    /// The relay accepts, lending a relay address.
    Registered = 0x02,
    /// The relay refuses.
    RelayReject = 0x03,
}

impl RelayMessageType {
    /// The message type with wire code `code`, if there is one.
    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            0x01 => Some(Self::Register),
            0x02 => Some(Self::Registered),
            0x03 => Some(Self::RelayReject),
            _ => None,
        }
    }

    /// The type's wire code.
    pub fn code(self) -> u8 {
        self as u8
    }

    /// The largest allowed frame `length` (type byte plus body) for this type: its largest
    /// encoding.
    pub fn max_frame_length(self) -> u32 {
        match self {
            Self::Register => 1,
            Self::Registered => 1 + 2 * MAX_ADDRESS_LEN,
            Self::RelayReject => 1 + 2 + 4 + MAX_REASON_MESSAGE_LEN,
        }
    }
}

/// Reads one relay control frame from the front of `buf`, applying the size limit for its type.
///
/// See [`frame::split_frame`] for the return value.
pub fn split_relay_frame(buf: &[u8]) -> Result<Option<(Frame<'_>, usize)>, DecodeError> {
    frame::split_frame(buf, |code| {
        RelayMessageType::from_code(code)
            .map(RelayMessageType::max_frame_length)
            .ok_or(DecodeError::UnknownMessageType(code))
    })
}

/// A relay control message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayMessage {
    /// The peer asks to register. It has no fields.
    Register,
    /// The relay accepts, lending a relay address.
    Registered(Registered),
    /// The relay refuses, then closes the connection.
    RelayReject(RelayReject),
}

/// The relay's answer to Register, and its update when the peer's observed address changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Registered {
    /// The address this relay lends to the peer.
    pub relay_address: Address,
    /// The peer's address, as the relay sees it.
    pub observed_address: Address,
}

/// The relay's refusal of Register.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayReject {
    /// Why the relay refused.
    pub reason: RelayRejectReason,
    /// Human-readable detail. May be empty; at most 1 KiB.
    pub message: String,
}

/// Why a relay refused a registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum RelayRejectReason {
    /// The relay only accepts peers on its allowlist.
    NotAllowed = 0,
    /// The relay is out of capacity.
    Full = 1,
    /// The registration broke a rule in the specification.
    Invalid = 2,
}

impl RelayRejectReason {
    /// The reason with wire code `code`. An unknown code means [`RelayRejectReason::Full`].
    pub fn from_code(code: u16) -> Self {
        match code {
            0 => Self::NotAllowed,
            2 => Self::Invalid,
            _ => Self::Full,
        }
    }

    /// The reason's wire code.
    pub fn code(self) -> u16 {
        self as u16
    }
}

impl RelayMessage {
    /// The message's type.
    pub fn message_type(&self) -> RelayMessageType {
        match self {
            Self::Register => RelayMessageType::Register,
            Self::Registered(_) => RelayMessageType::Registered,
            Self::RelayReject(_) => RelayMessageType::RelayReject,
        }
    }

    /// Checks the rules that the encoding alone doesn't enforce: RelayReject's message length.
    ///
    /// [`RelayMessage::decode`] applies this check; encoding doesn't.
    pub fn validate(&self) -> Result<(), MessageError> {
        match self {
            Self::RelayReject(m) => {
                check_len("message", m.message.len(), MAX_REASON_MESSAGE_LEN as usize)
            }
            Self::Register | Self::Registered(_) => Ok(()),
        }
    }

    /// Encodes the message as a frame, ready to send on the control stream.
    pub fn encode_frame(&self) -> Vec<u8> {
        let mut w = Writer::new();
        match self {
            Self::Register => {}
            Self::Registered(m) => {
                m.relay_address.encode(&mut w);
                m.observed_address.encode(&mut w);
            }
            Self::RelayReject(m) => {
                w.u16(m.reason.code());
                w.string(&m.message);
            }
        }
        frame::encode_frame(self.message_type().code(), &w.into_bytes())
    }

    /// Decodes a frame's message, rejecting anything that is a protocol violation: an unknown
    /// type, a length over the type's limit, a body shorter or longer than its fields, or a
    /// message that fails [`RelayMessage::validate`].
    pub fn decode(frame: Frame<'_>) -> Result<Self, DecodeError> {
        let message_type = RelayMessageType::from_code(frame.message_type)
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
            RelayMessageType::Register => Self::Register,
            RelayMessageType::Registered => Self::Registered(Registered {
                relay_address: Address::decode(&mut r)?,
                observed_address: Address::decode(&mut r)?,
            }),
            RelayMessageType::RelayReject => Self::RelayReject(RelayReject {
                reason: RelayRejectReason::from_code(r.u16()?),
                message: r.string()?.to_owned(),
            }),
        };
        r.finish()?;
        message.validate()?;
        Ok(message)
    }
}

impl From<Registered> for RelayMessage {
    fn from(m: Registered) -> Self {
        Self::Registered(m)
    }
}

impl From<RelayReject> for RelayMessage {
    fn from(m: RelayReject) -> Self {
        Self::RelayReject(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(s: &str) -> Address {
        s.parse().unwrap()
    }

    fn decode(bytes: &[u8]) -> Result<RelayMessage, DecodeError> {
        let (frame, used) = split_relay_frame(bytes)?.expect("whole frame");
        assert_eq!(used, bytes.len());
        RelayMessage::decode(frame)
    }

    fn samples() -> Vec<RelayMessage> {
        vec![
            RelayMessage::Register,
            Registered {
                relay_address: addr("198.51.100.1:40007"),
                observed_address: addr("[2001:db8::1]:4433"),
            }
            .into(),
            RelayReject {
                reason: RelayRejectReason::NotAllowed,
                message: "allowlist only".into(),
            }
            .into(),
        ]
    }

    #[test]
    fn every_type_round_trips() {
        for message in samples() {
            assert_eq!(decode(&message.encode_frame()), Ok(message));
        }
    }

    #[test]
    fn register_is_a_bare_type_byte() {
        assert_eq!(RelayMessage::Register.encode_frame(), [0, 0, 0, 1, 1]);
    }

    #[test]
    fn largest_encodings_fit_their_limits() {
        let registered: RelayMessage = Registered {
            relay_address: addr("[2001:db8::1]:1"),
            observed_address: addr("[2001:db8::2]:2"),
        }
        .into();
        let reject: RelayMessage = RelayReject {
            reason: RelayRejectReason::Full,
            message: "x".repeat(MAX_REASON_MESSAGE_LEN as usize),
        }
        .into();
        for message in [registered, reject] {
            let frame = message.encode_frame();
            assert_eq!(
                frame.len() - 4,
                message.message_type().max_frame_length() as usize
            );
            assert_eq!(decode(&frame), Ok(message));
        }
    }

    #[test]
    fn rejects_over_long_frames_before_the_body() {
        assert_eq!(
            split_relay_frame(&[0, 0, 0, 2, 1]),
            Err(DecodeError::FrameTooLong { length: 2, max: 1 })
        );
        assert_eq!(
            split_relay_frame(&[0, 0, 0, 1, 4]),
            Err(DecodeError::UnknownMessageType(4))
        );
    }

    #[test]
    fn rejects_short_bodies() {
        let frame = samples()[1].encode_frame();
        let mut short = frame[..frame.len() - 1].to_vec();
        short[3] -= 1;
        assert_eq!(decode(&short), Err(DecodeError::UnexpectedEnd));
    }

    #[test]
    fn rejects_trailing_bytes() {
        let registered = Frame {
            message_type: 2,
            body: &[4, 1, 2, 3, 4, 0, 1, 4, 1, 2, 3, 4, 0, 2, 0],
        };
        assert_eq!(
            RelayMessage::decode(registered),
            Err(DecodeError::TrailingBytes(1))
        );
        let register = Frame {
            message_type: 1,
            body: &[0],
        };
        assert!(matches!(
            RelayMessage::decode(register),
            Err(DecodeError::FrameTooLong { .. })
        ));
    }

    #[test]
    fn rejects_an_over_long_reject_message() {
        let message: RelayMessage = RelayReject {
            reason: RelayRejectReason::Full,
            message: "x".repeat(MAX_REASON_MESSAGE_LEN as usize + 1),
        }
        .into();
        let bytes = message.encode_frame();
        let frame = Frame {
            message_type: bytes[4],
            body: &bytes[5..],
        };
        assert!(matches!(
            RelayMessage::decode(frame),
            Err(DecodeError::FrameTooLong { .. })
        ));
        assert_eq!(
            message.validate(),
            Err(MessageError::TooLong {
                field: "message",
                len: 1025,
                max: 1024
            })
        );
    }

    #[test]
    fn unknown_reject_reasons_mean_full() {
        assert_eq!(RelayRejectReason::from_code(7), RelayRejectReason::Full);
        let decoded = decode(&[0, 0, 0, 7, 3, 0, 9, 0, 0, 0, 0]).unwrap();
        assert_eq!(
            decoded,
            RelayReject {
                reason: RelayRejectReason::Full,
                message: String::new()
            }
            .into()
        );
    }
}
