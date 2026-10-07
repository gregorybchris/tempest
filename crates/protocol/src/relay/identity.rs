//! Relay identities: `<relay key>@<host>:<port>` (`protocol/relays.md`, Relay identity).

use std::fmt;
use std::net::{IpAddr, Ipv6Addr};
use std::str::FromStr;

use crate::hash::Hash;

/// The relay port used when an identity doesn't name one.
pub const DEFAULT_RELAY_PORT: u16 = 4775;

/// The longest DNS name, in its usual text form without a trailing dot.
const MAX_DOMAIN_LEN: usize = 253;

/// The longest label of a DNS name.
const MAX_LABEL_LEN: usize = 63;

/// How to reach a relay and check that it is the right one.
///
/// As text, `<key>@<host>:<port>`, with IPv6 hosts in brackets. Parsing accepts a missing
/// `:<port>` as [`DEFAULT_RELAY_PORT`]; formatting always writes the port.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RelayIdentity {
    /// The relay's Ed25519 public key, written like a peer ID.
    pub key: Hash,
    /// The relay's DNS name or IP address.
    pub host: RelayHost,
    /// The relay's UDP port.
    pub port: u16,
}

/// A relay's host.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RelayHost {
    /// An IP address. IPv4-mapped IPv6 addresses are stored as IPv4.
    Ip(IpAddr),
    /// A DNS name in ASCII form (internationalized names as punycode), lowercased.
    Domain(String),
}

impl fmt::Display for RelayHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ip(IpAddr::V6(ip)) => write!(f, "[{ip}]"),
            Self::Ip(IpAddr::V4(ip)) => ip.fmt(f),
            Self::Domain(name) => f.write_str(name),
        }
    }
}

impl fmt::Display for RelayIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}:{}", self.key, self.host, self.port)
    }
}

/// Why text could not be parsed as a relay identity.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseRelayIdentityError {
    /// There was no `@` between key and host.
    #[error("a relay identity must be <key>@<host>:<port>")]
    MissingAt,
    /// The key was not 64 lowercase hexadecimal characters.
    #[error("relay key is not 64 lowercase hexadecimal characters")]
    InvalidKey,
    /// The host was neither an IP address nor a valid DNS name.
    #[error("relay host is not a DNS name, IPv4 address or bracketed IPv6 address")]
    InvalidHost,
    /// The port was not a number from 1 to 65535 without leading zeros.
    #[error("relay port is not a number from 1 to 65535")]
    InvalidPort,
}

impl FromStr for RelayIdentity {
    type Err = ParseRelayIdentityError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use ParseRelayIdentityError::*;

        let (key, host_port) = s.split_once('@').ok_or(MissingAt)?;
        let key = key.parse().map_err(|_| InvalidKey)?;

        let (host, port) = if let Some(bracketed) = host_port.strip_prefix('[') {
            let (ip, rest) = bracketed.split_once(']').ok_or(InvalidHost)?;
            let ip: Ipv6Addr = ip.parse().map_err(|_| InvalidHost)?;
            let ip = ip.to_ipv4_mapped().map_or(IpAddr::V6(ip), IpAddr::V4);
            let port = match rest {
                "" => None,
                _ => Some(rest.strip_prefix(':').ok_or(InvalidHost)?),
            };
            (RelayHost::Ip(ip), port)
        } else {
            let (host, port) = match host_port.split_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (host_port, None),
            };
            (parse_host(host)?, port)
        };
        let port = port.map_or(Ok(DEFAULT_RELAY_PORT), parse_port)?;
        Ok(Self { key, host, port })
    }
}

