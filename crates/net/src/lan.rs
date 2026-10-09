//! LAN discovery (`protocol/discovery.md`, LAN discovery).
//!
//! A [`Lan`] announces this peer to the multicast groups on each local network when it starts
//! and every minute after, queries them when it starts and when a download begins, and answers
//! other peers' queries. Every peer it hears of is handed to the downloads it watches. They dial
//! it and name their bundle in Hello, and a peer without the bundle closes with `unknown_bundle`.
//!
//! Each address the endpoint advertises gets a sending socket bound to it, so a datagram's
//! source address is one the endpoint is reachable at, and replies to queries come back to that
//! socket's own port. A listening socket per family, shared with other clients on the machine,
//! receives what is sent to the group.
//!
//! What to ignore, when to reply and which peers are known is decided by [`LanState`], which
//! touches no sockets.

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use socket2::{Domain, Protocol, Socket, Type};
use tempest_protocol::lan::{LAN_GROUP_V4, LAN_GROUP_V6, LAN_PORT, LanKind, LanMessage};
use tempest_protocol::message::PeerRecord;
use tempest_protocol::{Address, PeerId};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tokio::time::Instant;

use crate::driver::{BundleHandle, PeerSink, Status};
use crate::endpoint::Endpoint;

/// How LAN discovery behaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanConfig {
    /// The groups' UDP port.
    pub port: u16,
    /// How often to announce.
    pub announce_interval: Duration,
    /// The least time between replies to one source's queries.
    pub reply_interval: Duration,
    /// The least time between queries.
    pub query_interval: Duration,
    /// How long a peer that has gone quiet is still handed to new downloads.
    pub forget_after: Duration,
    /// The most peers remembered. Others are ignored until some are forgotten.
    pub max_peers: usize,
    /// The most addresses remembered per peer. A new one replaces the oldest.
    pub max_addresses: usize,
}

impl Default for LanConfig {
    fn default() -> Self {
        Self {
            port: LAN_PORT,
            announce_interval: Duration::from_secs(60),
            reply_interval: Duration::from_secs(1),
            query_interval: Duration::from_secs(1),
            forget_after: Duration::from_secs(3 * 60),
            max_peers: 1024,
            max_addresses: 8,
        }
    }
}

/// LAN discovery for one endpoint. Clones share it; it stops once every clone is dropped.
#[derive(Debug, Clone)]
pub struct Lan {
    commands: mpsc::Sender<Command>,
}

impl Lan {
    /// Starts announcing `endpoint` on the networks its advertised addresses are on. Must be
    /// called within a tokio runtime.
    ///
    /// Fails if no group could be joined, as on a machine with no network but loopback when the
    /// endpoint binds every interface.
    pub fn start(endpoint: &Endpoint, config: LanConfig) -> io::Result<Self> {
        let interfaces = local_interfaces(&endpoint.local_addrs())?;
        let sockets = Sockets::open(&interfaces, config.port)?;
        tracing::info!(
            "LAN discovery on {} interfaces, port {}",
            interfaces.len(),
            config.port
        );
        let (commands, receiver) = mpsc::channel(64);
        let task = LanTask {
            state: LanState::new(endpoint.peer_id(), &config),
            me: endpoint.peer_id(),
            config,
            sockets,
            watched: Vec::new(),
            last_query: None,
            requery: None,
        };
        tokio::spawn(task.run(receiver));
        Ok(Self { commands })
    }

    /// Hands the peers heard of, now and later, to `bundle` until it completes or stops, and
    /// queries the networks for more.
    pub async fn watch(&self, bundle: &BundleHandle) {
        let watched = Watched {
            sink: bundle.peer_sink(),
            status: bundle.status.clone(),
        };
        // The task only stops once every handle is gone.
        let _ = self.commands.send(Command::Watch(watched)).await;
    }
}

enum Command {
    Watch(Watched),
}

/// A bundle that gets the peers heard of.
struct Watched {
    sink: PeerSink,
    status: watch::Receiver<Status>,
}

