//! The tracker API's JSON types (`protocol/trackers.md`).
//!
//! The types implement serde's `Serialize` and `Deserialize`. Deserializing ignores unknown
//! fields and rejects malformed IDs and addresses. The HTTP exchange itself belongs to the caller.

use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};

use crate::address::Address;
use crate::hash::PeerId;
use crate::manifest::BundleId;
use crate::relay::RelayIdentity;

/// The path prefix of every endpoint.
pub const API_PREFIX: &str = "/tempest/v1/";

/// The number of peers a client wants when its announce doesn't say.
pub const DEFAULT_NUM_WANT: u32 = 50;

/// The URL of the endpoint at `path` (relative to [`API_PREFIX`]) on the tracker at `tracker`.
///
/// ```
/// use tempest_protocol::tracker::endpoint_url;
/// assert_eq!(
///     endpoint_url("https://example.org/tracker/", "announce"),
///     "https://example.org/tracker/tempest/v1/announce"
/// );
/// ```
pub fn endpoint_url(tracker: &str, path: &str) -> String {
    format!("{}{API_PREFIX}{path}", tracker.trim_end_matches('/'))
}

/// The URL to announce to: `POST /tempest/v1/announce`.
pub fn announce_url(tracker: &str) -> String {
    endpoint_url(tracker, "announce")
}

/// The URL of a bundle's swarm counts: `GET /tempest/v1/bundles/{bundle}`.
pub fn bundle_stats_url(tracker: &str, bundle: &BundleId) -> String {
    endpoint_url(tracker, &format!("bundles/{bundle}"))
}

/// The body of `POST /tempest/v1/announce`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnnounceRequest {
    /// The bundle ID.
    pub bundle: BundleId,
    /// The announcing peer's peer ID.
    pub peer: PeerId,
    /// The UDP port of the peer's QUIC endpoint.
    pub port: u16,
    /// Additional direct addresses reachable from the internet. Defaults to none.
    #[serde(default)]
    pub addrs: Vec<Address>,
    /// The peer's relay address, if it has one. The field is required, but may be `null`.
    #[serde(deserialize_with = "required_option")]
    pub relay: Option<Address>,
    /// Why the peer is announcing.
    pub event: AnnounceEvent,
    /// Whether the peer has the whole bundle.
    pub complete: bool,
    /// How many peers the client would like. Defaults to [`DEFAULT_NUM_WANT`].
    #[serde(default = "default_num_want")]
    pub num_want: u32,
    /// Asks a tracker that takes listings to list the bundle. Absent unless the peer opted in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listing: Option<ListingRequest>,
}

/// The `listing` field of an announce: the peer's signature over [`listing_message`], which
/// proves that the owner of the peer ID asked for the bundle to be listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListingRequest {
    /// The Ed25519 signature, by the announcing peer's key.
    pub sig: Signature,
}

/// What a peer signs to ask for a bundle's listing: [`LISTING_CONTEXT`] followed by the bundle
/// ID. It depends on the bundle alone, so a client computes the signature once.
///
/// ```
/// use tempest_protocol::tracker::listing_message;
/// let message = listing_message(&"3f9a".repeat(16).parse().unwrap());
/// assert_eq!(&message[..19], b"tempest listing v1\0");
/// assert_eq!(message.len(), 51);
/// ```
pub fn listing_message(bundle: &BundleId) -> Vec<u8> {
    [LISTING_CONTEXT, bundle.as_bytes()].concat()
}

/// The prefix of [`listing_message`], which keeps its signatures from meaning anything else.
pub const LISTING_CONTEXT: &[u8] = b"tempest listing v1\0";

/// The length of an Ed25519 signature in bytes.
pub const SIGNATURE_LEN: usize = 64;

/// An Ed25519 signature. As text, 128 lowercase hexadecimal characters.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Signature(pub [u8; SIGNATURE_LEN]);

impl fmt::Display for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Signature({self})")
    }
}

