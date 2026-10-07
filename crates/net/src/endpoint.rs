//! QUIC endpoints and authenticated connections (`protocol/connections.md`).
//!
//! An [`Endpoint`] owns one UDP socket per address family and both dials and accepts on it, so
//! everything a peer sends from one family leaves from the same address, including its relay
//! connection (see [`home`](crate::home)). Every connection it yields has completed the TLS
//! handshake with the `tempest/1` ALPN identifier, and knows the other peer's ID and whether it
//! goes through a relay.

use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6, UdpSocket};
use std::sync::Arc;
use std::time::Duration;

use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::{ConnectionError, TransportConfig, VarInt};
use socket2::{Domain, Protocol, Type};
use tempest_core::io::{Closed, Direction, PathKind};
use tempest_protocol::message::{GoodbyeReason, MAX_REASON_MESSAGE_LEN};
use tempest_protocol::{Address, PeerId};

use crate::home::{HomeRelay, RelayChoice};
use crate::identity::Identity;
use crate::tls::{self, PEER_ALPN, SERVER_NAME, UnexpectedPeer};
use crate::wrap::{RelaySocket, is_virtual};

/// The default UDP port for peer connections (D-126).
pub const DEFAULT_PORT: u16 = 4773;

/// The largest UDP payload sent on a path before path MTU discovery shows it carries more.
pub const MIN_MTU: u16 = 1200;

/// The longest pause between keep-alives on a connection that may pass through a relay
/// (`protocol/relays.md`, Forwarding datagrams).
pub const MAX_KEEP_ALIVE: Duration = Duration::from_secs(60);

/// How an [`Endpoint`] binds its sockets and configures its connections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointConfig {
    /// The IPv4 address to bind, if any.
    pub ipv4: Option<SocketAddrV4>,
    /// The IPv6 address to bind, if any. The socket carries only IPv6.
    pub ipv6: Option<SocketAddrV6>,
    /// The most Requests the other peer may have open at once: this peer's Hello
    /// `max_open_requests`. The other peer may open one more stream, the control stream.
    pub max_open_requests: u32,
    /// How long a connection may go without hearing from the other peer before it is lost.
    pub idle_timeout: Duration,
    /// How often to send something on an otherwise quiet connection, to keep it and the
    /// routers' mappings for it alive. At most [`MAX_KEEP_ALIVE`] is used, so a relay's
    /// forwarding permission never lapses.
    pub keep_alive: Duration,
}

impl EndpointConfig {
    /// Binds every interface of both families on `port`.
    pub fn any(port: u16) -> Self {
        Self {
            ipv4: Some(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port)),
            ipv6: Some(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, port, 0, 0)),
            ..Self::default()
        }
    }

    /// Binds only IPv4 loopback on a port the system picks, for tests.
    pub fn localhost() -> Self {
        Self {
            ipv4: Some(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)),
            ipv6: None,
            ..Self::default()
        }
    }

    fn transport(&self) -> TransportConfig {
        let mut transport = TransportConfig::default();
        // QUIC pads a client's Initial packets to the initial MTU, so they are exactly 1,200
        // bytes, and nothing larger is sent until path MTU discovery confirms it.
        transport
            .initial_mtu(MIN_MTU)
            .min_mtu(MIN_MTU)
            .max_concurrent_bidi_streams(VarInt::from_u32(self.max_open_requests.saturating_add(1)))
            .max_concurrent_uni_streams(VarInt::from_u32(0))
            .keep_alive_interval(Some(self.keep_alive.min(MAX_KEEP_ALIVE)))
            .max_idle_timeout(Some(
                self.idle_timeout
                    .try_into()
                    .expect("idle timeout below 2^62 microseconds"),
            ));
        transport
    }
}

/// Binds both families on [`DEFAULT_PORT`].
impl Default for EndpointConfig {
    fn default() -> Self {
        Self {
            ipv4: Some(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DEFAULT_PORT)),
            ipv6: Some(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, DEFAULT_PORT, 0, 0)),
            max_open_requests: tempest_core::Config::default().upload.max_open_requests,
            idle_timeout: Duration::from_secs(30),
            keep_alive: Duration::from_secs(10),
        }
    }
}

