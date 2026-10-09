//! The forwarding rule (`protocol/relays.md`, Forwarding datagrams): a registered peer's
//! packets go only to addresses that sent its relay address a datagram within the last
//! [`LIFETIME`].

use std::collections::{BTreeSet, HashMap};
use std::net::IpAddr;
use std::time::Duration;

use tempest_protocol::Address;
use tokio::time::Instant;

/// How long a datagram from an address lets the registered peer send to it.
pub const LIFETIME: Duration = Duration::from_secs(5 * 60);

/// The addresses a registered peer may send to, with when each last sent to it.
///
/// At most `max` addresses are remembered, so datagrams from many (perhaps forged) sources can't
/// use up the relay's memory, and at most `max_per_source` from one IP address or IPv6 /64, so
/// one host can't fill the table alone. Beyond either limit, a new address takes the place of
/// the oldest one the registered peer has never sent to. Datagrams with forged sources never
/// get answers, so they can't crowd out the addresses the peer is talking with. When every
/// address in the way has been sent to, new ones get no permission until old ones lapse.
#[derive(Debug)]
pub struct Permissions {
    granted: HashMap<Address, Grant>,
    /// The grants, oldest first, so lapsed ones are found without a scan.
    by_age: BTreeSet<(Instant, Address)>,
    /// The grants the registered peer has never sent to, oldest first.
    unused: BTreeSet<(Instant, Address)>,
    per_source: HashMap<IpAddr, usize>,
    max: usize,
    max_per_source: usize,
}

#[derive(Debug, Clone, Copy)]
struct Grant {
    /// When the address last sent a datagram.
    at: Instant,
    /// Whether the registered peer has sent to it.
    used: bool,
}

impl Permissions {
    /// No permissions yet.
    pub fn new(max: usize, max_per_source: usize) -> Self {
        Self {
            granted: HashMap::new(),
            by_age: BTreeSet::new(),
            unused: BTreeSet::new(),
            per_source: HashMap::new(),
            max,
            max_per_source,
        }
    }

    /// Records a datagram from `from`.
    pub fn grant(&mut self, from: Address, now: Instant) {
        if let Some(grant) = self.granted.get_mut(&from) {
            self.by_age.remove(&(grant.at, from));
            self.by_age.insert((now, from));
            if !grant.used {
                self.unused.remove(&(grant.at, from));
                self.unused.insert((now, from));
            }
            grant.at = now;
            return;
        }
        let source = source(from.ip());
        if self.source_full(source) || self.granted.len() >= self.max {
            self.forget_lapsed(now);
        }
        if self.source_full(source) {
            let oldest = self
                .unused
                .iter()
                .find(|(_, addr)| self::source(addr.ip()) == source);
            match oldest.copied() {
                Some((_, addr)) => self.forget(addr),
                None => return,
            }
        }
        if self.granted.len() >= self.max {
            match self.unused.first().copied() {
                Some((_, addr)) => self.forget(addr),
                None => return,
            }
        }
        self.granted.insert(
            from,
            Grant {
                at: now,
                used: false,
            },
        );
        self.by_age.insert((now, from));
        self.unused.insert((now, from));
        *self.per_source.entry(source).or_default() += 1;
    }

    /// Whether the registered peer may send to `to`, recording that it does if so.
    pub fn send(&mut self, to: Address, now: Instant) -> bool {
        let Some(grant) = self.granted.get_mut(&to) else {
            return false;
        };
        if now.duration_since(grant.at) >= LIFETIME {
            return false;
        }
        if !grant.used {
            grant.used = true;
            self.unused.remove(&(grant.at, to));
        }
        true
    }

    /// Whether the registered peer may send to `to`.
    pub fn allows(&self, to: Address, now: Instant) -> bool {
        self.granted
            .get(&to)
            .is_some_and(|grant| now.duration_since(grant.at) < LIFETIME)
    }

    fn source_full(&self, source: IpAddr) -> bool {
        self.per_source.get(&source).copied().unwrap_or(0) >= self.max_per_source
    }

    fn forget_lapsed(&mut self, now: Instant) {
        while let Some(&(at, addr)) = self.by_age.first() {
            if now.duration_since(at) < LIFETIME {
                break;
            }
            self.forget(addr);
        }
    }

    fn forget(&mut self, addr: Address) {
        let Some(grant) = self.granted.remove(&addr) else {
            return;
        };
        self.by_age.remove(&(grant.at, addr));
        self.unused.remove(&(grant.at, addr));
        let source = source(addr.ip());
        if let Some(count) = self.per_source.get_mut(&source) {
            *count -= 1;
            if *count == 0 {
                self.per_source.remove(&source);
            }
        }
    }
}

