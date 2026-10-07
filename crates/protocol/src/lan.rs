//! LAN discovery messages (`protocol/discovery.md`, LAN discovery).
//!
//! Peers announce themselves to a multicast group, and query it to hear from others sooner. A
//! message names only the sender's peer ID and port, never its bundles.

use std::net::{Ipv4Addr, Ipv6Addr};

use crate::hash::{HASH_LEN, PeerId};
use crate::wire::{Reader, Writer};

/// The bytes every LAN message starts with: ASCII `TMPS`.
pub const LAN_MAGIC: [u8; 4] = *b"TMPS";

/// The exact length of a LAN message.
pub const LAN_MESSAGE_LEN: usize = LAN_MAGIC.len() + 1 + HASH_LEN + 2;

/// The IPv4 multicast group.
pub const LAN_GROUP_V4: Ipv4Addr = Ipv4Addr::new(239, 255, 84, 77);

/// The IPv6 multicast group.
pub const LAN_GROUP_V6: Ipv6Addr = Ipv6Addr::new(0xff12, 0, 0, 0, 0, 0, 0x7465, 0x6d70);

/// The UDP port of both groups.
pub const LAN_PORT: u16 = 4774;

/// What a LAN message asks of its receivers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum LanKind {
    /// "I am here."
    Announce = 1,
    /// "Who is here?" Receivers reply with an announce, sent directly to the source.
    Query = 2,
}

impl LanKind {
    /// The kind with wire code `code`, if there is one.
    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Announce),
            2 => Some(Self::Query),
            _ => None,
        }
    }

    /// The kind's wire code.
    pub fn code(self) -> u8 {
        self as u8
    }
}

/// A LAN discovery datagram. The sender's IP address is the datagram's source address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LanMessage {
    /// Announce or query.
    pub kind: LanKind,
    /// The sender's peer ID.
    pub peer_id: PeerId,
    /// The UDP port of the sender's QUIC endpoint.
    pub port: u16,
}

impl LanMessage {
    /// Encodes the message as one datagram.
    pub fn encode(&self) -> [u8; LAN_MESSAGE_LEN] {
        let mut w = Writer::new();
        w.raw(&LAN_MAGIC);
        w.u8(self.kind.code());
        w.hash(&self.peer_id);
        w.u16(self.port);
        w.into_bytes().try_into().expect("fixed length")
    }

    /// Decodes a datagram. Returns `None` for anything a receiver must silently ignore: the wrong
    /// length, the wrong magic or an unknown kind.
    pub fn decode(datagram: &[u8]) -> Option<Self> {
        if datagram.len() != LAN_MESSAGE_LEN {
            return None;
        }
        let mut r = Reader::new(datagram);
        if r.take(LAN_MAGIC.len()).ok()? != LAN_MAGIC {
            return None;
        }
        Some(Self {
            kind: LanKind::from_code(r.u8().ok()?)?,
            peer_id: r.hash().ok()?,
            port: r.u16().ok()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::Hash;

    fn announce() -> LanMessage {
        LanMessage {
            kind: LanKind::Announce,
            peer_id: Hash::new([0xab; 32]),
            port: 4433,
        }
    }

    #[test]
    fn layout_matches_the_spec() {
        let bytes = announce().encode();
        assert_eq!(LAN_MESSAGE_LEN, 39);
        assert_eq!(&bytes[..5], b"TMPS\x01");
        assert_eq!(&bytes[5..37], &[0xab; 32]);
        assert_eq!(&bytes[37..], &[0x11, 0x51]);
        assert_eq!(LanMessage::decode(&bytes), Some(announce()));
    }

    #[test]
    fn groups_match_the_spec() {
        assert_eq!(LAN_GROUP_V4.to_string(), "239.255.84.77");
        assert_eq!(LAN_GROUP_V6.to_string(), "ff12::7465:6d70");
    }

    #[test]
    fn ignores_wrong_length_magic_or_kind() {
        let bytes = announce().encode();
        assert_eq!(LanMessage::decode(&bytes[..38]), None);
        assert_eq!(LanMessage::decode(&[bytes.as_slice(), &[0]].concat()), None);

        let mut magic = bytes;
        magic[0] = b'X';
        assert_eq!(LanMessage::decode(&magic), None);

        for kind in [0, 3, 0xff] {
            let mut unknown = bytes;
            unknown[4] = kind;
            assert_eq!(LanMessage::decode(&unknown), None);
        }
    }
}
