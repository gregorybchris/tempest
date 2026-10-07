//! The wrapping layer beneath QUIC for paths through this peer's home relay
//! (`protocol/relays.md`, Forwarding datagrams).
//!
//! A [`RelaySocket`] sits between quinn and a real UDP socket. Packets that arrive through the
//! relay, as Forwarded datagrams on the relay connection, are handed to quinn as if they came
//! from a made-up **virtual address**, one per relay and sender. quinn's replies to a virtual
//! address are wrapped in Forward datagrams on the relay connection. Everything else passes
//! through untouched.
//!
//! Virtual addresses come from ranges that never carry real traffic: `240.0.0.0/8` on an IPv4
//! socket and the discard-only prefix `100::/64` (RFC 6666) on an IPv6 one. A virtual address is
//! recognizable from its range alone, so sends to real addresses never wait on the layer's lock.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::io::{self, IoSliceMut};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};
use std::time::Instant;

use quinn::udp::{RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, UdpPoller};
use tempest_protocol::{Address, RelayDatagram};

/// The most packets from the relay waiting for quinn to read them. More are dropped, like any
/// packets a busy socket can't take.
const MAX_QUEUED: usize = 1024;

/// The most senders reached through the relay at once. The one used least recently is
/// forgotten to make room.
const MAX_ROUTES: usize = 4096;

/// The port of every virtual address. Any nonzero port would do.
const VIRTUAL_PORT: u16 = 1;

/// Virtual addresses on an IPv4 socket start with this octet.
const VIRTUAL_V4: u8 = 240;

/// Virtual addresses on an IPv6 socket start with these segments (`100::/64`).
const VIRTUAL_V6: [u16; 4] = [0x0100, 0, 0, 0];

/// How many virtual addresses there are per family. The counter wraps after the last.
const VIRTUAL_SPACE: u32 = 1 << 24;

/// Whether `addr` is a virtual address, standing for a sender reached through the relay.
pub fn is_virtual(addr: SocketAddr) -> bool {
    match addr.ip() {
        IpAddr::V4(ip) => ip.octets()[0] == VIRTUAL_V4,
        IpAddr::V6(ip) => ip.segments()[..4] == VIRTUAL_V6,
    }
}

/// The `n`th virtual address of a family.
fn virtual_addr(ipv6: bool, n: u32) -> SocketAddr {
    let ip: IpAddr = if ipv6 {
        let [s0, s1, s2, s3] = VIRTUAL_V6;
        Ipv6Addr::new(s0, s1, s2, s3, 0, 0, (n >> 16) as u16, n as u16).into()
    } else {
        let [_, a, b, c] = n.to_be_bytes();
        Ipv4Addr::new(VIRTUAL_V4, a, b, c).into()
    };
    SocketAddr::new(ip, VIRTUAL_PORT)
}

/// Where a virtual address leads: a sender, through a relay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Route {
    /// The relay's address.
    pub relay: SocketAddr,
    /// The sender's address, as the relay sees it.
    pub from: Address,
}

/// The virtual addresses handed out so far, without I/O.
#[derive(Debug)]
pub(crate) struct VirtualAddrs {
    ipv6: bool,
    capacity: usize,
    by_route: HashMap<Route, SocketAddr>,
    by_addr: HashMap<SocketAddr, (Route, Instant)>,
    /// The next number to try, below [`VIRTUAL_SPACE`].
    next: u32,
}

impl VirtualAddrs {
    pub(crate) fn new(ipv6: bool, capacity: usize) -> Self {
        Self {
            ipv6,
            capacity: capacity.max(1),
            by_route: HashMap::new(),
            by_addr: HashMap::new(),
            next: 0,
        }
    }

    /// The virtual address for `route`, handing out a new one if it has none.
    pub(crate) fn address(&mut self, route: Route, now: Instant) -> SocketAddr {
        if let Some(&addr) = self.by_route.get(&route) {
            self.by_addr.insert(addr, (route, now));
            return addr;
        }
        if self.by_addr.len() >= self.capacity {
            self.forget_least_recent();
        }
        let addr = loop {
            let addr = virtual_addr(self.ipv6, self.next);
            self.next = (self.next + 1) % VIRTUAL_SPACE;
            if !self.by_addr.contains_key(&addr) {
                break addr;
            }
        };
        self.by_route.insert(route, addr);
        self.by_addr.insert(addr, (route, now));
        addr
    }