impl Watched {
    /// Whether the bundle stopped.
    fn stopped(&self) -> bool {
        self.status.has_changed().is_err()
    }

    /// Whether the bundle still wants peers.
    fn wants_peers(&self) -> bool {
        !self.status.borrow().complete
    }
}

/// The local network an advertised address is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Interface {
    /// The advertised address: an IP of this interface and the endpoint's port in its family.
    address: Address,
    /// The interface's netmask, to tell which sources are on its network.
    netmask: IpAddr,
    /// The interface's index, which IPv6 multicast needs.
    index: u32,
}

impl Interface {
    /// Whether `ip` is on this interface's network.
    fn contains(&self, ip: IpAddr) -> bool {
        match (self.address.ip(), self.netmask, ip) {
            (IpAddr::V4(own), IpAddr::V4(mask), IpAddr::V4(ip)) => {
                own.to_bits() & mask.to_bits() == ip.to_bits() & mask.to_bits()
            }
            (IpAddr::V6(own), IpAddr::V6(mask), IpAddr::V6(ip)) => {
                own.to_bits() & mask.to_bits() == ip.to_bits() & mask.to_bits()
            }
            _ => false,
        }
    }
}

/// An interface address as the system lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SystemAddress {
    ip: IpAddr,
    netmask: IpAddr,
    index: u32,
}

/// The interfaces to announce on for sockets bound to `bound`.
fn local_interfaces(bound: &[SocketAddr]) -> io::Result<Vec<Interface>> {
    let system: Vec<SystemAddress> = if_addrs::get_if_addrs()?
        .into_iter()
        .map(|interface| {
            let (ip, netmask) = match &interface.addr {
                if_addrs::IfAddr::V4(v4) => (IpAddr::V4(v4.ip), IpAddr::V4(v4.netmask)),
                if_addrs::IfAddr::V6(v6) => (IpAddr::V6(v6.ip), IpAddr::V6(v6.netmask)),
            };
            SystemAddress {
                ip,
                netmask,
                index: interface.index.unwrap_or(0),
            }
        })
        .collect();
    Ok(select_interfaces(bound, &system))
}

/// The interfaces for sockets bound to `bound`, on a machine with the interface addresses
/// `system`: one per address the endpoint advertises (see [`crate::direct_addresses`]).
fn select_interfaces(bound: &[SocketAddr], system: &[SystemAddress]) -> Vec<Interface> {
    let ips: Vec<IpAddr> = system.iter().map(|a| a.ip).collect();
    crate::addresses::advertise(bound, &ips)
        .into_iter()
        .map(|address| {
            let listed = system.iter().find(|a| a.ip == address.ip());
            let netmask = match address.ip() {
                IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::BROADCAST),
                IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::from_bits(u128::MAX)),
            };
            Interface {
                address,
                netmask: listed.map_or(netmask, |a| a.netmask),
                index: listed.map_or(0, |a| a.index),
            }
        })
        .collect()
}

/// A socket sending from one interface.
struct Sender {
    interface: Interface,
    socket: Arc<UdpSocket>,
}

/// Every socket LAN discovery uses.
struct Sockets {
    senders: Vec<Sender>,
    listeners: Vec<Arc<UdpSocket>>,
}

impl Sockets {
    /// Opens a sending socket per interface and a listening socket per family on `port`, joined
    /// to the group on each interface. Interfaces whose group can't be joined are skipped.
    fn open(interfaces: &[Interface], port: u16) -> io::Result<Self> {
        let mut senders = Vec::new();
        let mut listeners = Vec::new();
        let mut last_error = None;
        for ipv4 in [true, false] {
            let family: Vec<&Interface> = interfaces
                .iter()
                .filter(|i| i.address.ip().is_ipv4() == ipv4)
                .collect();
            if family.is_empty() {
                continue;
            }
            let listener = match listening_socket(ipv4, port) {
                Ok(listener) => listener,
                Err(error) => {
                    last_error = Some(error);
                    continue;
                }
            };
            let mut joined = false;
            for interface in family {
                let sender = join(&listener, interface).and_then(|()| sending_socket(interface));
                match sender {
                    Ok(socket) => {
                        joined = true;
                        senders.push(Sender {
                            interface: *interface,
                            socket: Arc::new(UdpSocket::from_std(socket.into())?),
                        });
                    }
                    Err(error) => last_error = Some(error),
                }
            }
            if joined {
                listeners.push(Arc::new(UdpSocket::from_std(listener.into())?));
            }
        }
        if senders.is_empty() {
            return Err(last_error.unwrap_or_else(|| {
                io::Error::new(
                    io::ErrorKind::AddrNotAvailable,
                    "no local network to discover peers on",
                )
            }));
        }
        Ok(Self { senders, listeners })
    }