/// Why a connection could not be established.
#[derive(Debug, thiserror::Error)]
pub enum HandshakeError {
    /// The endpoint has no socket for the address's family.
    #[error("no socket for {0}'s address family")]
    NoSocket(SocketAddr),
    /// The connection could not be started.
    #[error(transparent)]
    Connect(#[from] quinn::ConnectError),
    /// The other peer's key was not the one expected.
    #[error(transparent)]
    UnexpectedPeer(#[from] UnexpectedPeer),
    /// The handshake failed.
    #[error(transparent)]
    Connection(#[from] ConnectionError),
    /// The handshake settled on an ALPN identifier other than the one asked for.
    #[error("the handshake did not settle on the expected ALPN identifier")]
    Alpn,
    /// The other peer did not present an Ed25519 raw public key.
    #[error("the other peer presented no Ed25519 raw public key")]
    NoPeerId,
}

/// A QUIC endpoint with at most one UDP socket per address family.
#[derive(Debug, Clone)]
pub struct Endpoint {
    identity: Identity,
    ipv4: Option<Socket>,
    ipv6: Option<Socket>,
    transport: Arc<TransportConfig>,
    home: HomeRelay,
}

/// One address family's socket: quinn's endpoint over the wrapping layer.
#[derive(Debug, Clone)]
pub(crate) struct Socket {
    pub quic: quinn::Endpoint,
    pub wrap: Arc<RelaySocket>,
}

impl Endpoint {
    /// Binds the configured sockets. Must be called within a tokio runtime.
    ///
    /// If both families are configured and the IPv6 socket can't be bound for any reason but its
    /// port being in use, as on hosts without IPv6, the endpoint carries on with IPv4 only.
    pub fn bind(identity: Identity, config: &EndpointConfig) -> io::Result<Self> {
        let transport = Arc::new(config.transport());
        let mut server = quinn::ServerConfig::with_crypto(Arc::new(
            QuicServerConfig::try_from(tls::server_config(&identity, PEER_ALPN))
                .expect("the ring provider has QUIC's initial cipher suite"),
        ));
        server.transport_config(transport.clone());
        let server = Arc::new(server);

        let bind = |addr: SocketAddr| -> io::Result<Socket> {
            let runtime = quinn::TokioRuntime;
            let wrap = Arc::new(RelaySocket::new(quinn::Runtime::wrap_udp_socket(
                &runtime,
                udp_socket(addr)?,
            )?)?);
            let quic = quinn::Endpoint::new_with_abstract_socket(
                quinn::EndpointConfig::default(),
                Some(server.as_ref().clone()),
                wrap.clone(),
                Arc::new(runtime),
            )?;
            Ok(Socket { quic, wrap })
        };
        let ipv4 = config.ipv4.map(|addr| bind(addr.into())).transpose()?;
        let ipv6 = match config.ipv6.map(|addr| bind(addr.into())) {
            Some(Err(e)) if ipv4.is_some() && e.kind() != io::ErrorKind::AddrInUse => None,
            other => other.transpose()?,
        };
        if ipv4.is_none() && ipv6.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "no address to bind",
            ));
        }
        Ok(Self {
            identity,
            ipv4,
            ipv6,
            transport,
            home: HomeRelay::default(),
        })
    }

    /// This peer's ID.
    pub fn peer_id(&self) -> PeerId {
        self.identity.peer_id()
    }

    /// The addresses the sockets are bound to.
    pub fn local_addrs(&self) -> Vec<SocketAddr> {
        self.sockets()
            .filter_map(|socket| socket.local_addr().ok())
            .collect()
    }

    fn sockets(&self) -> impl Iterator<Item = &quinn::Endpoint> {
        self.ipv4
            .iter()
            .chain(&self.ipv6)
            .map(|socket| &socket.quic)
    }

    /// The socket for `addr`'s family.
    pub(crate) fn socket(&self, addr: SocketAddr) -> Option<&Socket> {
        match addr {
            SocketAddr::V4(_) => self.ipv4.as_ref(),
            SocketAddr::V6(_) => self.ipv6.as_ref(),
        }
    }

    /// The home relay: its status and this peer's relay address.
    pub fn home_relay(&self) -> &HomeRelay {
        &self.home
    }

    /// This peer's relay address, while registered with a home relay.
    pub fn relay_address(&self) -> Option<Address> {
        self.home.address()
    }

    /// Sets the relays to pick a home relay from, and registers with the one that answers
    /// fastest. With none, any home relay is left. Must be called within a tokio runtime.
    pub fn set_relays(&self, choice: RelayChoice) {
        self.home.choose(self, choice);
    }

    /// Dials a peer, accepting only `expected` if given.
    pub async fn dial(
        &self,
        addr: SocketAddr,
        expected: Option<PeerId>,
    ) -> Result<Connection, HandshakeError> {
        self.connect(addr, PEER_ALPN, expected, self.transport.clone())
            .await
    }

    /// Dials another peer's relay address, accepting only `expected` if given. The connection
    /// is marked relayed.
    pub async fn dial_relayed(
        &self,
        addr: SocketAddr,
        expected: Option<PeerId>,
    ) -> Result<Connection, HandshakeError> {
        let mut connection = self.dial(addr, expected).await?;
        connection.path = PathKind::Relayed;
        Ok(connection)
    }

