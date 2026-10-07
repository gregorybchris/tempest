//! The forwarding rule (`protocol/relays.md`, Forwarding datagrams): a registered peer's
//! packets go only to addresses that sent its relay address a datagram within the last
//! [`LIFETIME`].

use std::collections::HashMap;
use std::time::Duration;

use tempest_protocol::Address;
use tokio::time::Instant;

/// How long a datagram from an address lets the registered peer send to it.
pub const LIFETIME: Duration = Duration::from_secs(5 * 60);

/// The addresses a registered peer may send to, with when each last sent to it.
#[derive(Debug)]
pub struct Permissions {
    granted: HashMap<Address, Instant>,
    max: usize,
}

impl Permissions {
    /// No permissions yet. At most `max` addresses are remembered, so datagrams from many
    /// (perhaps forged) sources can't use up the relay's memory; when full, new addresses get no
    /// permission until old ones lapse.
    pub fn new(max: usize) -> Self {
        Self {
            granted: HashMap::new(),
            max,
        }
    }

    /// Records a datagram from `from`.
    pub fn grant(&mut self, from: Address, now: Instant) {
        if self.granted.len() >= self.max && !self.granted.contains_key(&from) {
            self.granted
                .retain(|_, at| now.duration_since(*at) < LIFETIME);
            if self.granted.len() >= self.max {
                return;
            }
        }
        self.granted.insert(from, now);
    }

    /// Whether the registered peer may send to `to`.
    pub fn allows(&self, to: Address, now: Instant) -> bool {
        self.granted
            .get(&to)
            .is_some_and(|at| now.duration_since(*at) < LIFETIME)
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
        let mut permissions = Permissions::new(10);
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
        let mut permissions = Permissions::new(2);
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
}
