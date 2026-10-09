//! The relay server: relay connections, registration and forwarding.
//!
//! Each registration runs as one task that owns its lent socket while it holds it, its
//! forwarding permissions and its bandwidth limits.

use std::collections::HashSet;
use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::ops::RangeInclusive;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use quinn::crypto::rustls::QuicServerConfig;
use quinn::{Connection, RecvStream, SendDatagramError, SendStream, VarInt};
use socket2::{Domain, Protocol, Socket, Type};
use tempest_net::limit::TokenBucket;
use tempest_net::{Identity, ReadMessageError, read_relay_message, tls, write_relay_message};
use tempest_protocol::message::GoodbyeReason;
use tempest_protocol::relay::{RELAY_ALPN, Registered, RelayReject, RelayRejectReason};
use tempest_protocol::{Address, PeerId, RelayDatagram, RelayMessage};
use tokio::net::UdpSocket;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::{Instant, timeout};

use crate::permissions::Permissions;
use crate::registry::{Limits, Registry};

/// How long a peer has to finish the handshake and send Register.
const REGISTER_TIMEOUT: Duration = Duration::from_secs(10);
/// The most connections that may be opening at once: handshaking, or yet to send Register.
/// Beyond it new connections are refused, and beyond half of it they must first prove their
/// address with a QUIC Retry, so forged sources can't hold handshakes open.
const MAX_OPENING: usize = 512;
/// How often kept sockets that have expired are given up.
const SWEEP_EVERY: Duration = Duration::from_secs(30);
/// How long a RelayReject, or the Goodbyes at shutdown, may take to arrive.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
/// The most addresses a registered peer may hold forwarding permissions for.
const MAX_PERMISSIONS: usize = 4096;
/// The most of them that may share one IP address or IPv6 /64.
const MAX_PERMISSIONS_PER_SOURCE: usize = 64;
/// Larger than any UDP payload, so no datagram is cut short.
const MAX_UDP_PAYLOAD: usize = 65_535;

/// The default limit on what a relay forwards for each registered peer in each direction, in
/// bytes per second: 1 MiB/s. Relayed connections carry traffic only until the peers connect
/// directly, and the limit keeps a few peers from taking a relay's whole bandwidth.
pub const DEFAULT_RATE_LIMIT: u64 = 1 << 20;

/// How a [`Relay`] binds and whom it serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayConfig {
    /// The UDP address relay connections arrive at. Lent sockets are bound to the same IP.
    pub bind: SocketAddr,
    /// The IP address in the relay addresses it lends. Defaults to the address each peer
    /// reached the relay at, which is wrong behind a NAT.
    pub public_ip: Option<IpAddr>,
    /// The ports to lend, or `None` for any the system picks.
    pub ports: Option<RangeInclusive<u16>>,
    /// The only peers that may register, or `None` for anyone.
    pub allowlist: Option<HashSet<PeerId>>,
    /// The most peers registered at once, counting the sockets kept for those that left.
    pub max_registrations: usize,
    /// The most peers registered at once from one IP address, or one IPv6 /64.
    pub max_per_ip: usize,
    /// The most bytes per second forwarded for each registered peer in each direction, or
    /// `None` for no limit. Datagrams over the limit are dropped. Defaults to
    /// [`DEFAULT_RATE_LIMIT`].
    pub rate_limit: Option<u64>,
    /// How long a relay connection may go without hearing from its peer before it is lost.
    pub idle_timeout: Duration,
}

impl RelayConfig {
    /// Listens on `bind`, with the default limits.
    pub fn new(bind: SocketAddr) -> Self {
        Self {
            bind,
            public_ip: None,
            ports: None,
            allowlist: None,
            max_registrations: 1000,
            max_per_ip: 8,
            rate_limit: Some(DEFAULT_RATE_LIMIT),
            idle_timeout: Duration::from_secs(60),
        }
    }

    /// Listens on IPv4 loopback on a port the system picks, for tests.
    pub fn localhost() -> Self {
        Self::new(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))
    }

    fn limits(&self) -> Limits {
        Limits {
            allowlist: self.allowlist.clone(),
            max_registrations: self.max_registrations,
            max_per_ip: self.max_per_ip,
        }
    }

    fn transport(&self) -> io::Result<quinn::TransportConfig> {
        let mut transport = quinn::TransportConfig::default();
        let idle_timeout = self
            .idle_timeout
            .try_into()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "idle timeout too long"))?;
        transport
            // The control stream is the only one.
            .max_concurrent_bidi_streams(VarInt::from_u32(1))
            .max_concurrent_uni_streams(VarInt::from_u32(0))
            .datagram_receive_buffer_size(Some(1 << 20))
            .max_idle_timeout(Some(idle_timeout));
        Ok(transport)
    }
}