    /// The sender to answer `source` from: one on its network, or else any in its family.
    fn sender_for(&self, source: SocketAddr) -> Option<&Sender> {
        let family = |s: &&Sender| s.interface.address.ip().is_ipv4() == source.is_ipv4();
        self.senders
            .iter()
            .filter(family)
            .find(|s| s.interface.contains(source.ip()))
            .or_else(|| self.senders.iter().find(family))
    }
}

/// Whether `ip` is on the local network: on one of `interfaces`' networks, this machine, or a
/// link-local address, which no router forwards.
fn on_lan<'a>(mut interfaces: impl Iterator<Item = &'a Interface>, ip: IpAddr) -> bool {
    let ip = ip.to_canonical();
    let link_local = match ip {
        IpAddr::V4(v4) => v4.is_link_local(),
        IpAddr::V6(v6) => v6.is_unicast_link_local(),
    };
    link_local || ip.is_loopback() || interfaces.any(|i| i.contains(ip))
}

/// A socket on the group port of one family, shared with other clients on the machine.
fn listening_socket(ipv4: bool, port: u16) -> io::Result<Socket> {
    let (domain, wildcard) = if ipv4 {
        (
            Domain::IPV4,
            SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)),
        )
    } else {
        (
            Domain::IPV6,
            SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)),
        )
    };
    let socket = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))?;
    if !ipv4 {
        socket.set_only_v6(true)?;
    }
    socket.set_reuse_address(true)?;
    #[cfg(all(unix, not(any(target_os = "solaris", target_os = "illumos"))))]
    socket.set_reuse_port(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&wildcard.into())?;
    Ok(socket)
}

/// Joins `listener` to its family's group on `interface`. Joining an interface twice, as for
/// a second address on it, is not an error.
fn join(listener: &Socket, interface: &Interface) -> io::Result<()> {
    let result = match interface.address.ip() {
        IpAddr::V4(ip) => listener.join_multicast_v4(&LAN_GROUP_V4, &ip),
        IpAddr::V6(_) => listener.join_multicast_v6(&LAN_GROUP_V6, interface.index),
    };
    match result {
        Err(error) if error.kind() == io::ErrorKind::AddrInUse => Ok(()),
        other => other,
    }
}

/// A socket bound to `interface`'s address on a port of its own, sending multicast out of
/// that interface and to this machine too.
fn sending_socket(interface: &Interface) -> io::Result<Socket> {
    let ip = interface.address.ip();
    let socket = Socket::new(
        Domain::for_address(SocketAddr::new(ip, 0)),
        Type::DGRAM,
        Some(Protocol::UDP),
    )?;
    match ip {
        IpAddr::V4(v4) => {
            socket.set_multicast_if_v4(&v4)?;
            socket.set_multicast_loop_v4(true)?;
        }
        IpAddr::V6(_) => {
            socket.set_only_v6(true)?;
            socket.set_multicast_if_v6(interface.index)?;
            socket.set_multicast_loop_v6(true)?;
        }
    }
    socket.set_nonblocking(true)?;
    socket.bind(&SocketAddr::new(ip, 0).into())?;
    Ok(socket)
}