/// Why text could not be parsed as a signature.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a signature must be 128 lowercase hexadecimal characters")]
pub struct ParseSignatureError;

impl FromStr for Signature {
    type Err = ParseSignatureError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != 2 * SIGNATURE_LEN || !s.is_ascii() {
            return Err(ParseSignatureError);
        }
        // Two hashes' worth of hex, parsed with the hash's strict rules.
        let (first, second) = s.split_at(SIGNATURE_LEN);
        let first: crate::Hash = first.parse().map_err(|_| ParseSignatureError)?;
        let second: crate::Hash = second.parse().map_err(|_| ParseSignatureError)?;
        let mut bytes = [0; SIGNATURE_LEN];
        bytes[..32].copy_from_slice(first.as_bytes());
        bytes[32..].copy_from_slice(second.as_bytes());
        Ok(Self(bytes))
    }
}

fn default_num_want() -> u32 {
    DEFAULT_NUM_WANT
}

/// Deserializes an `Option` field that must be present, though it may be `null`.
///
/// Serde treats a missing `Option` field as `None` unless the field has its own deserializer.
fn required_option<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(d)
}

/// Why a peer announces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnounceEvent {
    /// The peer began participating in the bundle.
    Started,
    /// The periodic announce between other events.
    Update,
    /// The peer became complete while participating.
    Completed,
    /// The peer stopped participating.
    Stopped,
}

/// The tracker's answer to an announce (HTTP 200).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnnounceResponse {
    /// Seconds until the peer should announce again.
    pub interval_secs: u64,
    /// Other peers in the swarm.
    pub peers: Vec<PeerEntry>,
    /// The number of complete peers the tracker knows of.
    pub complete: u64,
    /// The number of incomplete peers the tracker knows of.
    pub incomplete: u64,
    /// The IP address the tracker observed for the request.
    pub your_ip: IpAddr,
    /// Relays the tracker recommends.
    pub relays: Vec<RelayIdentity>,
}

/// One peer in an announce response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerEntry {
    /// The peer's peer ID.
    pub peer: PeerId,
    /// The peer's direct addresses.
    pub addrs: Vec<Address>,
    /// The peer's relay address, if it has one. The field is required, but may be `null`.
    #[serde(deserialize_with = "required_option")]
    pub relay: Option<Address>,
}

/// A bundle's swarm counts: the answer to `GET /tempest/v1/bundles/{bundle}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BundleStats {
    /// The number of complete peers the tracker knows of.
    pub complete: u64,
    /// The number of incomplete peers the tracker knows of.
    pub incomplete: u64,
}

/// The body of an error response (HTTP 4xx or 5xx).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorResponse {
    /// A machine-readable code.
    pub error: ErrorCode,
    /// A human-readable explanation.
    pub message: String,
    /// When to try again, in seconds. A client must honor it when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
}

/// A tracker error code.
///
/// Codes this version doesn't define are kept as [`ErrorCode::Other`], so a newer tracker's codes
/// still parse; a client treats them by their HTTP status.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// A field is missing or malformed.
    InvalidRequest,
    /// The client is announcing too often.
    RateLimited,
    /// This tracker doesn't serve this bundle.
    BundleNotAllowed,
    /// The tracker is temporarily unable to serve requests.
    Unavailable,
    /// A code this version doesn't define.
    Other(String),
}

