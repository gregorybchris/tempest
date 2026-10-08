//! Property tests for LAN messages, relay control messages, relay datagrams and relay
//! identities: valid values round-trip, and decoding arbitrary input never panics.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use proptest::prelude::*;
use tempest_protocol::lan::{LAN_MAGIC, LAN_MESSAGE_LEN, LanKind, LanMessage};
use tempest_protocol::relay::{
    Registered, RelayHost, RelayReject, RelayRejectReason, split_relay_frame,
};
use tempest_protocol::{Address, Hash, RelayDatagram, RelayIdentity, RelayMessage};

fn any_hash() -> impl Strategy<Value = Hash> {
    any::<[u8; 32]>().prop_map(Hash::new)
}

fn any_address() -> impl Strategy<Value = Address> {
    any::<SocketAddr>().prop_map(Address::from)
}

fn any_relay_message() -> impl Strategy<Value = RelayMessage> {
    prop_oneof![
        Just(RelayMessage::Register),
        (any_address(), any_address()).prop_map(|(relay_address, observed_address)| {
            Registered {
                relay_address,
                observed_address,
            }
            .into()
        }),
        (0..3u16, ".{0,64}").prop_map(|(code, message)| {
            RelayReject {
                reason: RelayRejectReason::from_code(code),
                message,
            }
            .into()
        }),
    ]
}

fn any_relay_host() -> impl Strategy<Value = RelayHost> {
    prop_oneof![
        any::<Ipv4Addr>().prop_map(|ip| RelayHost::Ip(IpAddr::V4(ip))),
        any::<Ipv6Addr>()
            .prop_filter("not IPv4-mapped", |ip| ip.to_ipv4_mapped().is_none())
            .prop_map(|ip| RelayHost::Ip(IpAddr::V6(ip))),
        "[a-z0-9]([a-z0-9-]{0,10}[a-z0-9])?(\\.[a-z]([a-z0-9-]{0,10}[a-z0-9])?){0,3}"
            .prop_map(RelayHost::Domain),
    ]
}

proptest! {
    #[test]
    fn lan_messages_round_trip(kind in prop_oneof![Just(LanKind::Announce), Just(LanKind::Query)],
                               peer_id in any_hash(), port: u16) {
        let message = LanMessage { kind, peer_id, port };
        prop_assert_eq!(LanMessage::decode(&message.encode()), Some(message));
    }

    #[test]
    fn lan_decoding_accepts_only_valid_datagrams(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
        if let Some(message) = LanMessage::decode(&bytes) {
            prop_assert_eq!(bytes.len(), LAN_MESSAGE_LEN);
            prop_assert_eq!(&bytes[..4], &LAN_MAGIC);
            let encoded = message.encode();
            prop_assert_eq!(encoded.as_slice(), bytes.as_slice());
        }
    }

    #[test]
    fn relay_messages_round_trip(message in any_relay_message()) {
        let bytes = message.encode_frame();
        let (frame, used) = split_relay_frame(&bytes).unwrap().unwrap();
        prop_assert_eq!(used, bytes.len());
        prop_assert_eq!(RelayMessage::decode(frame), Ok(message));
    }

    #[test]
    fn relay_frame_decoding_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
        if let Ok(Some((frame, _))) = split_relay_frame(&bytes)
            && let Ok(message) = RelayMessage::decode(frame)
        {
            let reencoded = message.encode_frame();
            let (frame, _) = split_relay_frame(&reencoded).unwrap().unwrap();
            prop_assert_eq!(RelayMessage::decode(frame), Ok(message));
        }
    }

    #[test]
    fn relay_datagrams_round_trip(forward: bool, address in any_address(),
                                  packet in proptest::collection::vec(any::<u8>(), 0..1300)) {
        let datagram = if forward {
            RelayDatagram::Forward { to: address, packet: &packet }
        } else {
            RelayDatagram::Forwarded { from: address, packet: &packet }
        };
        let encoded = datagram.encode();
        prop_assert_eq!(RelayDatagram::decode(&encoded), Ok(datagram));
    }

    #[test]
    fn decoded_datagrams_re_encode_exactly(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
        if let Ok(datagram) = RelayDatagram::decode(&bytes) {
            prop_assert_eq!(datagram.encode(), bytes);
        }
    }

    #[test]
    fn relay_identities_round_trip(key in any_hash(), host in any_relay_host(), port in 1..=u16::MAX) {
        prop_assume!(!matches!(&host, RelayHost::Domain(name)
            if name.rsplit('.').next().unwrap().bytes().all(|b| b.is_ascii_digit())));
        let identity = RelayIdentity { key, host, port };
        prop_assert_eq!(identity.to_string().parse::<RelayIdentity>(), Ok(identity));
    }

    #[test]
    fn relay_identity_parsing_never_panics(text in "\\PC{0,96}") {
        if let Ok(identity) = text.parse::<RelayIdentity>() {
            prop_assert_eq!(identity.to_string().parse::<RelayIdentity>(), Ok(identity));
        }
    }
}