/// The group to send to from `ip`'s family.
fn group(ip: IpAddr, port: u16) -> SocketAddr {
    match ip {
        IpAddr::V4(_) => SocketAddr::from((LAN_GROUP_V4, port)),
        IpAddr::V6(_) => SocketAddr::from((LAN_GROUP_V6, port)),
    }
}

/// A datagram one of the sockets received.
struct Datagram {
    source: SocketAddr,
    bytes: Vec<u8>,
}

/// Larger than any LAN message, so a longer datagram is never cut down to a valid length.
const RECEIVE_BUFFER: usize = 64;

/// Reads datagrams from `socket` until the task that collects them stops.
async fn read(socket: Arc<UdpSocket>, datagrams: mpsc::Sender<Datagram>) {
    let mut buffer = [0; RECEIVE_BUFFER];
    loop {
        let datagram = match socket.recv_from(&mut buffer).await {
            Ok((len, source)) => Datagram {
                source,
                bytes: buffer[..len].to_vec(),
            },
            // Such as an ICMP error from an earlier send; the socket carries on.
            Err(_) => continue,
        };
        if datagrams.send(datagram).await.is_err() {
            return;
        }
    }
}

/// Waits until `requery`'s time, or forever when there is none.
async fn sleep_until_requery(requery: Option<(Instant, Duration)>) {
    match requery {
        Some((at, _)) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// The LAN discovery task's state.
struct LanTask {
    state: LanState,
    me: PeerId,
    config: LanConfig,
    sockets: Sockets,
    watched: Vec<Watched>,
    last_query: Option<Instant>,
    /// When to query again for downloads that still want peers, and the wait after that. A
    /// single query or its reply can be lost, and the next announce may be a minute away.
    requery: Option<(Instant, Duration)>,
}

impl LanTask {
    async fn run(mut self, mut commands: mpsc::Receiver<Command>) {
        let (datagrams, mut received) = mpsc::channel(256);
        let mut readers = JoinSet::new();
        let sockets = self.sockets.senders.iter().map(|s| &s.socket);
        for socket in sockets.chain(&self.sockets.listeners) {
            readers.spawn(read(socket.clone(), datagrams.clone()));
        }
        drop(datagrams);

        let mut announce = tokio::time::interval(self.config.announce_interval);
        announce.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        announce.tick().await;
        self.announce().await;
        self.query().await;
        loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(Command::Watch(watched)) => self.watch(watched).await,
                    None => break,
                },
                Some(datagram) = received.recv() => self.receive(datagram).await,
                () = sleep_until_requery(self.requery) => self.requery().await,
                _ = announce.tick() => {
                    self.announce().await;
                    self.state.expire(Instant::now());
                    self.watched.retain(|w| !w.stopped());
                }
            }
        }
        readers.abort_all();
    }

    async fn watch(&mut self, watched: Watched) {
        if watched.stopped() || !watched.wants_peers() {
            return;
        }
        let known = self.state.known();
        if !known.is_empty() {
            deliver(vec![watched.sink.clone()], known);
        }
        tracing::debug!(
            "LAN discovery watching a bundle; {} watched",
            self.watched.len() + 1
        );
        self.watched.push(watched);
        self.query().await;
        let wait = self.config.query_interval * 2;
        self.requery = Some((Instant::now() + wait, wait));
    }

    /// Queries again while a download still wants peers, backing off up to the announce
    /// interval.
    async fn requery(&mut self) {
        self.watched.retain(|w| !w.stopped());
        let Some((_, wait)) = self.requery else {
            return;
        };
        if !self.watched.iter().any(Watched::wants_peers) {
            self.requery = None;
            return;
        }
        self.query().await;
        let wait = (wait * 2).min(self.config.announce_interval);
        self.requery = Some((Instant::now() + wait, wait));
    }

    async fn receive(&mut self, datagram: Datagram) {
        // Only sources on one of this host's networks are on the LAN. Others reached the
        // listener from beyond it, and could steer this peer's dials anywhere.
        let interfaces = self.sockets.senders.iter().map(|s| &s.interface);
        if !on_lan(interfaces, datagram.source.ip()) {
            tracing::debug!(
                "ignored a LAN message from {}: not on a local network",
                datagram.source
            );
            return;
        }
        let now = Instant::now();
        let received = self.state.receive(now, datagram.source, &datagram.bytes);
        if let Some(source) = received.reply_to
            && let Some(sender) = self.sockets.sender_for(source)
        {
            let announce = self.message(LanKind::Announce, sender);
            if let Err(error) = sender.socket.send_to(&announce, source).await {
                tracing::debug!("LAN reply to {source} failed: {error}");
            }
        }
        if let Some(record) = received.discovered {
            tracing::debug!(
                "LAN peer {} found at {:?}",
                record.peer_id,
                record.addresses
            );
            self.watched.retain(|w| !w.stopped());
            let sinks = self
                .watched
                .iter()
                .filter(|w| w.wants_peers())
                .map(|w| w.sink.clone())
                .collect();
            deliver(sinks, vec![record]);
        }
    }

    async fn announce(&self) {
        self.send_to_groups(LanKind::Announce).await;
    }

    /// Queries the groups, unless a query went out moments ago.
    async fn query(&mut self) {
        let now = Instant::now();
        if self
            .last_query
            .is_some_and(|last| now < last + self.config.query_interval)
        {
            return;
        }
        self.last_query = Some(now);
        self.send_to_groups(LanKind::Query).await;
    }

    async fn send_to_groups(&self, kind: LanKind) {
        for sender in &self.sockets.senders {
            let message = self.message(kind, sender);
            let group = group(sender.interface.address.ip(), self.config.port);
            // A network that goes away fails its sends until it comes back.
            if let Err(error) = sender.socket.send_to(&message, group).await {
                tracing::debug!("LAN {kind:?} to {group} failed: {error}");
            }
        }
    }

    fn message(
        &self,
        kind: LanKind,
        sender: &Sender,
    ) -> [u8; tempest_protocol::lan::LAN_MESSAGE_LEN] {
        LanMessage {
            kind,
            peer_id: self.me,
            port: sender.interface.address.port(),
        }
        .encode()
    }
}