    /// Where `addr` leads, if it is a virtual address in use.
    pub(crate) fn route(&mut self, addr: SocketAddr, now: Instant) -> Option<Route> {
        let (route, used) = self.by_addr.get_mut(&addr)?;
        *used = now;
        Some(*route)
    }

    /// The number of virtual addresses in use.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.by_addr.len()
    }

    fn forget_least_recent(&mut self) {
        let oldest = self
            .by_addr
            .iter()
            .min_by_key(|(_, (_, used))| *used)
            .map(|(&addr, &(route, _))| (addr, route));
        if let Some((addr, route)) = oldest {
            self.by_addr.remove(&addr);
            self.by_route.remove(&route);
        }
    }
}

/// The relay connection packets to virtual addresses leave through.
#[derive(Debug, Clone)]
struct Link {
    relay: SocketAddr,
    conn: quinn::Connection,
}

#[derive(Debug)]
struct State {
    link: Option<Link>,
    addrs: VirtualAddrs,
    /// Packets from the relay, with their virtual source addresses, for quinn to read.
    queue: VecDeque<(SocketAddr, Vec<u8>)>,
    /// The task to wake when a packet is queued.
    waker: Option<Waker>,
}

/// A UDP socket for quinn that also carries packets through this peer's home relay.
pub(crate) struct RelaySocket {
    inner: Arc<dyn AsyncUdpSocket>,
    state: Mutex<State>,
}

impl fmt::Debug for RelaySocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RelaySocket")
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

impl RelaySocket {
    /// Wraps `inner`, a socket bound to an address of `inner.local_addr()`'s family.
    pub(crate) fn new(inner: Arc<dyn AsyncUdpSocket>) -> io::Result<Self> {
        let ipv6 = inner.local_addr()?.is_ipv6();
        Ok(Self {
            inner,
            state: Mutex::new(State {
                link: None,
                addrs: VirtualAddrs::new(ipv6, MAX_ROUTES),
                queue: VecDeque::new(),
                waker: None,
            }),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .expect("no thread panics holding the state")
    }

    /// Sends packets to virtual addresses through `conn`, the relay connection to `relay`.
    pub(crate) fn set_link(&self, relay: SocketAddr, conn: quinn::Connection) {
        self.state().link = Some(Link { relay, conn });
    }

    /// Stops sending through `conn`, unless another relay connection has replaced it.
    pub(crate) fn clear_link(&self, conn: &quinn::Connection) {
        let mut state = self.state();
        if state
            .link
            .as_ref()
            .is_some_and(|link| link.conn.stable_id() == conn.stable_id())
        {
            state.link = None;
        }
    }

    /// Hands quinn `packet`, which reached this peer's relay address at `relay` from `from`.
    pub(crate) fn deliver(&self, relay: SocketAddr, from: Address, packet: &[u8]) {
        let mut state = self.state();
        if state.queue.len() >= MAX_QUEUED {
            return;
        }
        let addr = state.addrs.address(Route { relay, from }, Instant::now());
        state.queue.push_back((addr, packet.to_vec()));
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    }

    /// The sender a virtual address stands for, as the relay sees it.
    pub(crate) fn sender(&self, addr: SocketAddr) -> Option<Address> {
        let route = self.state().addrs.route(addr, Instant::now())?;
        Some(route.from)
    }

    /// Wraps each of `transmit`'s packets in Forward on the relay connection. Packets with no
    /// route, or too large for the relay connection, are dropped like lost packets.
    fn send_through_relay(&self, transmit: &Transmit) {
        let (route, link) = {
            let mut state = self.state();
            let Some(route) = state.addrs.route(transmit.destination, Instant::now()) else {
                return;
            };
            match &state.link {
                Some(link) if link.relay == route.relay => (route, link.clone()),
                _ => return,
            }
        };
        let segment = transmit.segment_size.unwrap_or(transmit.contents.len());
        for packet in transmit.contents.chunks(segment.max(1)) {
            let datagram = RelayDatagram::Forward {
                to: route.from,
                packet,
            };
            // The relay connection is outside the lock, so its own sends can't wait on it.
            let _ = link.conn.send_datagram(datagram.encode().into());
        }
    }
}

impl AsyncUdpSocket for RelaySocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        self.inner.clone().create_io_poller()
    }