/// A running relay (`protocol/relays.md`).
#[derive(Debug)]
pub struct Relay {
    endpoint: quinn::Endpoint,
    key: PeerId,
    shared: Arc<Shared>,
}

#[derive(Debug)]
struct Shared {
    config: RelayConfig,
    registry: Mutex<Registry<Arc<UdpSocket>, Connection>>,
}

impl Relay {
    /// Binds the relay's socket. Must be called within a tokio runtime.
    pub fn bind(identity: &Identity, config: RelayConfig) -> io::Result<Self> {
        let crypto = QuicServerConfig::try_from(tls::server_config(identity, RELAY_ALPN))
            .expect("the ring provider has QUIC's initial cipher suite");
        let mut server = quinn::ServerConfig::with_crypto(Arc::new(crypto));
        server.transport_config(Arc::new(config.transport()?));
        let endpoint = quinn::Endpoint::new(
            quinn::EndpointConfig::default(),
            Some(server),
            udp_socket(config.bind)?,
            Arc::new(quinn::TokioRuntime),
        )?;
        Ok(Self {
            endpoint,
            key: identity.peer_id(),
            shared: Arc::new(Shared {
                registry: Mutex::new(Registry::new(config.limits())),
                config,
            }),
        })
    }

    /// The relay's key, the first part of its identity.
    pub fn key(&self) -> PeerId {
        self.key
    }

    /// The address relay connections arrive at.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.endpoint.local_addr()
    }

    /// The number of peers registered now.
    pub fn registrations(&self) -> usize {
        self.shared.registry().registered()
    }

    /// Serves until `shutdown` completes, then closes every relay connection.
    pub async fn serve(&self, shutdown: impl Future<Output = ()>) {
        let mut shutdown = std::pin::pin!(shutdown);
        let mut sweep = tokio::time::interval(SWEEP_EVERY);
        let opening = Arc::new(Semaphore::new(MAX_OPENING));
        loop {
            tokio::select! {
                incoming = self.endpoint.accept() => {
                    let Some(incoming) = incoming else { break };
                    let Some(opening) = admit_opening(&opening, incoming) else {
                        continue;
                    };
                    let shared = self.shared.clone();
                    tokio::spawn(async move { shared.serve_peer(opening).await });
                }
                _ = sweep.tick() => self.shared.registry().expire(Instant::now()),
                () = &mut shutdown => break,
            }
        }
        self.endpoint
            .close(VarInt::from_u32(GoodbyeReason::ShuttingDown as u32), &[]);
        let _ = timeout(DRAIN_TIMEOUT, self.endpoint.wait_idle()).await;
    }
}

impl Shared {
    fn registry(&self) -> std::sync::MutexGuard<'_, Registry<Arc<UdpSocket>, Connection>> {
        self.registry
            .lock()
            .expect("no task panics holding the registry")
    }

    /// Registers one peer and forwards its traffic until its relay connection ends.
    async fn serve_peer(&self, (incoming, permit): (quinn::Incoming, OwnedSemaphorePermit)) {
        let opened = timeout(REGISTER_TIMEOUT, open(incoming)).await;
        drop(permit);
        let Ok(Some((conn, peer, send, recv))) = opened else {
            return;
        };
        let Some(relay_ip) = self.config.public_ip.or_else(|| conn.local_ip()) else {
            let reject = RelayReject {
                reason: RelayRejectReason::Full,
                message: "the relay doesn't know its public address".into(),
            };
            return refuse(&conn, send, reject).await;
        };

        let observed = conn.remote_address();
        let ip = Address::from(observed).ip();
        // A new socket is bound outside the registry's lock, since that may try many ports.
        let checked = self.registry().check(peer, ip, Instant::now());
        let fresh = match checked {
            Ok(false) => None,
            Ok(true) => match self.lend_socket().await {
                Ok(socket) => Some(socket),
                Err(_) => {
                    let reject = RelayReject {
                        reason: RelayRejectReason::Full,
                        message: "the relay has no port to lend".into(),
                    };
                    return refuse(&conn, send, reject).await;
                }
            },
            Err(reject) => return refuse(&conn, send, reject).await,
        };
        let admitted = (self.registry()).admit(peer, ip, conn.clone(), Instant::now(), fresh);
        let admitted = match admitted {
            Ok(admitted) => admitted,
            Err(reject) => return refuse(&conn, send, reject).await,
        };
        if let Some(old) = admitted.replaced {
            tracing::info!("{peer} registered again, from {observed}; closing its old connection");
            close(&old, GoodbyeReason::Duplicate, "registered again");
        }

        let port = admitted.socket.local_addr().map_or(0, |addr| addr.port());
        let registered = Registered {
            relay_address: Address::new(relay_ip, port),
            observed_address: observed.into(),
        };
        tracing::info!(
            "registered {peer} from {observed}, lent {}",
            registered.relay_address
        );
        let mut forwarder = Forwarder::new(&conn, &admitted.socket, self.config.rate_limit);
        forwarder.run(send, recv, registered).await;
        tracing::info!("{peer} left");
        self.registry()
            .release(peer, admitted.generation, Instant::now());
    }

    /// Binds a socket to lend, on the relay's IP, on a blocking thread: trying a range of ports
    /// may take many system calls.
    async fn lend_socket(&self) -> io::Result<Arc<UdpSocket>> {
        let ip = self.config.bind.ip();
        let ports = self.config.ports.clone();
        let bind = move || match ports {
            None => udp_socket(SocketAddr::new(ip, 0)),
            Some(ports) => ports
                .into_iter()
                .find_map(|port| udp_socket(SocketAddr::new(ip, port)).ok())
                .ok_or_else(|| io::Error::from(io::ErrorKind::AddrInUse)),
        };
        let socket = tokio::task::spawn_blocking(bind)
            .await
            .map_err(io::Error::other)??;
        socket.set_nonblocking(true)?;
        Ok(Arc::new(UdpSocket::from_std(socket)?))
    }
}