/// Parses an unbracketed host: an IPv4 address or a DNS name.
fn parse_host(host: &str) -> Result<RelayHost, ParseRelayIdentityError> {
    if let Ok(ip) = host.parse() {
        return Ok(RelayHost::Ip(IpAddr::V4(ip)));
    }
    let valid_label = |label: &str| {
        (1..=MAX_LABEL_LEN).contains(&label.len())
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    };
    // A name whose last label is all digits would read as a malformed IPv4 address.
    let numeric_tld = host
        .rsplit('.')
        .next()
        .is_some_and(|tld| tld.bytes().all(|b| b.is_ascii_digit()));
    if host.len() > MAX_DOMAIN_LEN || !host.split('.').all(valid_label) || numeric_tld {
        return Err(ParseRelayIdentityError::InvalidHost);
    }
    Ok(RelayHost::Domain(host.to_ascii_lowercase()))
}

fn parse_port(port: &str) -> Result<u16, ParseRelayIdentityError> {
    let canonical =
        !port.is_empty() && !port.starts_with('0') && port.bytes().all(|b| b.is_ascii_digit());
    canonical
        .then(|| port.parse().ok())
        .flatten()
        .ok_or(ParseRelayIdentityError::InvalidPort)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "9d1e000000000000000000000000000000000000000000000000000000000001";

    fn parse(s: &str) -> Result<RelayIdentity, ParseRelayIdentityError> {
        s.parse()
    }

    #[test]
    fn parses_and_formats_each_host_form() {
        for host_port in [
            "relay.example.org:4775",
            "localhost:1",
            "198.51.100.1:65535",
            "[2001:db8::1]:4775",
        ] {
            let text = format!("{KEY}@{host_port}");
            assert_eq!(parse(&text).unwrap().to_string(), text);
        }
    }

    #[test]
    fn port_defaults_to_4775() {
        for host in ["relay.example.org", "198.51.100.1", "[2001:db8::1]"] {
            let identity = parse(&format!("{KEY}@{host}")).unwrap();
            assert_eq!(identity.port, DEFAULT_RELAY_PORT);
        }
    }

    #[test]
    fn normalizes_case_and_mapped_ipv4() {
        let identity = parse(&format!("{KEY}@Relay.Example.ORG")).unwrap();
        assert_eq!(identity.host, RelayHost::Domain("relay.example.org".into()));
        let identity = parse(&format!("{KEY}@[::ffff:198.51.100.1]:9")).unwrap();
        assert_eq!(identity.to_string(), format!("{KEY}@198.51.100.1:9"));
    }

    #[test]
    fn rejects_malformed_identities() {
        use ParseRelayIdentityError::*;
        let long_label = "a".repeat(64);
        let long_name = ["abc"; 64].join(".");
        let cases = [
            ("relay.example.org".to_string(), MissingAt),
            (
                format!("{}@relay.example.org", KEY.to_uppercase()),
                InvalidKey,
            ),
            ("abc@relay.example.org".into(), InvalidKey),
            (format!("{KEY}@"), InvalidHost),
            (format!("{KEY}@2001:db8::1"), InvalidHost),
            (format!("{KEY}@[2001:db8::1"), InvalidHost),
            (format!("{KEY}@[2001:db8::1]4775"), InvalidHost),
            (format!("{KEY}@[198.51.100.1]"), InvalidHost),
            (format!("{KEY}@-relay.org"), InvalidHost),
            (format!("{KEY}@relay..org"), InvalidHost),
            (format!("{KEY}@relay.org."), InvalidHost),
            (format!("{KEY}@relay_1.org"), InvalidHost),
            (format!("{KEY}@{long_label}.org"), InvalidHost),
            (format!("{KEY}@{long_name}"), InvalidHost),
            (format!("{KEY}@1.2.3"), InvalidHost),
            (format!("{KEY}@héllo.org"), InvalidHost),
            (format!("{KEY}@relay.org:"), InvalidPort),
            (format!("{KEY}@relay.org:0"), InvalidPort),
            (format!("{KEY}@relay.org:04775"), InvalidPort),
            (format!("{KEY}@relay.org:+1"), InvalidPort),
            (format!("{KEY}@relay.org:65536"), InvalidPort),
            (format!("{KEY}@relay.org:1:2"), InvalidPort),
        ];
        for (text, error) in cases {
            assert_eq!(parse(&text), Err(error), "{text}");
        }
    }
}
