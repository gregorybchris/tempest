//! Property tests for the tracker API's JSON types: every value round-trips through JSON, and
//! parsing arbitrary text never panics.

use std::net::{IpAddr, SocketAddr};

use proptest::collection::vec;
use proptest::prelude::*;
use tempest_protocol::relay::RelayHost;
use tempest_protocol::tracker::{
    AnnounceEvent, AnnounceRequest, AnnounceResponse, BundleStats, ErrorCode, ErrorResponse,
    ListingRequest, PeerEntry, Signature,
};
use tempest_protocol::{Address, Hash, RelayIdentity};

fn any_hash() -> impl Strategy<Value = Hash> {
    any::<[u8; 32]>().prop_map(Hash::new)
}

fn any_signature() -> impl Strategy<Value = Signature> {
    (any::<[u8; 32]>(), any::<[u8; 32]>()).prop_map(|(a, b)| {
        let mut bytes = [0; 64];
        bytes[..32].copy_from_slice(&a);
        bytes[32..].copy_from_slice(&b);
        Signature(bytes)
    })
}

fn any_address() -> impl Strategy<Value = Address> {
    any::<SocketAddr>().prop_map(Address::from)
}

fn any_event() -> impl Strategy<Value = AnnounceEvent> {
    prop_oneof![
        Just(AnnounceEvent::Started),
        Just(AnnounceEvent::Update),
        Just(AnnounceEvent::Completed),
        Just(AnnounceEvent::Stopped),
    ]
}

fn any_request() -> impl Strategy<Value = AnnounceRequest> {
    (
        any_hash(),
        any_hash(),
        any::<u16>(),
        vec(any_address(), 0..4),
        proptest::option::of(any_address()),
        any_event(),
        any::<bool>(),
        any::<u32>(),
        proptest::option::of(any_signature()),
    )
        .prop_map(
            |(bundle, peer, port, addrs, relay, event, complete, num_want, sig)| AnnounceRequest {
                bundle,
                peer,
                port,
                addrs,
                relay,
                event,
                complete,
                num_want,
                listing: sig.map(|sig| ListingRequest { sig }),
            },
        )
}

fn any_response() -> impl Strategy<Value = AnnounceResponse> {
    let peer = (
        any_hash(),
        vec(any_address(), 0..3),
        proptest::option::of(any_address()),
    )
        .prop_map(|(peer, addrs, relay)| PeerEntry { peer, addrs, relay });
    let relay =
        (any_hash(), any::<std::net::Ipv4Addr>(), 1..=u16::MAX).prop_map(|(key, ip, port)| {
            RelayIdentity {
                key,
                host: RelayHost::Ip(IpAddr::V4(ip)),
                port,
            }
        });
    (
        any::<u64>(),
        vec(peer, 0..4),
        any::<u64>(),
        any::<u64>(),
        any::<IpAddr>(),
        vec(relay, 0..3),
    )
        .prop_map(
            |(interval_secs, peers, complete, incomplete, your_ip, relays)| AnnounceResponse {
                interval_secs,
                peers,
                complete,
                incomplete,
                your_ip,
                relays,
            },
        )
}

fn any_error() -> impl Strategy<Value = ErrorResponse> {
    (
        "[a-z_]{0,20}",
        ".{0,32}",
        proptest::option::of(any::<u64>()),
    )
        .prop_map(|(code, message, retry_after_secs)| ErrorResponse {
            error: code.parse::<ErrorCode>().unwrap(),
            message,
            retry_after_secs,
        })
}

fn round_trip<T>(value: &T) -> T
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    serde_json::from_str(&serde_json::to_string(value).unwrap()).unwrap()
}

proptest! {
    #[test]
    fn announce_requests_round_trip(request in any_request()) {
        prop_assert_eq!(round_trip(&request), request);
    }

    #[test]
    fn announce_responses_round_trip(response in any_response()) {
        prop_assert_eq!(round_trip(&response), response);
    }

    #[test]
    fn errors_round_trip(error in any_error()) {
        prop_assert_eq!(round_trip(&error), error);
    }

    #[test]
    fn bundle_stats_round_trip(complete: u64, incomplete: u64) {
        let stats = BundleStats { complete, incomplete };
        prop_assert_eq!(round_trip(&stats), stats);
    }

    #[test]
    fn parsing_arbitrary_json_never_panics(text in "\\PC{0,128}") {
        let _ = serde_json::from_str::<AnnounceRequest>(&text);
        let _ = serde_json::from_str::<AnnounceResponse>(&text);
        let _ = serde_json::from_str::<ErrorResponse>(&text);
        let _ = serde_json::from_str::<BundleStats>(&text);
    }
}