/// Takes a slot for a new connection to open in, or refuses it: outright when every slot is
/// taken, and with a Retry, which costs the relay no state, when over half are and the
/// client's address is unproven.
fn admit_opening(
    slots: &Arc<Semaphore>,
    incoming: quinn::Incoming,
) -> Option<(quinn::Incoming, OwnedSemaphorePermit)> {
    let Ok(permit) = slots.clone().try_acquire_owned() else {
        tracing::debug!(
            "refused {}: every opening slot is taken",
            incoming.remote_address()
        );
        incoming.refuse();
        return None;
    };
    if slots.available_permits() < MAX_OPENING / 2 && !incoming.remote_address_validated() {
        // Fails only if the address is validated, which was just checked.
        let _ = incoming.retry();
        return None;
    }
    Some((incoming, permit))
}

/// Completes the handshake and reads Register from the control stream. Closes the connection
/// if the peer breaks a rule; returns `None` if it failed.
async fn open(incoming: quinn::Incoming) -> Option<(Connection, PeerId, SendStream, RecvStream)> {
    let conn = incoming.await.ok()?;
    let peer = tls::peer_id(&conn).filter(|_| tls::alpn(&conn).as_deref() == Some(RELAY_ALPN));
    let opened = match peer {
        Some(peer) => {
            let (send, mut recv) = conn.accept_bi().await.ok()?;
            match read_relay_message(&mut recv).await {
                Ok(Some(RelayMessage::Register)) => Some((peer, send, recv)),
                _ => None,
            }
        }
        None => None,
    };
    let Some((peer, send, recv)) = opened else {
        tracing::info!("closed {}: it didn't register", conn.remote_address());
        close(&conn, GoodbyeReason::ProtocolViolation, "");
        return None;
    };
    Some((conn, peer, send, recv))
}

/// Sends RelayReject, waits a moment for it to arrive, and closes the connection.
async fn refuse(conn: &Connection, mut send: SendStream, reject: RelayReject) {
    tracing::info!(
        "refused {}: {:?} {}",
        conn.remote_address(),
        reject.reason,
        reject.message
    );
    let sent = async {
        write_relay_message(&mut send, &reject.into()).await.ok()?;
        send.finish().ok()?;
        send.stopped().await.ok()
    };
    let _ = timeout(DRAIN_TIMEOUT, sent).await;
    close(conn, GoodbyeReason::Done, "");
}

/// Closes a relay connection with a Goodbye code, as peer connections close.
fn close(conn: &Connection, reason: GoodbyeReason, message: &str) {
    conn.close(VarInt::from_u32(reason as u32), message.as_bytes());
}

/// Moves one registered peer's traffic between its relay connection and its lent socket.
struct Forwarder<'a> {
    conn: &'a Connection,
    socket: &'a UdpSocket,
    permissions: Permissions,
    /// Limits what is forwarded to the peer.
    inbound: TokenBucket,
    /// Limits what the peer sends out.
    outbound: TokenBucket,
}

