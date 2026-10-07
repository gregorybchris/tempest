//! `tempest:` links (`protocol/discovery.md`, Links).
//!
//! A link is a URI whose query parameters name a bundle and, optionally, where to find it:
//! `tempest:?bundle=…&peer=…&addr=203.0.113.5:4433&name=Holiday%20Photos`.

use std::fmt;
use std::net::SocketAddr;
use std::str::FromStr;

use crate::address::Address;
use crate::hash::Hash;
use crate::manifest::BundleId;

const SCHEME: &str = "tempest:";

/// A parsed `tempest:` link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// The bundle ID.
    pub bundle: BundleId,
    /// The sharing peer's peer ID.
    pub peer: Option<Hash>,
    /// The sharing peer's direct addresses. Only meaningful with `peer`.
    pub addrs: Vec<Address>,
    /// The sharing peer's relay address. Only meaningful with `peer`.
    pub relay: Option<Address>,
    /// URLs of trackers that know about the bundle.
    pub trackers: Vec<String>,
    /// An unverified display-name hint, to show until the manifest arrives.
    pub name: Option<String>,
}

impl Link {
    /// A link that names only `bundle`.
    pub fn new(bundle: BundleId) -> Self {
        Self {
            bundle,
            peer: None,
            addrs: Vec::new(),
            relay: None,
            trackers: Vec::new(),
            name: None,
        }
    }
}

/// Why text could not be parsed as a link.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseLinkError {
    /// The text did not start with `tempest:?`.
    #[error("not a tempest: link")]
    NotALink,
    /// A `%` was not followed by two hexadecimal digits.
    #[error("invalid percent-encoding")]
    InvalidPercentEncoding,
    /// A decoded key or value was not valid UTF-8.
    #[error("percent-decoded text is not valid UTF-8")]
    InvalidUtf8,
    /// The required `bundle` parameter was missing.
    #[error("missing bundle parameter")]
    MissingBundle,
    /// A parameter that may appear at most once appeared again.
    #[error("parameter {0} appears more than once")]
    Repeated(&'static str),
    /// A known parameter had no `=value`.
    #[error("parameter {0} has no value")]
    MissingValue(&'static str),
    /// A `bundle` or `peer` value was not 64 lowercase hexadecimal characters.
    #[error("parameter {0} is not 64 lowercase hexadecimal characters")]
    InvalidHash(&'static str),
    /// An `addr` or `relay` value was not `ip:port`.
    #[error("parameter {0} is not ip:port")]
    InvalidAddress(&'static str),
    /// An `addr` or `relay` parameter was given without `peer`.
    #[error("addr and relay need a peer parameter")]
    AddressWithoutPeer,
    /// A `tracker` value was empty.
    #[error("tracker URL is empty")]
    EmptyTracker,
}

impl fmt::Display for Link {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{SCHEME}?bundle={}", self.bundle)?;
        if let Some(peer) = &self.peer {
            write!(f, "&peer={peer}")?;
        }
        for addr in &self.addrs {
            write!(f, "&addr={}", Encoded(&addr.to_string()))?;
        }
        if let Some(relay) = &self.relay {
            write!(f, "&relay={}", Encoded(&relay.to_string()))?;
        }
        for tracker in &self.trackers {
            write!(f, "&tracker={}", Encoded(tracker))?;
        }
        if let Some(name) = &self.name {
            write!(f, "&name={}", Encoded(name))?;
        }
        Ok(())
    }
}

impl FromStr for Link {
    type Err = ParseLinkError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // The scheme is case-insensitive (RFC 3986), and a fragment is not part of the query.
        let rest = s
            .get(..SCHEME.len())
            .filter(|scheme| scheme.eq_ignore_ascii_case(SCHEME))
            .and_then(|_| s[SCHEME.len()..].strip_prefix('?'))
            .ok_or(ParseLinkError::NotALink)?;
        let query = rest.split_once('#').map_or(rest, |(query, _)| query);

        let mut bundle = None;
        let mut link = Link::new(Hash::ZERO);
        for pair in query.split('&').filter(|pair| !pair.is_empty()) {
            let (key, value) = match pair.split_once('=') {
                Some((key, value)) => (percent_decode(key)?, Some(value)),
                None => (percent_decode(pair)?, None),
            };
            let key = match key.as_str() {
                "bundle" => "bundle",
                "peer" => "peer",
                "addr" => "addr",
                "relay" => "relay",
                "tracker" => "tracker",
                "name" => "name",
                _ => continue,
            };
            let value = percent_decode(value.ok_or(ParseLinkError::MissingValue(key))?)?;
            match key {
                "bundle" => set_once(&mut bundle, key, parse_hash(key, &value)?)?,
                "peer" => set_once(&mut link.peer, key, parse_hash(key, &value)?)?,
                "addr" => link.addrs.push(parse_address(key, &value)?),
                "relay" => set_once(&mut link.relay, key, parse_address(key, &value)?)?,
                "tracker" if value.is_empty() => return Err(ParseLinkError::EmptyTracker),
                "tracker" => link.trackers.push(value),
                _ => set_once(&mut link.name, key, value)?,
            }
        }

        link.bundle = bundle.ok_or(ParseLinkError::MissingBundle)?;
        if link.peer.is_none() && (!link.addrs.is_empty() || link.relay.is_some()) {
            return Err(ParseLinkError::AddressWithoutPeer);
        }
        Ok(link)
    }
}

fn set_once<T>(slot: &mut Option<T>, key: &'static str, value: T) -> Result<(), ParseLinkError> {
    match slot {
        Some(_) => Err(ParseLinkError::Repeated(key)),
        None => {
            *slot = Some(value);
            Ok(())
        }
    }
}

fn parse_hash(key: &'static str, value: &str) -> Result<Hash, ParseLinkError> {
    value.parse().map_err(|_| ParseLinkError::InvalidHash(key))
}

fn parse_address(key: &'static str, value: &str) -> Result<Address, ParseLinkError> {
    value
        .parse::<SocketAddr>()
        .map(Address::from)
        .map_err(|_| ParseLinkError::InvalidAddress(key))
}

/// Whether `byte` may appear unencoded in a query value: RFC 3986's `pchar`, plus `/` and `?`,
/// minus the characters that delimit parameters (`&`, `=`) or that some parsers treat specially
/// (`+`).
fn is_query_safe(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"-._~!$'()*,;:@/?".contains(&byte)
}

/// Formats text percent-encoded for a query value.
struct Encoded<'a>(&'a str);

