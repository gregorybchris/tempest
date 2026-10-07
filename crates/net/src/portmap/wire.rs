//! The messages of PCP (RFC 6887) and NAT-PMP (RFC 6886), the two protocols that ask a router
//! for a port mapping over UDP, apart from any I/O.
//!
//! Only what port mapping needs is here: PCP's MAP opcode for UDP, and NAT-PMP's external
//! address and UDP mapping requests. Responses come from the router, so their parsers accept
//! any bytes.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// The UDP port a router listens on for both PCP and NAT-PMP requests.
pub const SERVER_PORT: u16 = 5351;

/// PCP's version number. NAT-PMP's is 0.
const PCP_VERSION: u8 = 2;
/// PCP's MAP opcode.
const PCP_MAP: u8 = 1;
/// The bit set in a PCP or NAT-PMP opcode that marks a response.
const PCP_RESPONSE: u8 = 0x80;
const NATPMP_RESPONSE: u8 = 128;
/// UDP's IANA protocol number, which PCP's MAP carries.
const UDP: u8 = 17;
/// NAT-PMP's opcodes.
const NATPMP_ADDRESS: u8 = 0;
const NATPMP_MAP_UDP: u8 = 1;

/// A PCP header's length, in both directions.
const PCP_HEADER_LEN: usize = 24;
/// The length of a PCP MAP request or response without options.
pub const PCP_MAP_LEN: usize = PCP_HEADER_LEN + 36;
/// The longest PCP message (RFC 6887, section 7).
const PCP_MAX_LEN: usize = 1100;

/// The nonce that ties a PCP mapping's requests and responses together.
pub type Nonce = [u8; 12];

/// A PCP MAP request for UDP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PcpMapRequest {
    /// The address the request is sent from, which the router checks.
    pub client: Ipv4Addr,
    /// The mapping's nonce, the same for every request about it.
    pub nonce: Nonce,
    /// The port to map to.
    pub internal_port: u16,
    /// The external port to ask for, or 0 for any.
    pub external_port: u16,
    /// The external address to ask for, or unspecified for any.
    pub external_ip: Ipv4Addr,
    /// How long the mapping should last, in seconds. 0 removes it.
    pub lifetime: u32,
}

impl PcpMapRequest {
    /// The request's bytes.
    pub fn encode(&self) -> [u8; PCP_MAP_LEN] {
        let mut bytes = [0; PCP_MAP_LEN];
        bytes[0] = PCP_VERSION;
        bytes[1] = PCP_MAP;
        bytes[4..8].copy_from_slice(&self.lifetime.to_be_bytes());
        bytes[8..24].copy_from_slice(&self.client.to_ipv6_mapped().octets());
        bytes[24..36].copy_from_slice(&self.nonce);
        bytes[36] = UDP;
        bytes[40..42].copy_from_slice(&self.internal_port.to_be_bytes());
        bytes[42..44].copy_from_slice(&self.external_port.to_be_bytes());
        bytes[44..60].copy_from_slice(&self.external_ip.to_ipv6_mapped().octets());
        bytes
    }
}

/// A router's answer to a PCP request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcpResponse {
    /// The router doesn't speak this version of PCP. A NAT-PMP router answers like this, in
    /// NAT-PMP.
    UnsupportedVersion,
    /// An answer to a MAP request.
    Map(PcpMapResponse),
}

/// A router's answer to a PCP MAP request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PcpMapResponse {
    /// Whether the mapping was made, and if not, why.
    pub result: ResultCode,
    /// How long the mapping lasts, in seconds, or on failure how long the failure will last.
    pub lifetime: u32,
    /// The request's nonce.
    pub nonce: Nonce,
    /// The request's protocol number.
    pub protocol: u8,
    /// The request's internal port.
    pub internal_port: u16,
    /// The external port mapped.
    pub external_port: u16,
    /// The external address mapped.
    pub external_ip: IpAddr,
}

