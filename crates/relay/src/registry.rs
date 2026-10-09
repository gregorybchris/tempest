//! Who is registered, which lent socket each holds, and the sockets kept for peers that left.
//!
//! The rules live here without I/O, with time passed in, so they can be tested directly:
//!
//! - Only allowlisted peers may register, if there is an allowlist (`not_allowed`).
//! - Each source IP, or IPv6 /64, may hold a limited number of registrations (`full`).
//! - Lent sockets, held or kept, are limited in number (`full`). A kept socket is given up
//!   before a newcomer is turned away.
//! - A peer that registers again within [`KEEP_FOR`] of leaving gets its socket back, so its
//!   relay address stays the same. So does a peer that registers again while its old connection
//!   is still open; the old connection is then replaced.
//!
//! Registering takes two steps, so that binding a new socket, which may try many ports, happens
//! outside the lock around the registry: [`Registry::check`] says whether a peer may register
//! and needs a new socket, and [`Registry::admit`] registers it, with the socket bound meanwhile.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::time::Duration;

use tempest_protocol::PeerId;
use tempest_protocol::relay::{RelayReject, RelayRejectReason};
use tokio::time::Instant;

use crate::permissions::source;

/// How long a peer's lent socket is kept after it disconnects (`protocol/relays.md`: 5 minutes).
pub const KEEP_FOR: Duration = Duration::from_secs(5 * 60);

/// Who may register, and how many.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    /// The only peers that may register, or `None` for anyone.
    pub allowlist: Option<HashSet<PeerId>>,
    /// The most lent sockets, held or kept.
    pub max_registrations: usize,
    /// The most registrations held from one source IP address, or one IPv6 /64.
    pub max_per_ip: usize,
}

/// Registrations, generic over the lent socket `S` and the connection `C` that holds it.
#[derive(Debug)]
pub struct Registry<S, C> {
    limits: Limits,
    held: HashMap<PeerId, Held<S, C>>,
    kept: HashMap<PeerId, Kept<S>>,
    next_generation: u64,
}

#[derive(Debug)]
struct Held<S, C> {
    socket: S,
    connection: C,
    ip: IpAddr,
    generation: u64,
}

#[derive(Debug)]
struct Kept<S> {
    socket: S,
    until: Instant,
}

/// A registration that was let in.
#[derive(Debug)]
pub struct Admitted<S, C> {
    /// The lent socket.
    pub socket: S,
    /// Identifies this registration when it ends; see [`Registry::release`].
    pub generation: u64,
    /// The connection this registration replaced, which the caller should close.
    pub replaced: Option<C>,
}

impl<S: Clone, C> Registry<S, C> {
    /// An empty registry.
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            held: HashMap::new(),
            kept: HashMap::new(),
            next_generation: 0,
        }
    }

    /// The number of peers registered now.
    pub fn registered(&self) -> usize {
        self.held.len()
    }

    /// The number of lent sockets, held or kept.
    pub fn sockets(&self) -> usize {
        self.held.len() + self.kept.len()
    }

    /// Whether `peer`, connected from `ip`, may register, and if so whether it needs a new
    /// socket: `false` if it has one held or kept. Refuses with the RelayReject to send.
    pub fn check(&mut self, peer: PeerId, ip: IpAddr, now: Instant) -> Result<bool, RelayReject> {
        self.expire(now);
        if let Some(allowlist) = &self.limits.allowlist
            && !allowlist.contains(&peer)
        {
            return Err(reject(
                RelayRejectReason::NotAllowed,
                "not on the allowlist",
            ));
        }
        let from_ip = self
            .held
            .iter()
            .filter(|(id, held)| **id != peer && source(held.ip) == source(ip))
            .count();
        if from_ip >= self.limits.max_per_ip {
            return Err(reject(
                RelayRejectReason::Full,
                "too many registrations from this IP address",
            ));
        }
        if self.held.contains_key(&peer) || self.kept.contains_key(&peer) {
            return Ok(false);
        }
        if self.sockets() >= self.limits.max_registrations && !self.give_up_oldest_kept() {
            return Err(reject(RelayRejectReason::Full, "the relay is full"));
        }
        Ok(true)
    }

    /// Lets `peer`, connected from `ip` over `connection`, register, giving back its socket if
    /// it has one and otherwise `fresh`, bound since [`check`](Self::check) asked for one.
    /// `fresh` is dropped if the peer doesn't need it. Refuses with the RelayReject to send,
    /// such as when the peer needs a socket and none was bound.
    pub fn admit(
        &mut self,
        peer: PeerId,
        ip: IpAddr,
        connection: C,
        now: Instant,
        fresh: Option<S>,
    ) -> Result<Admitted<S, C>, RelayReject> {
        self.check(peer, ip, now)?;
        let (socket, replaced) = if let Some(held) = self.held.remove(&peer) {
            (held.socket, Some(held.connection))
        } else if let Some(kept) = self.kept.remove(&peer) {
            (kept.socket, None)
        } else {
            let busy = || {
                reject(
                    RelayRejectReason::Full,
                    "the relay had no port ready to lend",
                )
            };
            (fresh.ok_or_else(busy)?, None)
        };

        let generation = self.next_generation;
        self.next_generation += 1;
        self.held.insert(
            peer,
            Held {
                socket: socket.clone(),
                connection,
                ip,
                generation,
            },
        );
        Ok(Admitted {
            socket,
            generation,
            replaced,
        })
    }

    /// Ends `peer`'s registration `generation`, keeping its socket for [`KEEP_FOR`]. Does
    /// nothing if that registration was already replaced.
    pub fn release(&mut self, peer: PeerId, generation: u64, now: Instant) {
        if self
            .held
            .get(&peer)
            .is_some_and(|held| held.generation == generation)
        {
            let held = self.held.remove(&peer).expect("checked above");
            self.kept.insert(
                peer,
                Kept {
                    socket: held.socket,
                    until: now + KEEP_FOR,
                },
            );
        }
    }

    /// Gives up the sockets kept for longer than [`KEEP_FOR`].
    pub fn expire(&mut self, now: Instant) {
        self.kept.retain(|_, kept| kept.until > now);
    }

    /// Gives up the kept socket closest to expiring. Returns whether there was one.
    fn give_up_oldest_kept(&mut self) -> bool {
        let oldest = self
            .kept
            .iter()
            .min_by_key(|(_, kept)| kept.until)
            .map(|(peer, _)| *peer);
        oldest.is_some_and(|peer| self.kept.remove(&peer).is_some())
    }
}

