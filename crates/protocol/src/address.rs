//! Network addresses (`protocol/messages.md`, Common structures).

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use crate::wire::{Decode, DecodeError, Encode, Reader, Writer};

const FAMILY_IPV4: u8 = 4;
const FAMILY_IPV6: u8 = 6;

/// An IP address and UDP port, in canonical form.
///
/// IPv4-mapped IPv6 addresses (`::ffff:a.b.c.d`) are stored as IPv4, and IPv6 flow and scope
/// information is dropped, so two equal `Address`es always have the same encoding.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Address(SocketAddr);

impl Address {
    /// Creates an address, canonicalizing IPv4-mapped IPv6 addresses to IPv4.
    pub fn new(ip: IpAddr, port: u16) -> Self {
        let ip = match ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
            IpAddr::V4(_) => ip,
        };
        Self(SocketAddr::new(ip, port))
    }

    /// The IP address.
    pub fn ip(&self) -> IpAddr {
        self.0.ip()
    }

    /// The port.
    pub fn port(&self) -> u16 {
        self.0.port()
    }

    /// The address as a standard socket address.
    pub fn socket_addr(&self) -> SocketAddr {
        self.0
    }
}

impl From<SocketAddr> for Address {
    fn from(addr: SocketAddr) -> Self {
        Self::new(addr.ip(), addr.port())
    }
}

impl From<Address> for SocketAddr {
    fn from(addr: Address) -> Self {
        addr.0
    }
}

/// Formats as `ip:port`, with IPv6 addresses in brackets.
impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl fmt::Debug for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Address({self})")
    }
}

impl Encode for Address {
    fn encode(&self, w: &mut Writer) {
        match self.0.ip() {
            IpAddr::V4(ip) => {
                w.u8(FAMILY_IPV4);
                w.raw(&ip.octets());
            }
            IpAddr::V6(ip) => {
                w.u8(FAMILY_IPV6);
                w.raw(&ip.octets());
            }
        }
        w.u16(self.0.port());
    }
}

impl Decode for Address {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let ip = match r.u8()? {
            FAMILY_IPV4 => {
                let octets: [u8; 4] = r.take(4)?.try_into().expect("took 4 bytes");
                IpAddr::V4(Ipv4Addr::from(octets))
            }
            FAMILY_IPV6 => {
                let octets: [u8; 16] = r.take(16)?.try_into().expect("took 16 bytes");
                let ip = Ipv6Addr::from(octets);
                if ip.to_ipv4_mapped().is_some() {
                    return Err(DecodeError::MappedIpv4Address);
                }
                IpAddr::V6(ip)
            }
            family => return Err(DecodeError::UnknownAddressFamily(family)),
        };
        let port = r.u16()?;
        Ok(Self(SocketAddr::new(ip, port)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(s: &str) -> Address {
        s.parse::<SocketAddr>().unwrap().into()
    }

    #[test]
    fn ipv4_encoding_matches_spec() {
        assert_eq!(
            addr("203.0.113.5:4433").to_bytes(),
            hex::decode("04cb0071051151").unwrap()
        );
    }

    #[test]
    fn ipv6_round_trips() {
        let a = addr("[2001:db8::1]:4433");
        let bytes = a.to_bytes();
        assert_eq!(bytes.len(), 1 + 16 + 2);
        assert_eq!(Address::from_bytes(&bytes), Ok(a));
        assert_eq!(a.to_string(), "[2001:db8::1]:4433");
    }

    #[test]
    fn mapped_ipv4_is_canonicalized_to_family_4() {
        let mapped = addr("[::ffff:203.0.113.5]:4433");
        assert_eq!(mapped, addr("203.0.113.5:4433"));
        assert_eq!(mapped.to_bytes()[0], FAMILY_IPV4);
    }

    #[test]
    fn mapped_ipv4_with_family_6_is_rejected() {
        let mut bytes = vec![FAMILY_IPV6];
        bytes.extend_from_slice(&"::ffff:203.0.113.5".parse::<Ipv6Addr>().unwrap().octets());
        bytes.extend_from_slice(&[0x11, 0x51]);
        assert_eq!(
            Address::from_bytes(&bytes),
            Err(DecodeError::MappedIpv4Address)
        );
    }

    #[test]
    fn unknown_family_is_rejected() {
        assert_eq!(
            Address::from_bytes(&[5, 1, 2, 3, 4, 0, 1]),
            Err(DecodeError::UnknownAddressFamily(5))
        );
    }
}