/// Hands `records` to `sinks` without holding up the LAN task.
fn deliver(sinks: Vec<PeerSink>, records: Vec<PeerRecord>) {
    if sinks.is_empty() {
        return;
    }
    tokio::spawn(async move {
        for sink in sinks {
            sink.add_peers(records.clone()).await;
        }
    });
}

/// What to do about a received datagram.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Received {
    /// Where to send an announce, in reply to a query.
    pub reply_to: Option<SocketAddr>,
    /// The sender, if it is new or has a new address.
    pub discovered: Option<PeerRecord>,
}

/// LAN discovery's rules, apart from any socket: which datagrams to ignore, when to reply to a
/// query, and which peers have been heard of.
#[derive(Debug)]
pub struct LanState {
    me: PeerId,
    reply_interval: Duration,
    forget_after: Duration,
    max_peers: usize,
    max_addresses: usize,
    peers: HashMap<PeerId, Heard>,
    /// When each source was last replied to.
    replies: HashMap<SocketAddr, Instant>,
}

#[derive(Debug)]
struct Heard {
    /// Oldest first.
    addresses: Vec<Address>,
    last: Instant,
}

impl LanState {
    /// The state for the peer `me`.
    pub fn new(me: PeerId, config: &LanConfig) -> Self {
        Self {
            me,
            reply_interval: config.reply_interval,
            forget_after: config.forget_after,
            max_peers: config.max_peers,
            max_addresses: config.max_addresses,
            peers: HashMap::new(),
            replies: HashMap::new(),
        }
    }

    /// Handles a datagram from `source`. Datagrams that are not LAN messages, and this peer's
    /// own, are ignored. A query or an announce both make the sender known.
    pub fn receive(&mut self, now: Instant, source: SocketAddr, datagram: &[u8]) -> Received {
        let Some(message) = LanMessage::decode(datagram) else {
            return Received::default();
        };
        if message.peer_id == self.me {
            return Received::default();
        }
        let reply_to =
            (message.kind == LanKind::Query && self.may_reply(now, source)).then_some(source);
        let discovered = self.hear(now, message.peer_id, source.ip(), message.port);
        Received {
            reply_to,
            discovered,
        }
    }