impl<'a> Forwarder<'a> {
    fn new(conn: &'a Connection, socket: &'a UdpSocket, rate_limit: Option<u64>) -> Self {
        let now = Instant::now();
        Self {
            conn,
            socket,
            permissions: Permissions::new(MAX_PERMISSIONS, MAX_PERMISSIONS_PER_SOURCE),
            inbound: TokenBucket::new(rate_limit, now),
            outbound: TokenBucket::new(rate_limit, now),
        }
    }

    /// Sends Registered, then forwards until the connection ends.
    async fn run(&mut self, mut send: SendStream, mut recv: RecvStream, registered: Registered) {
        if write_relay_message(&mut send, &registered.into())
            .await
            .is_err()
        {
            return;
        }
        // Nothing more is defined for the peer to send; a frame is a violation, and the end of
        // the stream is fine.
        let control = read_relay_message(&mut recv);
        let mut control = std::pin::pin!(control);
        let mut control_open = true;
        let mut buffer = vec![0; MAX_UDP_PAYLOAD];
        loop {
            tokio::select! {
                received = self.socket.recv_from(&mut buffer) => {
                    // Errors here are ICMP reports about earlier sends; carry on.
                    if let Ok((len, from)) = received
                        && !self.forward_to_peer(from.into(), &buffer[..len])
                    {
                        break;
                    }
                }
                datagram = self.conn.read_datagram() => {
                    let Ok(datagram) = datagram else { break };
                    self.forward_from_peer(&datagram).await;
                }
                message = &mut control, if control_open => {
                    control_open = false;
                    if !matches!(message, Ok(None)) {
                        if !matches!(message, Err(ReadMessageError::Read(_))) {
                            tracing::info!("closed {}: it broke the protocol", self.conn.remote_address());
                            close(self.conn, GoodbyeReason::ProtocolViolation, "");
                        }
                        break;
                    }
                }
            }
        }
    }

    /// Passes a packet that arrived at the relay address to the peer as Forwarded. Returns
    /// whether the connection is still open.
    fn forward_to_peer(&mut self, from: Address, packet: &[u8]) -> bool {
        let now = Instant::now();
        self.permissions.grant(from, now);
        if self.inbound.take(packet.len() as u64, now).is_some() {
            return true;
        }
        let datagram = RelayDatagram::Forwarded { from, packet }.encode();
        // A datagram too large for the path is dropped, like any lost packet.
        !matches!(
            self.conn.send_datagram(datagram.into()),
            Err(SendDatagramError::ConnectionLost(_))
        )
    }

    /// Sends a Forward's packet from the relay address, if the forwarding rule allows it.
    /// Anything else the peer sends as a datagram is dropped.
    async fn forward_from_peer(&mut self, datagram: &[u8]) {
        let Ok(RelayDatagram::Forward { to, packet }) = RelayDatagram::decode(datagram) else {
            return;
        };
        let now = Instant::now();
        if !self.permissions.send(to, now) || self.outbound.take(packet.len() as u64, now).is_some()
        {
            return;
        }
        if let Some(to) = destination(to, self.socket) {
            // Failed sends are lost packets, which QUIC between the peers recovers from.
            let _ = self.socket.send_to(packet, to).await;
        }
    }
}

/// `to` in the form `socket` sends to: IPv4 addresses as IPv4-mapped on an IPv6 socket, which
/// carries both families. `None` if the socket can't reach it.
fn destination(to: Address, socket: &UdpSocket) -> Option<SocketAddr> {
    let local = socket.local_addr().ok()?;
    match (to.ip(), local.ip()) {
        (IpAddr::V4(v4), IpAddr::V6(_)) => {
            Some(SocketAddr::new(v4.to_ipv6_mapped().into(), to.port()))
        }
        (IpAddr::V6(_), IpAddr::V4(_)) => None,
        _ => Some(to.socket_addr()),
    }
}

/// Opens a UDP socket bound to `addr`. On IPv6's wildcard address it carries both families,
/// so peers of either can reach it.
fn udp_socket(addr: SocketAddr) -> io::Result<std::net::UdpSocket> {
    let socket = Socket::new(Domain::for_address(addr), Type::DGRAM, Some(Protocol::UDP))?;
    if addr.ip() == IpAddr::V6(Ipv6Addr::UNSPECIFIED) {
        socket.set_only_v6(false)?;
    }
    socket.bind(&addr.into())?;
    Ok(socket.into())
}
