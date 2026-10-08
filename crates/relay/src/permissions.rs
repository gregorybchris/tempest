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
/// one host can't fill the table alone. Beyond either limit, new addresses get no permission
/// until old ones lapse; permissions already granted are never taken away early.
#[derive(Debug)]
pub struct Permissions {
    granted: HashMap<Address, Instant>,
    /// The same grants, oldest first, so lapsed ones are found without a scan.
    by_age: BTreeSet<(Instant, Address)>,
    per_source: HashMap<IpAddr, usize>,
    max: usize,
    max_per_source: usize,
}

impl Permissions {
    /// No permissions yet.
    pub fn new(max: usize, max_per_source: usize) -> Self {
        Self {
            granted: HashMap::new(),
            by_age: BTreeSet::new(),
            per_source: HashMap::new(),
            max,
            max_per_source,
        }
    }

    /// Records a datagram from `from`.
    pub fn grant(&mut self, from: Address, now: Instant) {
        if let Some(at) = self.granted.get_mut(&from) {
            self.by_age.remove(&(*at, from));
            self.by_age.insert((now, from));
            *at = now;
            return;
        }
        let source = source(from.ip());
        let full = |p: &Self| {
            p.granted.len() >= p.max
                || p.per_source.get(&source).copied().unwrap_or(0) >= p.max_per_source
        };
        if full(self) {
            self.forget_lapsed(now);
            if full(self) {
                return;
            }
        }
        self.granted.insert(from, now);
        self.by_age.insert((now, from));
        *self.per_source.entry(source).or_default() += 1;
    }

    /// Whether the registered peer may send to `to`.
    pub fn allows(&self, to: Address, now: Instant) -> bool {
        self.granted
            .get(&to)
            .is_some_and(|at| now.duration_since(*at) < LIFETIME)
    }

    fn forget_lapsed(&mut self, now: Instant) {
        while let Some(&(at, addr)) = self.by_age.first() {
            if now.duration_since(at) < LIFETIME {
                break;
            }
            self.by_age.pop_first();
            self.granted.remove(&addr);
            let source = source(addr.ip());
            if let Some(count) = self.per_source.get_mut(&source) {
                *count -= 1;
                if *count == 0 {
                    self.per_source.remove(&source);
                }
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

    #[test]
    fn a_datagram_grants_permission_for_five_minutes() {
        let mut permissions = Permissions::new(10, 10);
        let start = Instant::now();
        assert!(!permissions.allows(addr(1), start));
        permissions.grant(addr(1), start);
        assert!(permissions.allows(addr(1), start + LIFETIME - Duration::from_millis(1)));
        assert!(!permissions.allows(addr(1), start + LIFETIME));
        assert!(!permissions.allows(addr(2), start));

        // Another datagram renews it.
        permissions.grant(addr(1), start + LIFETIME);
        assert!(permissions.allows(addr(1), start + LIFETIME));
    }

    #[test]
    fn a_full_table_admits_new_addresses_only_as_old_ones_lapse() {
        let mut permissions = Permissions::new(2, 10);
        let start = Instant::now();
        permissions.grant(addr(1), start);
        permissions.grant(addr(2), start + Duration::from_secs(60));
        permissions.grant(addr(3), start + Duration::from_secs(60));
        assert!(!permissions.allows(addr(3), start + Duration::from_secs(60)));

        let later = start + LIFETIME;
        permissions.grant(addr(3), later);
        assert!(permissions.allows(addr(3), later));
        assert!(permissions.allows(addr(2), later));
        assert!(!permissions.allows(addr(1), later));
    }

    #[test]
    fn one_source_cannot_fill_the_table() {
        let mut permissions = Permissions::new(100, 3);
        let now = Instant::now();
        for port in 1..=10 {
            permissions.grant(addr(port), now);
        }
        assert!(permissions.allows(addr(3), now));
        assert!(!permissions.allows(addr(4), now));
        // Other hosts still get in, and another port of the same IPv6 /64 counts as the same
        // source.
        let other: Address = "203.0.113.1:1".parse().unwrap();
        permissions.grant(other, now);
        assert!(permissions.allows(other, now));
        for n in 1..=4u16 {
            let v6: Address = format!("[2001:db8::{n}]:9").parse().unwrap();
            permissions.grant(v6, now);
            assert_eq!(permissions.allows(v6, now), n <= 3);
        }
        // A source's lapsed grants make room for it again.
        let later = now + LIFETIME;
        permissions.grant(addr(4), later);
        assert!(permissions.allows(addr(4), later));
    }
}