    /// Dials with any ALPN identifier and transport settings, as for a relay, from the same
    /// socket peers are dialed from.
    pub(crate) async fn connect(
        &self,
        addr: SocketAddr,
        alpn: &[u8],
        expected: Option<PeerId>,
        transport: Arc<TransportConfig>,
    ) -> Result<Connection, HandshakeError> {
        // A dual-stack caller may hand over an IPv4 address in IPv6-mapped form.
        let addr = Address::from(addr).socket_addr();
        let socket = &self
            .socket(addr)
            .ok_or(HandshakeError::NoSocket(addr))?
            .quic;
        let verifier = Arc::new(tls::RawKeyVerifier::new(&tls::provider(), expected));
        let crypto = tls::client_config_with(&self.identity, alpn, verifier.clone());
        let mut client = quinn::ClientConfig::new(Arc::new(
            QuicClientConfig::try_from(crypto)
                .expect("the ring provider has QUIC's initial cipher suite"),
        ));
        client.transport_config(transport);
        let conn = socket
            .connect_with(client, addr, SERVER_NAME)?
            .await
            .map_err(|error| match verifier.unexpected() {
                Some(unexpected) => HandshakeError::UnexpectedPeer(unexpected),
                None => error.into(),
            })?;
        Connection::new(conn, Direction::Dialed, alpn)
    }

    /// Waits for the next incoming connection attempt. Returns `None` once the endpoint is
    /// closed.
    pub async fn accept(&self) -> Option<Incoming> {
        let incoming = match (&self.ipv4, &self.ipv6) {
            (Some(v4), Some(v6)) => tokio::select! {
                incoming = v4.quic.accept() => incoming,
                incoming = v6.quic.accept() => incoming,
            },
            (Some(socket), None) | (None, Some(socket)) => socket.quic.accept().await,
            (None, None) => None,
        };
        incoming.map(Incoming)
    }

    /// Closes every connection with a Goodbye, including the relay connection, and stops
    /// accepting.
    pub fn close(&self, reason: GoodbyeReason, message: &str) {
        self.home.stop();
        for socket in self.sockets() {
            socket.close(VarInt::from_u32(reason as u32), reason_phrase(message));
        }
    }

    /// Waits until every connection has finished closing, so the Goodbyes reach the other peers.
    pub async fn wait_idle(&self) {
        for socket in self.sockets() {
            socket.wait_idle().await;
        }
    }
}

/// Opens a UDP socket bound to `addr`. An IPv6 socket carries only IPv6.
fn udp_socket(addr: SocketAddr) -> io::Result<UdpSocket> {
    let domain = Domain::for_address(addr);
    let socket = socket2::Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))?;
    if addr.is_ipv6() {
        socket.set_only_v6(true)?;
    }
    socket.bind(&addr.into())?;
    Ok(socket.into())
}

/// A connection attempt from another peer, before the handshake.
#[derive(Debug)]
pub struct Incoming(quinn::Incoming);

impl Incoming {
    /// The address the attempt came from: a virtual address if it came through the home
    /// relay.
    pub fn remote_address(&self) -> SocketAddr {
        self.0.remote_address()
    }

    /// Completes the handshake.
    pub async fn accept(self) -> Result<Connection, HandshakeError> {
        let conn = self.0.await?;
        let mut connection = Connection::new(conn, Direction::Accepted, PEER_ALPN)?;
        if is_virtual(connection.remote_address()) {
            connection.path = PathKind::Relayed;
        }
        Ok(connection)
    }

    /// Turns the attempt away without a handshake, as when at the connection limit.
    pub fn refuse(self) {
        self.0.refuse();
    }
}

/// An established, authenticated connection with another peer.
#[derive(Debug, Clone)]
pub struct Connection {
    inner: quinn::Connection,
    peer: PeerId,
    direction: Direction,
    path: PathKind,
}

impl Connection {
    fn new(
        inner: quinn::Connection,
        direction: Direction,
        alpn: &[u8],
    ) -> Result<Self, HandshakeError> {
        let fail = |error| {
            inner.close(
                VarInt::from_u32(GoodbyeReason::ProtocolViolation as u32),
                &[],
            );
            Err(error)
        };
        if tls::alpn(&inner).as_deref() != Some(alpn) {
            return fail(HandshakeError::Alpn);
        }
        let Some(peer) = tls::peer_id(&inner) else {
            return fail(HandshakeError::NoPeerId);
        };
        Ok(Self {
            inner,
            peer,
            direction,
            path: PathKind::Direct,
        })
    }

    /// The other peer's ID.
    pub fn peer(&self) -> PeerId {
        self.peer
    }

