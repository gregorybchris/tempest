//! Forward and Forwarded datagrams (`protocol/relays.md`, Forwarding datagrams).

use crate::address::Address;
use crate::wire::{Decode, DecodeError, Encode, Reader, Writer};

const FORWARD_KIND: u8 = 1;
const FORWARDED_KIND: u8 = 2;

/// One QUIC datagram on a relay connection, wrapping one UDP payload of the inner connection.
///
/// The packet is borrowed from the datagram it was decoded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayDatagram<'a> {
    /// Peer to relay: send `packet` from the peer's relay address to `to`.
    Forward {
        /// Where the relay should send the packet.
        to: Address,
        /// One UDP payload.
        packet: &'a [u8],
    },
    /// Relay to peer: `packet` arrived at the peer's relay address from `from`.
    Forwarded {
        /// Who sent the packet to the peer's relay address.
        from: Address,
        /// One UDP payload.
        packet: &'a [u8],
    },
}

impl<'a> RelayDatagram<'a> {
    /// Encodes the datagram.
    pub fn encode(&self) -> Vec<u8> {
        let (kind, address, packet) = match *self {
            Self::Forward { to, packet } => (FORWARD_KIND, to, packet),
            Self::Forwarded { from, packet } => (FORWARDED_KIND, from, packet),
        };
        let mut w = Writer::new();
        w.u8(kind);
        address.encode(&mut w);
        w.raw(packet);
        w.into_bytes()
    }

    /// Decodes a datagram. A receiver drops any datagram this rejects, including one with an
    /// unknown kind ([`DecodeError::UnknownDatagramKind`]).
    pub fn decode(datagram: &'a [u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(datagram);
        let kind = r.u8()?;
        if kind != FORWARD_KIND && kind != FORWARDED_KIND {
            return Err(DecodeError::UnknownDatagramKind(kind));
        }
        let address = Address::decode(&mut r)?;
        let packet = r.rest();
        Ok(if kind == FORWARD_KIND {
            Self::Forward {
                to: address,
                packet,
            }
        } else {
            Self::Forwarded {
                from: address,
                packet,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_the_spec() {
        let datagram = RelayDatagram::Forwarded {
            from: "203.0.113.5:4433".parse().unwrap(),
            packet: b"quic",
        };
        let bytes = datagram.encode();
        assert_eq!(bytes, hex::decode("0204cb007105115171756963").unwrap());
        assert_eq!(RelayDatagram::decode(&bytes), Ok(datagram));
    }

    #[test]
    fn forward_round_trips_with_an_empty_packet() {
        let datagram = RelayDatagram::Forward {
            to: "[2001:db8::1]:1".parse().unwrap(),
            packet: &[],
        };
        assert_eq!(RelayDatagram::decode(&datagram.encode()), Ok(datagram));
    }

    #[test]
    fn rejects_unknown_kinds_and_bad_addresses() {
        assert_eq!(RelayDatagram::decode(&[]), Err(DecodeError::UnexpectedEnd));
        assert_eq!(
            RelayDatagram::decode(&[3, 4, 1, 2, 3, 4, 0, 1]),
            Err(DecodeError::UnknownDatagramKind(3))
        );
        assert_eq!(
            RelayDatagram::decode(&[1, 5, 1, 2, 3, 4, 0, 1]),
            Err(DecodeError::UnknownAddressFamily(5))
        );
        assert_eq!(
            RelayDatagram::decode(&[1, 4, 1, 2, 3]),
            Err(DecodeError::UnexpectedEnd)
        );
    }
}
