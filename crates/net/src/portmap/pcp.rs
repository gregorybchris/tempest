//! Asking the router for a mapping with PCP, or with NAT-PMP if the router only speaks that.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use ring::rand::{SecureRandom, SystemRandom};
use tokio::net::UdpSocket;
use tokio::time::timeout;

use super::wire::{
    self, NatPmpRequest, NatPmpResponse, Nonce, PcpMapRequest, PcpResponse, ResultCode,
};
use super::{Granted, MappingError};

/// How long to wait for an answer after each send: RFC 6886's schedule, cut short, since a
/// router that answers at all answers at once.
const WAITS: [Duration; 3] = [
    Duration::from_millis(250),
    Duration::from_millis(500),
    Duration::from_millis(1000),
];

/// Which protocol the router speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Version {
    /// PCP.
    Pcp,
    /// NAT-PMP, PCP's predecessor.
    NatPmp,
}

/// A client for one router's PCP or NAT-PMP server, and one mapping.
#[derive(Debug)]
pub struct Client {
    socket: UdpSocket,
    client_ip: Ipv4Addr,
    nonce: Nonce,
    version: Version,
}

impl Client {
    /// A client sending from `bind` to the server at `server`.
    pub async fn new(server: SocketAddr, bind: Ipv4Addr) -> io::Result<Self> {
        let socket = UdpSocket::bind((bind, 0)).await?;
        socket.connect(server).await?;
        let IpAddr::V4(client_ip) = socket.local_addr()?.ip() else {
            return Err(io::Error::other("an IPv4 socket has an IPv6 address"));
        };
        let mut nonce = Nonce::default();
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| io::Error::other("no randomness for a PCP nonce"))?;
        Ok(Self {
            socket,
            client_ip,
            nonce,
            version: Version::Pcp,
        })
    }

    /// The protocol the router speaks, as far as it is known.
    pub fn version(&self) -> Version {
        self.version
    }

    /// Asks for `internal_port` to be mapped for `lifetime`, or removes the mapping if
    /// `lifetime` is zero. `suggested` is the external address to ask for, such as the one
    /// already mapped.
    pub async fn map(
        &mut self,
        internal_port: u16,
        suggested: Option<SocketAddrV4>,
        lifetime: Duration,
    ) -> Result<Granted, MappingError> {
        let lifetime = u32::try_from(lifetime.as_secs()).unwrap_or(u32::MAX);
        if self.version == Version::Pcp {
            match self.map_pcp(internal_port, suggested, lifetime).await? {
                Some(granted) => return Ok(granted),
                None => self.version = Version::NatPmp,
            }
        }
        self.map_natpmp(internal_port, suggested, lifetime).await
    }

    /// Maps with PCP. Returns `None` if the router doesn't speak it.
    async fn map_pcp(
        &self,
        internal_port: u16,
        suggested: Option<SocketAddrV4>,
        lifetime: u32,
    ) -> Result<Option<Granted>, MappingError> {
        let request = PcpMapRequest {
            client: self.client_ip,
            nonce: self.nonce,
            internal_port,
            external_port: suggested.map_or(0, |addr| addr.port()),
            external_ip: suggested.map_or(Ipv4Addr::UNSPECIFIED, |addr| *addr.ip()),
            lifetime,
        };
        let response = self
            .exchange(&request.encode(), |bytes| match wire::parse_pcp(bytes) {
                Ok(PcpResponse::UnsupportedVersion) => Some(None),
                Ok(PcpResponse::Map(map))
                    if map.nonce == self.nonce
                        && map.internal_port == internal_port
                        && map.protocol == 17 =>
                {
                    Some(Some(map))
                }
                // Not an answer to this request.
                _ => None,
            })
            .await?;
        let Some(map) = response else {
            return Ok(None);
        };
        if map.result != ResultCode::SUCCESS {
            return Err(MappingError::Refused(map.result.to_string()));
        }
        let IpAddr::V4(ip) = map.external_ip else {
            return Err(MappingError::Invalid("an IPv6 external address"));
        };
        Ok(Some(Granted {
            external: SocketAddrV4::new(ip, map.external_port),
            lifetime: Some(Duration::from_secs(map.lifetime.into())),
        }))
    }

    /// Maps with NAT-PMP, which takes a second request for the external address.
    async fn map_natpmp(
        &self,
        internal_port: u16,
        suggested: Option<SocketAddrV4>,
        lifetime: u32,
    ) -> Result<Granted, MappingError> {
        // A removal asks for no external port (RFC 6886, section 3.4).
        let request = NatPmpRequest::Map {
            internal_port,
            external_port: suggested
                .filter(|_| lifetime > 0)
                .map_or(0, |addr| addr.port()),
            lifetime,
        };
        let mapped = self
            .exchange(&request.encode(), |bytes| match wire::parse_natpmp(bytes) {
                Ok(NatPmpResponse::Map {
                    internal_port: port,
                    external_port,
                    lifetime,
                }) if port == internal_port => Some(Ok((external_port, lifetime))),
                Ok(NatPmpResponse::Failed { opcode: 1, result }) => Some(Err(result)),
                _ => None,
            })
            .await?;
        let (external_port, lifetime) =
            mapped.map_err(|result| MappingError::Refused(result.to_string()))?;
        if lifetime == 0 {
            // A removal: there is no external address to look up.
            return Ok(Granted {
                external: SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, external_port),
                lifetime: Some(Duration::ZERO),
            });
        }
        let address = self
            .exchange(
                &NatPmpRequest::Address.encode(),
                |bytes| match wire::parse_natpmp(bytes) {
                    Ok(NatPmpResponse::Address { ip }) => Some(Ok(ip)),
                    Ok(NatPmpResponse::Failed { opcode: 0, result }) => Some(Err(result)),
                    _ => None,
                },
            )
            .await?;
        let ip = address.map_err(|result| MappingError::Refused(result.to_string()))?;
        Ok(Granted {
            external: SocketAddrV4::new(ip, external_port),
            lifetime: Some(Duration::from_secs(lifetime.into())),
        })
    }

    /// Sends `request` until `answer` accepts a response, resending after each wait.
    async fn exchange<T>(
        &self,
        request: &[u8],
        answer: impl Fn(&[u8]) -> Option<T>,
    ) -> Result<T, MappingError> {
        let mut buffer = [0; 1100];
        for wait in WAITS {
            self.socket.send(request).await.map_err(unreachable)?;
            let receive = async {
                loop {
                    let len = self.socket.recv(&mut buffer).await?;
                    if let Some(answer) = answer(&buffer[..len]) {
                        return Ok(answer);
                    }
                }
            };
            match timeout(wait, receive).await {
                Ok(Ok(answer)) => return Ok(answer),
                Ok(Err(error)) => return Err(unreachable(error)),
                Err(_) => {}
            }
        }
        Err(MappingError::NoAnswer)
    }
}

/// What a socket error means here: mostly that nothing listens on the router's port.
fn unreachable(error: io::Error) -> MappingError {
    match error.kind() {
        io::ErrorKind::ConnectionRefused => MappingError::NoAnswer,
        _ => MappingError::Io(error.to_string()),
    }
}