    fn may_reply(&mut self, now: Instant, source: SocketAddr) -> bool {
        if let Some(last) = self.replies.get(&source)
            && now < *last + self.reply_interval
        {
            return false;
        }
        if !self.replies.contains_key(&source) && self.replies.len() >= self.max_peers {
            let interval = self.reply_interval;
            self.replies.retain(|_, last| now < *last + interval);
            if self.replies.len() >= self.max_peers {
                return false;
            }
        }
        self.replies.insert(source, now);
        true
    }

    /// Notes that `peer` is at `ip` and `port`. Returns its record if that is news.
    fn hear(&mut self, now: Instant, peer: PeerId, ip: IpAddr, port: u16) -> Option<PeerRecord> {
        let address = Address::new(ip, port);
        // A link-local IPv6 source needs a scope ID, which an address can't carry.
        let unreachable = port == 0
            || address.ip().is_unspecified()
            || address.ip().is_multicast()
            || matches!(address.ip(), IpAddr::V6(v6) if v6.is_unicast_link_local());
        if unreachable {
            return None;
        }
        if !self.peers.contains_key(&peer) && self.peers.len() >= self.max_peers {
            self.expire(now);
            if self.peers.len() >= self.max_peers {
                return None;
            }
        }
        let heard = self.peers.entry(peer).or_insert(Heard {
            addresses: Vec::new(),
            last: now,
        });
        heard.last = now;
        if heard.addresses.contains(&address) {
            return None;
        }
        if heard.addresses.len() >= self.max_addresses {
            heard.addresses.remove(0);
        }
        heard.addresses.push(address);
        Some(record(peer, heard))
    }

    /// Every peer heard of and not yet forgotten.
    pub fn known(&self) -> Vec<PeerRecord> {
        self.peers
            .iter()
            .map(|(&peer, heard)| record(peer, heard))
            .collect()
    }

    /// Forgets peers not heard from for a while, and sources replied to long enough ago.
    pub fn expire(&mut self, now: Instant) {
        self.peers
            .retain(|_, heard| now < heard.last + self.forget_after);
        self.replies
            .retain(|_, last| now < *last + self.reply_interval);
    }
}