impl ErrorCode {
    /// The code's text.
    pub fn as_str(&self) -> &str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::RateLimited => "rate_limited",
            Self::BundleNotAllowed => "bundle_not_allowed",
            Self::Unavailable => "unavailable",
            Self::Other(code) => code,
        }
    }

    /// The HTTP status a tracker typically sends with the code, if it is a known one.
    pub fn typical_status(&self) -> Option<u16> {
        match self {
            Self::InvalidRequest => Some(400),
            Self::RateLimited => Some(429),
            Self::BundleNotAllowed => Some(403),
            Self::Unavailable => Some(503),
            Self::Other(_) => None,
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ErrorCode {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "invalid_request" => Self::InvalidRequest,
            "rate_limited" => Self::RateLimited,
            "bundle_not_allowed" => Self::BundleNotAllowed,
            "unavailable" => Self::Unavailable,
            other => Self::Other(other.to_owned()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const BUNDLE: &str = "3f9a000000000000000000000000000000000000000000000000000000000000";
    const PEER: &str = "7c21000000000000000000000000000000000000000000000000000000000000";
    const LISTING_SIG: &str = "ad66ac47ca8e640246582dcc0000087592145360a7a7f3b7385efd630f83a2aa\
                               706008afbc7f1499e94be591e8e4948f372ed9d26c7077f4a1c8205e86acdd0d";

    fn request() -> serde_json::Value {
        json!({
            "bundle": BUNDLE,
            "peer": PEER,
            "port": 4433,
            "relay": null,
            "event": "started",
            "complete": false
        })
    }

    #[test]
    fn spec_announce_request_parses_with_defaults() {
        let parsed: AnnounceRequest = serde_json::from_value(request()).unwrap();
        assert_eq!(
            parsed,
            AnnounceRequest {
                bundle: BUNDLE.parse().unwrap(),
                peer: PEER.parse().unwrap(),
                port: 4433,
                addrs: vec![],
                relay: None,
                event: AnnounceEvent::Started,
                complete: false,
                num_want: 50,
                listing: None,
            }
        );
    }

    /// The test vector of `protocol/trackers.md`: RFC 8032's first test key signs the listing
    /// message of bundle `3f9a00…`. The signature is checked by the tracker's tests.
    #[test]
    fn spec_listing_parses() {
        let mut with_listing = request();
        with_listing["listing"] = json!({ "sig": LISTING_SIG });
        let parsed: AnnounceRequest = serde_json::from_value(with_listing).unwrap();
        assert_eq!(parsed.listing.unwrap().sig.to_string(), LISTING_SIG);
        assert_eq!(
            hex::encode(listing_message(&parsed.bundle)),
            "74656d70657374206c697374696e67207631003f9a\
             000000000000000000000000000000000000000000000000000000000000"
        );
        // Absent when the peer didn't opt in, and then not serialized.
        let plain: AnnounceRequest = serde_json::from_value(request()).unwrap();
        assert!(!serde_json::to_string(&plain).unwrap().contains("listing"));
    }

    #[test]
    fn malformed_listings_are_rejected() {
        for listing in [
            json!(true),
            json!({}),
            json!({ "sig": LISTING_SIG.to_uppercase() }),
            json!({ "sig": &LISTING_SIG[2..] }),
        ] {
            let mut bad = request();
            bad["listing"] = listing.clone();
            assert!(
                serde_json::from_value::<AnnounceRequest>(bad).is_err(),
                "{listing}"
            );
        }
    }

    #[test]
    fn spec_announce_response_parses() {
        let parsed: AnnounceResponse = serde_json::from_value(json!({
            "interval_secs": 300,
            "peers": [
                { "peer": PEER, "addrs": ["198.51.100.9:4433"], "relay": null }
            ],
            "complete": 1,
            "incomplete": 4,
            "your_ip": "203.0.113.5",
            "relays": []
        }))
        .unwrap();
        assert_eq!(parsed.interval_secs, 300);
        assert_eq!(parsed.peers[0].addrs[0].to_string(), "198.51.100.9:4433");
        assert_eq!(parsed.your_ip.to_string(), "203.0.113.5");
        assert_eq!((parsed.complete, parsed.incomplete), (1, 4));
    }

    #[test]
    fn spec_bundle_stats_parse() {
        let parsed: BundleStats =
            serde_json::from_str(r#"{ "complete": 1, "incomplete": 4 }"#).unwrap();
        assert_eq!(
            parsed,
            BundleStats {
                complete: 1,
                incomplete: 4
            }
        );
    }

    #[test]
    fn relay_is_required_but_nullable() {
        let mut missing = request();
        missing.as_object_mut().unwrap().remove("relay");
        assert!(serde_json::from_value::<AnnounceRequest>(missing).is_err());

        let mut set = request();
        set["relay"] = json!("[2001:db8::1]:40007");
        let parsed: AnnounceRequest = serde_json::from_value(set).unwrap();
        assert_eq!(parsed.relay, Some("[2001:db8::1]:40007".parse().unwrap()));
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let mut extra = request();
        extra["future"] = json!({ "nested": [1, 2] });
        assert!(serde_json::from_value::<AnnounceRequest>(extra).is_ok());
    }

    #[test]
    fn malformed_values_are_rejected() {
        let cases = [
            ("bundle", json!(BUNDLE.to_uppercase())),
            ("peer", json!("7c21")),
            ("port", json!(65536)),
            ("addrs", json!(["203.0.113.5"])),
            ("relay", json!("relay.example.org:4775")),
            ("event", json!("paused")),
            ("complete", json!("yes")),
            ("num_want", json!(-1)),
        ];
        for (field, value) in cases {
            let mut bad = request();
            bad[field] = value;
            assert!(
                serde_json::from_value::<AnnounceRequest>(bad).is_err(),
                "{field}"
            );
        }
    }

    #[test]
    fn requests_round_trip() {
        let parsed: AnnounceRequest = serde_json::from_value(request()).unwrap();
        let text = serde_json::to_string(&parsed).unwrap();
        assert_eq!(
            serde_json::from_str::<AnnounceRequest>(&text).unwrap(),
            parsed
        );
    }

    #[test]
    fn errors_parse_with_and_without_retry() {
        let parsed: ErrorResponse = serde_json::from_value(json!({
            "error": "rate_limited",
            "message": "slow down",
            "retry_after_secs": 60
        }))
        .unwrap();
        assert_eq!(parsed.error, ErrorCode::RateLimited);
        assert_eq!(parsed.retry_after_secs, Some(60));

        let parsed: ErrorResponse =
            serde_json::from_value(json!({ "error": "teapot", "message": "" })).unwrap();
        assert_eq!(parsed.error, ErrorCode::Other("teapot".into()));
        assert_eq!(parsed.retry_after_secs, None);
        assert_eq!(
            serde_json::to_value(&parsed).unwrap(),
            json!({ "error": "teapot", "message": "" })
        );
    }

    #[test]
    fn error_codes_have_their_typical_status() {
        for (code, status) in [
            ("invalid_request", 400),
            ("rate_limited", 429),
            ("bundle_not_allowed", 403),
            ("unavailable", 503),
        ] {
            let parsed: ErrorCode = code.parse().unwrap();
            assert_eq!(parsed.as_str(), code);
            assert_eq!(parsed.typical_status(), Some(status));
        }
    }

    #[test]
    fn urls_join_the_base_and_prefix() {
        let bundle: BundleId = BUNDLE.parse().unwrap();
        assert_eq!(
            announce_url("https://tracker.example.org"),
            "https://tracker.example.org/tempest/v1/announce"
        );
        assert_eq!(
            bundle_stats_url("https://example.org/tracker/", &bundle),
            format!("https://example.org/tracker/tempest/v1/bundles/{BUNDLE}")
        );
    }

    #[test]
    fn relays_parse_as_identities() {
        let key = PEER;
        let parsed: AnnounceResponse = serde_json::from_value(json!({
            "interval_secs": 300,
            "peers": [],
            "complete": 0,
            "incomplete": 0,
            "your_ip": "2001:db8::5",
            "relays": [format!("{key}@relay.example.org")]
        }))
        .unwrap();
        assert_eq!(parsed.relays[0].port, 4775);

        let bad = json!({
            "interval_secs": 300,
            "peers": [],
            "complete": 0,
            "incomplete": 0,
            "your_ip": "2001:db8::5",
            "relays": ["relay.example.org"]
        });
        assert!(serde_json::from_value::<AnnounceResponse>(bad).is_err());
    }
}