fn reject(reason: RelayRejectReason, message: &str) -> RelayReject {
    RelayReject {
        reason,
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::net::Ipv4Addr;

    use proptest::prelude::*;

    use super::*;

    const IP: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1));
    const OTHER_IP: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 2));

    fn peer(n: u8) -> PeerId {
        PeerId::new([n; 32])
    }

    fn limits() -> Limits {
        Limits {
            allowlist: None,
            max_registrations: 3,
            max_per_ip: 2,
        }
    }

    /// A registry whose sockets are numbered in the order they were bound, and whose
    /// connections are labelled. Admitting checks first and binds only if asked to, as the
    /// relay does.
    struct Test {
        registry: Registry<u32, &'static str>,
        bound: Cell<u32>,
        now: Instant,
    }

    impl Test {
        fn new(limits: Limits) -> Self {
            Self {
                registry: Registry::new(limits),
                bound: Cell::new(0),
                now: Instant::now(),
            }
        }

        fn admit(
            &mut self,
            peer: PeerId,
            ip: IpAddr,
            connection: &'static str,
        ) -> Result<Admitted<u32, &'static str>, RelayRejectReason> {
            let fresh = match self.registry.check(peer, ip, self.now) {
                Ok(true) => {
                    self.bound.set(self.bound.get() + 1);
                    Some(self.bound.get())
                }
                Ok(false) => None,
                Err(reject) => return Err(reject.reason),
            };
            self.registry
                .admit(peer, ip, connection, self.now, fresh)
                .map_err(|reject| reject.reason)
        }
    }

    #[test]
    fn only_the_allowlist_may_register() {
        let mut test = Test::new(Limits {
            allowlist: Some([peer(1)].into()),
            ..limits()
        });
        assert!(test.admit(peer(1), IP, "a").is_ok());
        assert_eq!(
            test.admit(peer(2), IP, "b").unwrap_err(),
            RelayRejectReason::NotAllowed
        );
    }

    #[test]
    fn each_ip_holds_a_limited_number() {
        let mut test = Test::new(limits());
        test.admit(peer(1), IP, "a").unwrap();
        test.admit(peer(2), IP, "b").unwrap();
        assert_eq!(
            test.admit(peer(3), IP, "c").unwrap_err(),
            RelayRejectReason::Full
        );
        assert!(test.admit(peer(3), OTHER_IP, "c").is_ok());
        // Registering again doesn't count against the peer itself.
        assert!(test.admit(peer(1), IP, "a2").is_ok());

        // An IPv6 /64 counts as one source.
        let mut test = Test::new(limits());
        let v6 = |s: &str| s.parse::<IpAddr>().unwrap();
        test.admit(peer(1), v6("2001:db8::1"), "a").unwrap();
        test.admit(peer(2), v6("2001:db8::2"), "b").unwrap();
        assert!(test.admit(peer(3), v6("2001:db8::3"), "c").is_err());
        assert!(test.admit(peer(3), v6("2001:db8:0:1::3"), "c").is_ok());
    }

    #[test]
    fn a_returning_peer_gets_its_socket_back_within_five_minutes() {
        let mut test = Test::new(limits());
        let first = test.admit(peer(1), IP, "a").unwrap();
        test.registry.release(peer(1), first.generation, test.now);
        assert_eq!(test.registry.registered(), 0);
        assert_eq!(test.registry.sockets(), 1);

        test.now += KEEP_FOR - Duration::from_secs(1);
        assert_eq!(test.admit(peer(1), OTHER_IP, "a").unwrap().socket, 1);
        assert_eq!(test.bound.get(), 1);
    }

    #[test]
    fn a_kept_socket_expires_after_five_minutes() {
        let mut test = Test::new(limits());
        let first = test.admit(peer(1), IP, "a").unwrap();
        test.registry.release(peer(1), first.generation, test.now);
        test.now += KEEP_FOR;
        test.registry.expire(test.now);
        assert_eq!(test.registry.sockets(), 0);
        assert_eq!(test.admit(peer(1), IP, "a").unwrap().socket, 2);
    }

    #[test]
    fn registering_again_replaces_the_old_connection() {
        let mut test = Test::new(limits());
        let old = test.admit(peer(1), IP, "old").unwrap();
        let new = test.admit(peer(1), IP, "new").unwrap();
        assert_eq!(new.socket, old.socket);
        assert_eq!(new.replaced, Some("old"));

        // The old connection ending doesn't end the new registration.
        test.registry.release(peer(1), old.generation, test.now);
        assert_eq!(test.registry.registered(), 1);
        test.registry.release(peer(1), new.generation, test.now);
        assert_eq!(test.registry.registered(), 0);
    }

    #[test]
    fn kept_sockets_make_way_for_newcomers_oldest_first() {
        let mut test = Test::new(limits());
        for n in 1..=3 {
            let ip = IpAddr::V4(Ipv4Addr::new(192, 0, 2, n));
            let admitted = test.admit(peer(n), ip, "x").unwrap();
            if n < 3 {
                test.registry
                    .release(peer(n), admitted.generation, test.now);
                test.now += Duration::from_secs(1);
            }
        }
        assert!(test.admit(peer(4), OTHER_IP, "x").is_ok());
        assert_eq!(test.registry.sockets(), 3);
        // Peer 1's socket was given up; peer 2's was kept.
        assert_eq!(test.admit(peer(2), IP, "x").unwrap().socket, 2);
        assert_eq!(
            test.admit(peer(1), IP, "x").unwrap_err(),
            RelayRejectReason::Full
        );
    }

    #[test]
    fn a_newcomer_without_a_socket_is_refused() {
        let mut registry: Registry<u32, ()> = Registry::new(limits());
        let refused = registry
            .admit(peer(1), IP, (), Instant::now(), None)
            .unwrap_err();
        assert_eq!(refused.reason, RelayRejectReason::Full);
        assert_eq!(registry.sockets(), 0);
    }

    #[test]
    fn a_socket_bound_for_a_returning_peer_is_not_used() {
        let mut registry: Registry<u32, ()> = Registry::new(limits());
        let now = Instant::now();
        assert_eq!(registry.check(peer(1), IP, now), Ok(true));
        let first = registry.admit(peer(1), IP, (), now, Some(7)).unwrap();
        registry.release(peer(1), first.generation, now);
        assert_eq!(registry.check(peer(1), IP, now), Ok(false));
        // Meanwhile another task bound a socket anyway; the kept one is given back.
        let again = registry.admit(peer(1), IP, (), now, Some(8)).unwrap();
        assert_eq!(again.socket, 7);
        assert_eq!(registry.sockets(), 1);
    }

    #[derive(Debug, Clone)]
    enum Op {
        Admit { peer: u8, ip: u8 },
        Release { peer: u8 },
        Wait { secs: u64 },
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            (0..6u8, 0..3u8).prop_map(|(peer, ip)| Op::Admit { peer, ip }),
            (0..6u8).prop_map(|peer| Op::Release { peer }),
            (0..200u64).prop_map(|secs| Op::Wait { secs }),
        ]
    }

    proptest! {
        #[test]
        fn limits_always_hold(ops in prop::collection::vec(op(), 0..60)) {
            let mut test = Test::new(limits());
            let mut generations = HashMap::new();
            let mut addresses: HashMap<u8, u32> = HashMap::new();
            for op in ops {
                match op {
                    Op::Admit { peer: n, ip } => {
                        let ip = IpAddr::V4(Ipv4Addr::new(192, 0, 2, ip));
                        if let Ok(admitted) = test.admit(peer(n), ip, "x") {
                            generations.insert(n, admitted.generation);
                            // No two peers ever hold the same socket.
                            for (other, socket) in &addresses {
                                if *other != n && test.registry.held.contains_key(&peer(*other)) {
                                    prop_assert_ne!(*socket, admitted.socket);
                                }
                            }
                            addresses.insert(n, admitted.socket);
                        }
                    }
                    Op::Release { peer: n } => {
                        if let Some(generation) = generations.get(&n) {
                            test.registry.release(peer(n), *generation, test.now);
                        }
                    }
                    Op::Wait { secs } => test.now += Duration::from_secs(secs),
                }
                prop_assert!(test.registry.sockets() <= 3);
                for ip in [0, 1, 2] {
                    let ip = IpAddr::V4(Ipv4Addr::new(192, 0, 2, ip));
                    let held = test.registry.held.values().filter(|h| h.ip == ip).count();
                    prop_assert!(held <= 2);
                }
                for id in test.registry.held.keys() {
                    prop_assert!(!test.registry.kept.contains_key(id));
                }
            }
        }
    }
}