/// Parses a PCP response. Options after the MAP response are ignored.
pub fn parse_pcp(bytes: &[u8]) -> Result<PcpResponse, ParseError> {
    let [version, opcode, ..] = *bytes else {
        return Err(ParseError::Short);
    };
    if version == 0 {
        // NAT-PMP's Unsupported Version response: a router that only speaks NAT-PMP.
        return Ok(PcpResponse::UnsupportedVersion);
    }
    if version != PCP_VERSION {
        return Err(ParseError::Version(version));
    }
    if bytes.len() < PCP_HEADER_LEN {
        return Err(ParseError::Short);
    }
    if bytes.len() > PCP_MAX_LEN || !bytes.len().is_multiple_of(4) {
        return Err(ParseError::Length(bytes.len()));
    }
    let result = ResultCode(bytes[3]);
    if result == ResultCode::UNSUPPORTED_VERSION {
        return Ok(PcpResponse::UnsupportedVersion);
    }
    if opcode != PCP_RESPONSE | PCP_MAP {
        return Err(ParseError::Opcode(opcode));
    }
    if bytes.len() < PCP_MAP_LEN {
        return Err(ParseError::Short);
    }
    let external = Ipv6Addr::from(array::<16>(&bytes[44..60]));
    Ok(PcpResponse::Map(PcpMapResponse {
        result,
        lifetime: u32::from_be_bytes(array(&bytes[4..8])),
        nonce: array(&bytes[24..36]),
        protocol: bytes[36],
        internal_port: u16::from_be_bytes(array(&bytes[40..42])),
        external_port: u16::from_be_bytes(array(&bytes[42..44])),
        external_ip: external
            .to_ipv4_mapped()
            .map_or(IpAddr::V6(external), IpAddr::V4),
    }))
}

/// A NAT-PMP request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NatPmpRequest {
    /// Asks for the router's external address.
    Address,
    /// Asks for a UDP mapping.
    Map {
        /// The port to map to.
        internal_port: u16,
        /// The external port to ask for, or 0 for any.
        external_port: u16,
        /// How long the mapping should last, in seconds. 0 removes it.
        lifetime: u32,
    },
}

impl NatPmpRequest {
    /// The request's bytes.
    pub fn encode(&self) -> Vec<u8> {
        match *self {
            Self::Address => vec![0, NATPMP_ADDRESS],
            Self::Map {
                internal_port,
                external_port,
                lifetime,
            } => {
                let mut bytes = vec![0, NATPMP_MAP_UDP, 0, 0];
                bytes.extend_from_slice(&internal_port.to_be_bytes());
                bytes.extend_from_slice(&external_port.to_be_bytes());
                bytes.extend_from_slice(&lifetime.to_be_bytes());
                bytes
            }
        }
    }
}

/// A router's answer to a NAT-PMP request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NatPmpResponse {
    /// The router's external address.
    Address {
        /// The address.
        ip: Ipv4Addr,
    },
    /// A UDP mapping.
    Map {
        /// The request's internal port.
        internal_port: u16,
        /// The external port mapped.
        external_port: u16,
        /// How long the mapping lasts, in seconds.
        lifetime: u32,
    },
    /// The request failed.
    Failed {
        /// The request's opcode.
        opcode: u8,
        /// Why.
        result: ResultCode,
    },
}

/// Parses a NAT-PMP response.
pub fn parse_natpmp(bytes: &[u8]) -> Result<NatPmpResponse, ParseError> {
    if bytes.len() < 8 {
        return Err(ParseError::Short);
    }
    if bytes[0] != 0 {
        return Err(ParseError::Version(bytes[0]));
    }
    let Some(opcode) = bytes[1].checked_sub(NATPMP_RESPONSE) else {
        return Err(ParseError::Opcode(bytes[1]));
    };
    // NAT-PMP's result codes are 16 bits; those above 255 are unknown either way.
    let result = u16::from_be_bytes(array(&bytes[2..4]));
    if result != 0 {
        let result = ResultCode(u8::try_from(result).unwrap_or(u8::MAX));
        return Ok(NatPmpResponse::Failed { opcode, result });
    }
    match opcode {
        NATPMP_ADDRESS if bytes.len() >= 12 => Ok(NatPmpResponse::Address {
            ip: Ipv4Addr::from(array::<4>(&bytes[8..12])),
        }),
        NATPMP_MAP_UDP if bytes.len() >= 16 => Ok(NatPmpResponse::Map {
            internal_port: u16::from_be_bytes(array(&bytes[8..10])),
            external_port: u16::from_be_bytes(array(&bytes[10..12])),
            lifetime: u32::from_be_bytes(array(&bytes[12..16])),
        }),
        NATPMP_ADDRESS | NATPMP_MAP_UDP => Err(ParseError::Short),
        _ => Err(ParseError::Opcode(bytes[1])),
    }
}

/// A PCP or NAT-PMP result code. Both protocols share the codes up to 5, apart from 1's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResultCode(pub u8);

impl ResultCode {
    /// The request was carried out.
    pub const SUCCESS: Self = Self(0);
    /// The router speaks another version of the protocol.
    pub const UNSUPPORTED_VERSION: Self = Self(1);
}