impl fmt::Display for Encoded<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for &byte in self.0.as_bytes() {
            if is_query_safe(byte) {
                write!(f, "{}", byte as char)?;
            } else {
                write!(f, "%{byte:02X}")?;
            }
        }
        Ok(())
    }
}

/// Decodes `%XX` escapes. Any other character stands for itself, including `+`.
fn percent_decode(s: &str) -> Result<String, ParseLinkError> {
    let hex = |byte: u8| (byte as char).to_digit(16);
    let mut bytes = Vec::with_capacity(s.len());
    let mut rest = s.as_bytes();
    while let Some((&byte, tail)) = rest.split_first() {
        rest = tail;
        if byte != b'%' {
            bytes.push(byte);
            continue;
        }
        let (Some(high), Some(low)) = (
            tail.first().copied().and_then(hex),
            tail.get(1).copied().and_then(hex),
        ) else {
            return Err(ParseLinkError::InvalidPercentEncoding);
        };
        bytes.push((high * 16 + low) as u8);
        rest = &tail[2..];
    }
    String::from_utf8(bytes).map_err(|_| ParseLinkError::InvalidUtf8)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUNDLE: &str = "207855f013f5a4066c9987ee3fa4b0ae63c8eeee02035634587286457cf5b85a";
    const PEER: &str = "7c21000000000000000000000000000000000000000000000000000000000001";

    fn parse(s: &str) -> Result<Link, ParseLinkError> {
        s.parse()
    }

    #[test]
    fn minimal_link_round_trips() {
        let link = Link::new(BUNDLE.parse().unwrap());
        let text = link.to_string();
        assert_eq!(text, format!("tempest:?bundle={BUNDLE}"));
        assert_eq!(parse(&text), Ok(link));
    }

    #[test]
    fn full_link() {
        let text = format!(
            "tempest:?bundle={BUNDLE}&peer={PEER}&addr=203.0.113.5:4433\
             &addr=%5B2001:db8::1%5D:4433&relay=198.51.100.1:40007\
             &tracker=https://tracker.example.org/announce?x%3D1%26y%3D2&name=Holiday%20Photos"
        );
        let link = parse(&text).unwrap();
        assert_eq!(link.peer, Some(PEER.parse().unwrap()));
        assert_eq!(
            link.addrs
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["203.0.113.5:4433", "[2001:db8::1]:4433"]
        );
        assert_eq!(link.relay.unwrap().to_string(), "198.51.100.1:40007");
        assert_eq!(
            link.trackers,
            ["https://tracker.example.org/announce?x=1&y=2"]
        );
        assert_eq!(link.name.as_deref(), Some("Holiday Photos"));
        assert_eq!(link.to_string(), text);
    }

    #[test]
    fn unencoded_brackets_and_unknown_parameters_are_accepted() {
        let link = parse(&format!(
            "TEMPEST:?x=1&bundle={BUNDLE}&peer={PEER}&addr=[::1]:1&future&#fragment"
        ))
        .unwrap();
        assert_eq!(link.addrs[0].to_string(), "[::1]:1");
    }

    #[test]
    fn plus_is_not_a_space() {
        let link = parse(&format!("tempest:?bundle={BUNDLE}&name=a+b")).unwrap();
        assert_eq!(link.name.as_deref(), Some("a+b"));
        assert!(link.to_string().ends_with("name=a%2Bb"));
    }

    #[test]
    fn invalid_links() {
        use ParseLinkError::*;
        let with = |rest: &str| parse(&format!("tempest:?bundle={BUNDLE}{rest}"));
        assert_eq!(parse("magnet:?xt=1"), Err(NotALink));
        assert_eq!(parse("tempest:bundle=1"), Err(NotALink));
        assert_eq!(parse("tempest:?name=x"), Err(MissingBundle));
        assert_eq!(with(&format!("&bundle={BUNDLE}")), Err(Repeated("bundle")));
        assert_eq!(
            parse(&format!("tempest:?bundle={}", BUNDLE.to_uppercase())),
            Err(InvalidHash("bundle"))
        );
        assert_eq!(with("&peer"), Err(MissingValue("peer")));
        assert_eq!(with("&addr=1.2.3.4:5"), Err(AddressWithoutPeer));
        assert_eq!(with("&relay=1.2.3.4:5"), Err(AddressWithoutPeer));
        assert_eq!(
            with(&format!("&peer={PEER}&addr=::1:5")),
            Err(InvalidAddress("addr"))
        );
        assert_eq!(with("&name=%4"), Err(InvalidPercentEncoding));
        assert_eq!(with("&name=%+1"), Err(InvalidPercentEncoding));
        assert_eq!(with("&name=%C3%28"), Err(InvalidUtf8));
        assert_eq!(with("&tracker="), Err(EmptyTracker));
        assert_eq!(with("&name=a&name=b"), Err(Repeated("name")));
    }
}