fn record(peer: PeerId, heard: &Heard) -> PeerRecord {
    PeerRecord {
        peer_id: peer,
        addresses: heard.addresses.clone(),
        relay: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempest_protocol::Hash;

    fn peer(byte: u8) -> PeerId {
        Hash::new([byte; 32])
    }

    fn datagram(kind: LanKind, from: u8, port: u16) -> [u8; 39] {
        LanMessage {
            kind,
            peer_id: peer(from),
            port,
        }
        .encode()
    }

    fn source(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    fn state() -> LanState {
        LanState::new(peer(0), &LanConfig::default())
    }

    #[test]
    fn an_announce_makes_its_sender_known_once() {
        let now = Instant::now();
        let mut state = state();
        let received = state.receive(
            now,
            source("192.168.1.7:40000"),
            &datagram(LanKind::Announce, 1, 4773),
        );
        assert_eq!(received.reply_to, None);
        let record = received.discovered.unwrap();
        assert_eq!(record.peer_id, peer(1));
        // The IP comes from the datagram, the port from the message.
        assert_eq!(
            record.addresses,
            [Address::from(source("192.168.1.7:4773"))]
        );

        let again = state.receive(
            now,
            source("192.168.1.7:40001"),
            &datagram(LanKind::Announce, 1, 4773),
        );
        assert_eq!(again, Received::default());
        assert_eq!(state.known(), [record]);
    }

    #[test]
    fn a_new_address_is_news() {
        let now = Instant::now();
        let mut state = state();
        state.receive(
            now,
            source("192.168.1.7:1"),
            &datagram(LanKind::Announce, 1, 4773),
        );
        let received = state.receive(
            now,
            source("[2001:db8::7]:1"),
            &datagram(LanKind::Announce, 1, 4773),
        );
        let addresses: Vec<String> = received
            .discovered
            .unwrap()
            .addresses
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(addresses, ["192.168.1.7:4773", "[2001:db8::7]:4773"]);
    }

    #[test]
    fn queries_get_one_reply_per_second_per_source() {
        let start = Instant::now();
        let mut state = state();
        let query = datagram(LanKind::Query, 1, 4773);
        let from = source("10.0.0.2:5555");
        let first = state.receive(start, from, &query);
        assert_eq!(first.reply_to, Some(from));
        assert!(first.discovered.is_some(), "a query reveals its sender too");
        let soon = start + Duration::from_millis(500);
        assert_eq!(state.receive(soon, from, &query).reply_to, None);
        let other = source("10.0.0.3:5555");
        assert_eq!(state.receive(soon, other, &query).reply_to, Some(other));
        let later = start + Duration::from_secs(1);
        assert_eq!(state.receive(later, from, &query).reply_to, Some(from));
    }

    #[test]
    fn junk_and_own_messages_are_ignored() {
        let now = Instant::now();
        let mut state = state();
        let from = source("10.0.0.2:5555");
        let own = datagram(LanKind::Query, 0, 4773);
        assert_eq!(state.receive(now, from, &own), Received::default());
        let mut long = datagram(LanKind::Announce, 1, 4773).to_vec();
        long.push(0);
        assert_eq!(state.receive(now, from, &long), Received::default());
        let mut unknown = datagram(LanKind::Announce, 1, 4773);
        unknown[4] = 3;
        assert_eq!(state.receive(now, from, &unknown), Received::default());
        assert!(state.known().is_empty());
    }

    #[test]
    fn unreachable_senders_are_not_known() {
        let now = Instant::now();
        let mut state = state();
        let zero_port = datagram(LanKind::Announce, 1, 0);
        assert_eq!(
            state
                .receive(now, source("10.0.0.2:1"), &zero_port)
                .discovered,
            None
        );
        let link_local = datagram(LanKind::Query, 2, 4773);
        let received = state.receive(now, source("[fe80::1]:1"), &link_local);
        assert_eq!(received.discovered, None);
        // It can still be answered: the reply goes to the socket address, scope and all.
        assert!(received.reply_to.is_some());
        assert!(state.known().is_empty());
    }

    #[test]
    fn mapped_sources_are_read_as_ipv4() {
        let mut state = state();
        let received = state.receive(
            Instant::now(),
            source("[::ffff:10.0.0.2]:1"),
            &datagram(LanKind::Announce, 1, 4773),
        );
        let addresses = received.discovered.unwrap().addresses;
        assert_eq!(addresses[0].to_string(), "10.0.0.2:4773");
    }

    #[test]
    fn quiet_peers_are_forgotten() {
        let start = Instant::now();
        let mut state = state();
        state.receive(
            start,
            source("10.0.0.2:1"),
            &datagram(LanKind::Announce, 1, 4773),
        );
        state.receive(
            start + Duration::from_secs(120),
            source("10.0.0.3:1"),
            &datagram(LanKind::Announce, 2, 4773),
        );
        state.expire(start + Duration::from_secs(180));
        let known: Vec<PeerId> = state.known().iter().map(|r| r.peer_id).collect();
        assert_eq!(known, [peer(2)]);
    }

    #[test]
    fn memory_is_bounded() {
        let now = Instant::now();
        let config = LanConfig {
            max_peers: 2,
            max_addresses: 2,
            ..LanConfig::default()
        };
        let mut state = LanState::new(peer(0), &config);
        for from in 1..=3 {
            state.receive(
                now,
                source("10.0.0.2:1"),
                &datagram(LanKind::Announce, from, 4773),
            );
        }
        assert_eq!(state.known().len(), 2);
        for port in 1..=3 {
            state.receive(
                now,
                source("10.0.0.2:1"),
                &datagram(LanKind::Announce, 1, port),
            );
        }
        let record = state
            .known()
            .into_iter()
            .find(|r| r.peer_id == peer(1))
            .unwrap();
        let ports: Vec<u16> = record.addresses.iter().map(Address::port).collect();
        assert_eq!(ports, [2, 3], "the oldest address makes way");
    }

    proptest::proptest! {
        /// Whatever arrives, from wherever, memory stays within its bounds and only LAN
        /// messages from others have any effect.
        #[test]
        fn any_datagrams_keep_memory_bounded(
            datagrams in proptest::collection::vec(
                (
                    proptest::collection::vec(proptest::prelude::any::<u8>(), 0..48),
                    proptest::prelude::any::<u8>(),
                    proptest::prelude::any::<u8>(),
                    proptest::prelude::any::<u16>(),
                    proptest::prelude::any::<bool>(),
                ),
                0..200,
            ),
        ) {
            let config = LanConfig { max_peers: 4, max_addresses: 2, ..LanConfig::default() };
            let mut state = LanState::new(peer(0), &config);
            let now = Instant::now();
            for (junk, from, host, port, query) in datagrams {
                let source = SocketAddr::from(([10, 0, 0, host], 5000));
                let bytes = if junk.len() % 2 == 0 {
                    let kind = if query { LanKind::Query } else { LanKind::Announce };
                    datagram(kind, from % 8, port).to_vec()
                } else {
                    junk
                };
                let received = state.receive(now, source, &bytes);
                if received != Received::default() {
                    let message = LanMessage::decode(&bytes).unwrap();
                    proptest::prop_assert_ne!(message.peer_id, peer(0));
                }
                proptest::prop_assert!(state.peers.len() <= 4);
                proptest::prop_assert!(state.replies.len() <= 4);
                proptest::prop_assert!(state.known().iter().all(|r| r.addresses.len() <= 2));
            }
        }
    }

    fn system(ip: &str, netmask: &str, index: u32) -> SystemAddress {
        SystemAddress {
            ip: ip.parse().unwrap(),
            netmask: netmask.parse().unwrap(),
            index,
        }
    }

    #[test]
    fn interfaces_follow_the_advertised_addresses() {
        let system = [
            system("127.0.0.1", "255.0.0.0", 1),
            system("192.168.1.20", "255.255.255.0", 4),
            system("fe80::1", "ffff:ffff:ffff:ffff::", 4),
            system("2001:db8::20", "ffff:ffff:ffff:ffff::", 4),
        ];
        let wildcard = select_interfaces(&[source("0.0.0.0:4773"), source("[::]:4773")], &system);
        let addresses: Vec<String> = wildcard.iter().map(|i| i.address.to_string()).collect();
        assert_eq!(addresses, ["192.168.1.20:4773", "[2001:db8::20]:4773"]);
        assert!(wildcard.iter().all(|i| i.index == 4));
        assert!(wildcard[0].contains("192.168.1.99".parse().unwrap()));
        assert!(!wildcard[0].contains("192.168.2.99".parse().unwrap()));
        assert!(wildcard[1].contains("2001:db8::99".parse().unwrap()));
        for (ip, local) in [
            ("192.168.1.99", true),
            ("::ffff:192.168.1.99", true),
            ("2001:db8::99", true),
            ("127.0.0.1", true),
            ("169.254.3.4", true),
            ("fe80::7", true),
            ("192.168.2.99", false),
            ("203.0.113.9", false),
            ("2001:db8:1::99", false),
        ] {
            assert_eq!(on_lan(wildcard.iter(), ip.parse().unwrap()), local, "{ip}");
        }

        let loopback = select_interfaces(&[source("127.0.0.1:5000")], &system);
        assert_eq!(loopback[0].address.to_string(), "127.0.0.1:5000");
        assert_eq!(loopback[0].index, 1);
    }
}
