//! Property tests for peer messages: every message round-trips, Hello ignores its tail, and
//! decoding arbitrary frames never panics.

use proptest::collection::vec;
use proptest::prelude::*;
use tempest_protocol::frame::{Frame, encode_frame};
use tempest_protocol::message::{
    Candidates, Data, Have, HaveUpdate, Hello, ManifestMessage, Message, MessageType, PeerRecord,
    Peers, Reject, RejectReason, Request, Serving, Want, split_message_frame,
};
use tempest_protocol::{Address, Hash, RangeList};

fn any_hash() -> impl Strategy<Value = Hash> {
    any::<[u8; 32]>().prop_map(Hash::new)
}

fn any_address() -> impl Strategy<Value = Address> {
    any::<std::net::SocketAddr>().prop_map(Address::from)
}

fn any_ranges() -> impl Strategy<Value = RangeList> {
    vec(any::<u32>().prop_map(|i| i % 4096), 0..32).prop_map(RangeList::from_chunks)
}

fn any_hello() -> impl Strategy<Value = Hello> {
    (
        any_hash(),
        vec(".{0,8}", 0..3),
        ".{0,16}",
        0..=10u32,
        1..=u32::MAX,
        vec(any_address(), 0..4),
        proptest::option::of(any_address()),
    )
        .prop_map(
            |(bundle_id, capabilities, client, log, open, addresses, relay)| Hello {
                bundle_id,
                capabilities,
                client,
                max_request_chunks: 1 << log,
                max_open_requests: open,
                addresses,
                relay,
            },
        )
}

fn any_peer_record() -> impl Strategy<Value = PeerRecord> {
    (
        any_hash(),
        vec(any_address(), 0..4),
        proptest::option::of(any_address()),
    )
        .prop_map(|(peer_id, addresses, relay)| PeerRecord {
            peer_id,
            addresses,
            relay,
        })
}

fn any_reject_reason() -> impl Strategy<Value = RejectReason> {
    prop_oneof![
        Just(RejectReason::Busy),
        Just(RejectReason::DontHave),
        Just(RejectReason::Invalid),
    ]
}

/// Any valid message.
fn any_message() -> impl Strategy<Value = Message> {
    prop_oneof![
        any_hello().prop_map(Message::from),
        any_ranges().prop_map(|ranges| Have { ranges }.into()),
        (any_ranges(), any_ranges())
            .prop_map(|(added, removed)| HaveUpdate { added, removed }.into()),
        any::<bool>().prop_map(|wanted| Want { wanted }.into()),
        any::<bool>().prop_map(|serving| Serving { serving }.into()),
        vec(any_peer_record(), 0..=50).prop_map(|peers| Peers { peers }.into()),
        vec(any_address(), 0..=16).prop_map(|addresses| Candidates { addresses }.into()),
        (any::<u32>(), any::<u32>()).prop_map(|(start, count)| Request { start, count }.into()),
        (vec(any_hash(), 0..=32), vec(any::<u8>(), 0..256)).prop_map(|(proof, chunks)| Data {
            proof,
            chunks
        }
        .into()),
        (any_reject_reason(), ".{0,64}")
            .prop_map(|(reason, message)| Reject { reason, message }.into()),
        Just(Message::ManifestRequest),
        vec(any::<u8>(), 0..256).prop_map(|manifest| ManifestMessage { manifest }.into()),
    ]
}

fn decode(bytes: &[u8]) -> Result<Message, tempest_protocol::DecodeError> {
    let (frame, used) = split_message_frame(bytes)?.expect("a whole frame");
    assert_eq!(used, bytes.len());
    Message::decode(frame)
}

proptest! {
    #[test]
    fn messages_round_trip(message in any_message()) {
        prop_assert_eq!(message.validate(), Ok(()));
        let bytes = message.encode_frame();
        prop_assert_eq!(decode(&bytes), Ok(message));
    }

    #[test]
    fn hello_ignores_trailing_bytes(hello in any_hello(), tail in vec(any::<u8>(), 1..64)) {
        let message = Message::from(hello);
        let mut bytes = message.encode_frame();
        bytes.extend_from_slice(&tail);
        let body = &bytes[5..];
        let framed = encode_frame(MessageType::Hello.code(), body);
        prop_assert_eq!(decode(&framed), Ok(message));
    }

    #[test]
    fn other_messages_reject_trailing_bytes(message in any_message(), tail in vec(any::<u8>(), 1..8)) {
        prop_assume!(!matches!(message, Message::Hello(_)));
        let mut bytes = message.encode_frame();
        bytes.extend_from_slice(&tail);
        let frame = Frame { message_type: bytes[4], body: &bytes[5..] };
        prop_assert!(Message::decode(frame).is_err());
    }

    #[test]
    fn decoding_never_panics(message_type in any::<u8>(), body in vec(any::<u8>(), 0..512)) {
        let frame = Frame { message_type, body: &body };
        if let Ok(message) = Message::decode(frame) {
            // Whatever decodes re-encodes to a frame that decodes to the same message.
            prop_assert_eq!(decode(&message.encode_frame()), Ok(message));
        }
    }

    #[test]
    fn decoding_a_corrupted_message_never_panics(
        message in any_message(), index in any::<prop::sample::Index>(), byte in any::<u8>(),
    ) {
        let bytes = message.encode_frame();
        let mut body = bytes[5..].to_vec();
        if !body.is_empty() {
            let i = index.index(body.len());
            body[i] = byte;
        }
        let _ = Message::decode(Frame { message_type: bytes[4], body: &body });
    }
}