impl fmt::Display for ResultCode {
    /// PCP's name for the code, which describes NAT-PMP's codes too.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self.0 {
            0 => "success",
            1 => "unsupported version",
            2 => "not authorized",
            3 => "malformed request",
            4 => "unsupported opcode",
            5 => "unsupported option",
            6 => "malformed option",
            7 => "network failure",
            8 => "no resources",
            9 => "unsupported protocol",
            10 => "user exceeded quota",
            11 => "cannot provide external address",
            12 => "address mismatch",
            13 => "excessive remote peers",
            code => return write!(f, "result code {code}"),
        };
        f.write_str(name)
    }
}

/// Why a router's response could not be parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    /// It ends too early.
    #[error("the response is too short")]
    Short,
    /// Its length is not a multiple of 4 or exceeds 1,100 bytes.
    #[error("a PCP response cannot be {0} bytes long")]
    Length(usize),
    /// It is in an unknown version of the protocol.
    #[error("unknown version {0}")]
    Version(u8),
    /// It answers a request that wasn't made.
    #[error("unexpected opcode {0}")]
    Opcode(u8),
}

/// The first `N` bytes of `bytes`, which has at least that many.
fn array<const N: usize>(bytes: &[u8]) -> [u8; N] {
    bytes[..N]
        .try_into()
        .expect("the caller checked the length")
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const NONCE: Nonce = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];

    /// A PCP MAP response as a router would send it, with `options` bytes of options after.
    fn pcp_response(result: u8, external: Ipv4Addr, options: usize) -> Vec<u8> {
        let mut bytes = vec![0; PCP_MAP_LEN + options];
        bytes[0] = 2;
        bytes[1] = 0x81;
        bytes[3] = result;
        bytes[4..8].copy_from_slice(&7200u32.to_be_bytes());
        bytes[8..12].copy_from_slice(&99u32.to_be_bytes());
        bytes[24..36].copy_from_slice(&NONCE);
        bytes[36] = 17;
        bytes[40..42].copy_from_slice(&4773u16.to_be_bytes());
        bytes[42..44].copy_from_slice(&40000u16.to_be_bytes());
        bytes[44..60].copy_from_slice(&external.to_ipv6_mapped().octets());
        bytes
    }

    #[test]
    fn a_pcp_map_request_follows_rfc_6887() {
        let request = PcpMapRequest {
            client: Ipv4Addr::new(192, 168, 1, 20),
            nonce: NONCE,
            internal_port: 4773,
            external_port: 4773,
            external_ip: Ipv4Addr::UNSPECIFIED,
            lifetime: 7200,
        };
        let expected = hex::decode(concat!(
            "02010000",                         // version 2, MAP request, reserved
            "00001c20",                         // lifetime 7200
            "00000000000000000000ffffc0a80114", // client 192.168.1.20, IPv4-mapped
            "0102030405060708090a0b0c",         // nonce
            "11000000",                         // UDP, reserved
            "12a512a5",                         // internal and suggested external port 4773
            "00000000000000000000ffff00000000", // any external address
        ))
        .unwrap();
        assert_eq!(request.encode().to_vec(), expected);
    }

    #[test]
    fn a_pcp_map_response_is_parsed() {
        let external = Ipv4Addr::new(203, 0, 113, 7);
        let PcpResponse::Map(map) = parse_pcp(&pcp_response(0, external, 0)).unwrap() else {
            panic!("expected a MAP response");
        };
        assert_eq!(
            map,
            PcpMapResponse {
                result: ResultCode::SUCCESS,
                lifetime: 7200,
                nonce: NONCE,
                protocol: 17,
                internal_port: 4773,
                external_port: 40000,
                external_ip: IpAddr::V4(external),
            }
        );
        // Options are skipped; a failure still names the request it answers.
        let with_options = parse_pcp(&pcp_response(8, external, 8)).unwrap();
        assert!(matches!(
            with_options,
            PcpResponse::Map(PcpMapResponse {
                result: ResultCode(8),
                nonce: NONCE,
                ..
            })
        ));
    }

    #[test]
    fn other_versions_are_recognised() {
        // A NAT-PMP router's Unsupported Version answer.
        assert_eq!(
            parse_pcp(&[0, 129, 0, 1, 0, 0, 0, 5]),
            Ok(PcpResponse::UnsupportedVersion)
        );
        let mut header = vec![0; 24];
        header[0] = 2;
        header[1] = 0x81;
        header[3] = 1;
        assert_eq!(parse_pcp(&header), Ok(PcpResponse::UnsupportedVersion));
        header[0] = 3;
        assert_eq!(parse_pcp(&header), Err(ParseError::Version(3)));
    }

    #[test]
    fn malformed_pcp_responses_are_rejected() {
        let good = pcp_response(0, Ipv4Addr::new(203, 0, 113, 7), 0);
        assert_eq!(parse_pcp(&good[..40]), Err(ParseError::Short));
        assert_eq!(parse_pcp(&good[..59]), Err(ParseError::Length(59)));
        assert_eq!(parse_pcp(&[2]), Err(ParseError::Short));
        let mut request = good.clone();
        request[1] = 0x01;
        assert_eq!(parse_pcp(&request), Err(ParseError::Opcode(1)));
        assert_eq!(parse_pcp(&vec![2; 1104]), Err(ParseError::Length(1104)));
    }

    #[test]
    fn natpmp_requests_follow_rfc_6886() {
        assert_eq!(NatPmpRequest::Address.encode(), [0, 0]);
        let map = NatPmpRequest::Map {
            internal_port: 4773,
            external_port: 0,
            lifetime: 7200,
        };
        assert_eq!(
            map.encode(),
            [0, 1, 0, 0, 0x12, 0xa5, 0, 0, 0, 0, 0x1c, 0x20]
        );
    }

    #[test]
    fn natpmp_responses_are_parsed() {
        assert_eq!(
            parse_natpmp(&[0, 128, 0, 0, 0, 0, 0, 9, 203, 0, 113, 7]),
            Ok(NatPmpResponse::Address {
                ip: Ipv4Addr::new(203, 0, 113, 7)
            })
        );
        assert_eq!(
            parse_natpmp(&[
                0, 129, 0, 0, 0, 0, 0, 9, 0x12, 0xa5, 0x9c, 0x40, 0, 0, 0x0e, 0x10
            ]),
            Ok(NatPmpResponse::Map {
                internal_port: 4773,
                external_port: 40000,
                lifetime: 3600,
            })
        );
        assert_eq!(
            parse_natpmp(&[0, 129, 0, 2, 0, 0, 0, 9]),
            Ok(NatPmpResponse::Failed {
                opcode: 1,
                result: ResultCode(2)
            })
        );
    }

    #[test]
    fn malformed_natpmp_responses_are_rejected() {
        assert_eq!(parse_natpmp(&[0, 128, 0, 0]), Err(ParseError::Short));
        assert_eq!(
            parse_natpmp(&[0, 129, 0, 0, 0, 0, 0, 9, 0, 0]),
            Err(ParseError::Short)
        );
        assert_eq!(
            parse_natpmp(&[2, 128, 0, 0, 0, 0, 0, 0]),
            Err(ParseError::Version(2))
        );
        assert_eq!(
            parse_natpmp(&[0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            Err(ParseError::Opcode(1))
        );
        assert_eq!(
            parse_natpmp(&[0, 130, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            Err(ParseError::Opcode(130))
        );
    }

    #[test]
    fn result_codes_have_names() {
        assert_eq!(ResultCode(2).to_string(), "not authorized");
        assert_eq!(ResultCode(8).to_string(), "no resources");
        assert_eq!(ResultCode(200).to_string(), "result code 200");
    }

    proptest! {
        #[test]
        fn parsers_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..1200)) {
            let _ = parse_pcp(&bytes);
            let _ = parse_natpmp(&bytes);
        }

        #[test]
        fn pcp_requests_and_responses_agree(
            client: [u8; 4],
            nonce: Nonce,
            internal_port: u16,
            external_port: u16,
            external_ip: [u8; 4],
            lifetime: u32,
        ) {
            let request = PcpMapRequest {
                client: client.into(),
                nonce,
                internal_port,
                external_port,
                external_ip: external_ip.into(),
                lifetime,
            };
            // A router echoes the request's MAP fields; marking the bytes a response makes one.
            let mut echoed = request.encode();
            echoed[1] |= 0x80;
            let PcpResponse::Map(map) = parse_pcp(&echoed).unwrap() else {
                panic!("expected a MAP response");
            };
            prop_assert_eq!(map.nonce, nonce);
            prop_assert_eq!(map.internal_port, internal_port);
            prop_assert_eq!(map.external_port, external_port);
            prop_assert_eq!(map.external_ip, IpAddr::V4(external_ip.into()));
            prop_assert_eq!(map.lifetime, lifetime);
        }
    }
}