/// The source an address counts against: its IPv4 address, or its IPv6 /64, which one host
/// usually controls whole.
pub(crate) fn source(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => IpAddr::V6((u128::from(v6) & !((1u128 << 64) - 1)).into()),
        v4 => v4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(n: u16) -> Address {
        format!("198.51.100.7:{n}").parse().unwrap()
    }

    fn other(n: u8) -> Address {
        format!("203.0.113.{n}:1").parse().unwrap()
    }

    #[test]
    fn a_datagram_grants_permission_for_five_minutes() {
        let mut permissions = Permissions::new(10, 10);
        let start = Instant::now();
        assert!(!permissions.allows(addr(1), start));
        permissions.grant(addr(1), start);
        assert!(permissions.allows(addr(1), start + LIFETIME - Duration::from_millis(1)));
        assert!(!permissions.allows(addr(1), start + LIFETIME));
        assert!(!permissions.send(addr(1), start + LIFETIME));
        assert!(!permissions.allows(addr(2), start));

        // Another datagram renews it.
        permissions.grant(addr(1), start + LIFETIME);
        assert!(permissions.send(addr(1), start + LIFETIME));
    }

    #[test]
    fn new_addresses_replace_the_oldest_never_sent_to() {
        let mut permissions = Permissions::new(3, 10);
        let start = Instant::now();
        let at = |secs| start + Duration::from_secs(secs);
        permissions.grant(other(1), at(0));
        permissions.grant(other(2), at(1));
        permissions.grant(other(3), at(2));
        // The peer answers the first: it is kept, and the second, never answered, makes way.
        assert!(permissions.send(other(1), at(3)));
        permissions.grant(other(4), at(4));
        assert!(permissions.allows(other(1), at(4)));
        assert!(!permissions.allows(other(2), at(4)));
        assert!(permissions.allows(other(3), at(4)));
        assert!(permissions.allows(other(4), at(4)));

        // A flood of forged sources churns through the unanswered ones only.
        assert!(permissions.send(other(3), at(5)));
        for n in 10..100 {
            permissions.grant(other(n), at(6));
        }
        assert!(permissions.allows(other(1), at(6)));
        assert!(permissions.allows(other(3), at(6)));
        assert!(permissions.allows(other(99), at(6)));

        // With every place answered, newcomers wait for one to lapse.
        assert!(permissions.send(other(99), at(7)));
        permissions.grant(other(100), at(8));
        assert!(!permissions.allows(other(100), at(8)));
        let later = at(7) + LIFETIME;
        permissions.grant(other(100), later);
        assert!(permissions.allows(other(100), later));
    }

    #[test]
    fn one_source_cannot_fill_the_table() {
        let mut permissions = Permissions::new(100, 3);
        let now = Instant::now();
        for port in 1..=3 {
            permissions.grant(addr(port), now);
            assert!(permissions.send(addr(port), now));
        }
        for port in 4..=10 {
            permissions.grant(addr(port), now);
        }
        assert!(permissions.allows(addr(3), now));
        assert!(!permissions.allows(addr(4), now));
        // Other hosts still get in, and another port of the same IPv6 /64 counts as the same
        // source.
        let other = other(1);
        permissions.grant(other, now);
        assert!(permissions.allows(other, now));
        for n in 1..=4u16 {
            let v6: Address = format!("[2001:db8::{n}]:9").parse().unwrap();
            permissions.grant(v6, now);
            if n <= 3 {
                assert!(permissions.send(v6, now));
            }
            assert_eq!(permissions.allows(v6, now), n <= 3);
        }
        // A source's lapsed grants make room for it again.
        let later = now + LIFETIME;
        permissions.grant(addr(4), later);
        assert!(permissions.allows(addr(4), later));
    }

    #[test]
    fn a_sources_new_address_replaces_its_oldest_unanswered_one() {
        let mut permissions = Permissions::new(100, 2);
        let now = Instant::now();
        permissions.grant(addr(1), now);
        permissions.grant(addr(2), now + Duration::from_secs(1));
        permissions.grant(addr(3), now + Duration::from_secs(2));
        assert!(!permissions.allows(addr(1), now + Duration::from_secs(2)));
        assert!(permissions.allows(addr(2), now + Duration::from_secs(2)));
        assert!(permissions.allows(addr(3), now + Duration::from_secs(2)));
    }
}