    /// Which side dialed.
    pub fn direction(&self) -> Direction {
        self.direction
    }

    /// Whether the connection goes through a relay: this peer's home relay or the other
    /// peer's.
    pub fn path(&self) -> PathKind {
        self.path
    }

    /// The other peer's address, or a virtual address for a connection through this peer's
    /// home relay.
    pub fn remote_address(&self) -> SocketAddr {
        self.inner.remote_address()
    }

    /// QUIC's current estimate of the round-trip time.
    pub fn rtt(&self) -> Duration {
        self.inner.rtt()
    }

    /// The underlying QUIC connection, for opening and accepting streams.
    pub fn quic(&self) -> &quinn::Connection {
        &self.inner
    }

    /// Closes the connection with a Goodbye. The reason phrase is cut to 1 KiB.
    pub fn close(&self, reason: GoodbyeReason, message: &str) {
        self.inner
            .close(VarInt::from_u32(reason as u32), reason_phrase(message));
    }

    /// Waits for the connection to end, and says why.
    pub async fn closed(&self) -> ConnectionError {
        self.inner.closed().await
    }
}

/// How a connection that ended with `error` looks to a session: a Goodbye from the other peer,
/// or lost. Returns `None` if this side closed it.
pub fn closed_cause(error: &ConnectionError) -> Option<Closed> {
    match error {
        ConnectionError::ApplicationClosed(close) => Some(Closed::Goodbye {
            reason: GoodbyeReason::from_code(close.error_code.into_inner()),
            message: String::from_utf8_lossy(&close.reason).into_owned(),
        }),
        ConnectionError::LocallyClosed => None,
        _ => Some(Closed::Lost),
    }
}

/// `message` cut to at most 1 KiB, at a character boundary.
fn reason_phrase(message: &str) -> &[u8] {
    let mut end = message.len().min(MAX_REASON_MESSAGE_LEN as usize);
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    &message.as_bytes()[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_phrases_are_cut_at_a_character_boundary() {
        assert_eq!(reason_phrase("bye"), b"bye");
        let long = "é".repeat(600);
        let phrase = reason_phrase(&long);
        assert_eq!(phrase.len(), 1024);
        assert!(std::str::from_utf8(phrase).is_ok());
        let odd = format!("x{long}");
        assert_eq!(reason_phrase(&odd).len(), 1023);
    }

    #[test]
    fn the_transport_follows_the_spec() {
        let config = EndpointConfig::default();
        assert_eq!(config.ipv4.unwrap().port(), 4773);
        assert_eq!(config.ipv6.unwrap().port(), 4773);
        // Only the configuration's Debug form exposes its values.
        let transport = format!("{:?}", config.transport());
        assert!(transport.contains("initial_mtu: 1200"), "{transport}");
        assert!(transport.contains("min_mtu: 1200"), "{transport}");
        assert!(
            transport.contains("max_concurrent_uni_streams: 0"),
            "{transport}"
        );
    }

    #[tokio::test]
    async fn the_handshake_rejects_another_alpn() {
        let server =
            Endpoint::bind(Identity::generate().unwrap(), &EndpointConfig::localhost()).unwrap();
        let client =
            Endpoint::bind(Identity::generate().unwrap(), &EndpointConfig::localhost()).unwrap();
        let addr = server.local_addrs()[0];
        let accept = tokio::spawn(async move {
            let incoming = server.accept().await.unwrap();
            incoming.accept().await
        });
        let transport = client.transport.clone();
        let dial = client
            .connect(addr, b"tempest-relay/1", None, transport)
            .await;
        assert!(dial.is_err());
        assert!(accept.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn a_client_without_a_key_is_rejected() {
        let server =
            Endpoint::bind(Identity::generate().unwrap(), &EndpointConfig::localhost()).unwrap();
        let addr = server.local_addrs()[0];
        let accept = tokio::spawn(async move {
            let incoming = server.accept().await.unwrap();
            incoming.accept().await
        });

        // A client that verifies the server's raw key but presents none of its own.
        let provider = tls::provider();
        let verifier = Arc::new(tls::RawKeyVerifier::new(&provider, None));
        let mut crypto = rustls::ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_no_client_auth();
        crypto.alpn_protocols = vec![PEER_ALPN.to_vec()];
        let client_config =
            quinn::ClientConfig::new(Arc::new(QuicClientConfig::try_from(crypto).unwrap()));
        let socket = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        let conn = socket
            .connect_with(client_config, addr, SERVER_NAME)
            .unwrap()
            .await;
        // TLS 1.3 clients finish before the server checks their certificate, so the rejection
        // may only show once the server's alert arrives.
        if let Ok(conn) = conn {
            conn.closed().await;
        }
        assert!(accept.await.unwrap().is_err());
    }
}