    fn try_send(&self, transmit: &Transmit) -> io::Result<()> {
        if is_virtual(transmit.destination) {
            self.send_through_relay(transmit);
            return Ok(());
        }
        self.inner.try_send(transmit)
    }

    fn poll_recv(
        &self,
        cx: &mut Context,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        {
            let mut state = self.state();
            if !state.queue.is_empty() {
                let mut count = 0;
                for (buf, meta) in bufs.iter_mut().zip(meta.iter_mut()) {
                    let Some((addr, packet)) = state.queue.pop_front() else {
                        break;
                    };
                    // A packet too large for quinn's buffers is dropped.
                    if packet.len() > buf.len() {
                        continue;
                    }
                    buf[..packet.len()].copy_from_slice(&packet);
                    *meta = RecvMeta {
                        addr,
                        len: packet.len(),
                        stride: packet.len(),
                        ecn: None,
                        dst_ip: None,
                    };
                    count += 1;
                }
                if count > 0 {
                    return Poll::Ready(Ok(count));
                }
            }
            // Registered under the lock, so a packet queued from now on wakes this task.
            state.waker = Some(cx.waker().clone());
        }
        self.inner.poll_recv(cx, bufs, meta)
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }

    fn max_transmit_segments(&self) -> usize {
        self.inner.max_transmit_segments()
    }

    fn max_receive_segments(&self) -> usize {
        self.inner.max_receive_segments()
    }

    fn may_fragment(&self) -> bool {
        self.inner.may_fragment()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use proptest::prelude::*;

    use super::*;

    fn route(n: u16) -> Route {
        Route {
            relay: "198.51.100.1:4775".parse().unwrap(),
            from: Address::new([203, 0, 113, 5].into(), n),
        }
    }

    #[test]
    fn virtual_addresses_are_in_reserved_ranges() {
        let v4 = virtual_addr(false, 0x0001_0203);
        assert_eq!(v4, "240.1.2.3:1".parse().unwrap());
        let v6 = virtual_addr(true, 0x0001_0203);
        assert_eq!(v6, "[100::1:203]:1".parse().unwrap());
        assert!(is_virtual(v4) && is_virtual(v6));
        for real in [
            "203.0.113.5:4433",
            "[2001:db8::1]:4433",
            "[::ffff:240.0.0.1]:1",
        ] {
            assert!(!is_virtual(real.parse().unwrap()), "{real}");
        }
    }

    #[test]
    fn a_route_keeps_its_address() {
        let mut addrs = VirtualAddrs::new(false, 8);
        let now = Instant::now();
        let first = addrs.address(route(1), now);
        let second = addrs.address(route(2), now);
        assert_ne!(first, second);
        assert_eq!(addrs.address(route(1), now), first);
        assert_eq!(addrs.route(first, now), Some(route(1)));
        assert_eq!(addrs.route(second, now), Some(route(2)));
        assert_eq!(addrs.route(virtual_addr(false, 7), now), None);
    }

    #[test]
    fn the_least_recently_used_route_makes_room() {
        let mut addrs = VirtualAddrs::new(false, 2);
        let start = Instant::now();
        let later = |secs| start + Duration::from_secs(secs);
        let first = addrs.address(route(1), start);
        let second = addrs.address(route(2), later(1));
        // Using the first route makes the second the least recent.
        addrs.route(first, later(2));
        let third = addrs.address(route(3), later(3));
        assert_eq!(addrs.len(), 2);
        assert_eq!(addrs.route(second, later(4)), None);
        assert_eq!(addrs.route(first, later(4)), Some(route(1)));
        assert_eq!(addrs.route(third, later(4)), Some(route(3)));
    }

    proptest! {
        #[test]
        fn addresses_and_routes_stay_one_to_one(
            ports in prop::collection::vec(0u16..64, 1..200),
            capacity in 1usize..16,
        ) {
            let mut addrs = VirtualAddrs::new(true, capacity);
            let now = Instant::now();
            for (step, port) in ports.into_iter().enumerate() {
                let now = now + Duration::from_millis(step as u64);
                let addr = addrs.address(route(port), now);
                prop_assert!(is_virtual(addr));
                prop_assert_eq!(addrs.route(addr, now), Some(route(port)));
                prop_assert!(addrs.len() <= capacity);
                prop_assert_eq!(addrs.by_route.len(), addrs.by_addr.len());
            }
        }
    }
}
