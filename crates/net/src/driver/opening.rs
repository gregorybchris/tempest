//! Bounds on incoming connections that have not yet been handed to a bundle.
//!
//! An accepted connection costs a handshake and a routing task before its Hello names a bundle,
//! and only then do the session's connection limits apply. [`Openings`] caps how many such
//! connections there may be at once, in all and per source, so that nobody can tie the node up
//! with connections that never send Hello.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};

use crate::endpoint::Endpoint;

/// The incoming connections being opened, counted in all and by source.
#[derive(Debug, Clone)]
pub(super) struct Openings {
    counts: Arc<Mutex<Counts>>,
    max: usize,
    max_per_source: usize,
}

#[derive(Debug, Default)]
struct Counts {
    total: usize,
    by_source: HashMap<IpAddr, usize>,
}

impl Openings {
    pub(super) fn new(max: usize, max_per_source: usize) -> Self {
        Self {
            counts: Arc::default(),
            max,
            max_per_source,
        }
    }

    /// Takes a place for a connection from `source` (see [`source`]), unless a limit is reached.
    pub(super) fn try_open(&self, source: IpAddr) -> Option<Opening> {
        let mut counts = self.counts.lock().expect("never poisoned");
        let from_source = counts.by_source.get(&source).copied().unwrap_or(0);
        if counts.total >= self.max || from_source >= self.max_per_source {
            return None;
        }
        counts.total += 1;
        counts.by_source.insert(source, from_source + 1);
        Some(Opening {
            counts: self.counts.clone(),
            source,
        })
    }
}

/// A place taken by one incoming connection, given back when dropped.
#[derive(Debug)]
pub(super) struct Opening {
    counts: Arc<Mutex<Counts>>,
    source: IpAddr,
}

impl Drop for Opening {
    fn drop(&mut self) {
        let mut counts = self.counts.lock().expect("never poisoned");
        counts.total -= 1;
        if let Some(count) = counts.by_source.get_mut(&self.source) {
            *count -= 1;
            if *count == 0 {
                counts.by_source.remove(&self.source);
            }
        }
    }
}

/// Where a connection attempt from `addr` comes from, for counting per source: the sender's IP
/// as the home relay sees it for a relayed attempt, an IPv6 address's /64 prefix, since one host
/// commonly holds a whole /64, and otherwise the IP.
pub(super) fn source(endpoint: &Endpoint, addr: SocketAddr) -> IpAddr {
    let ip = endpoint
        .relayed_sender(addr)
        .map_or(addr.ip(), |sender| SocketAddr::from(sender).ip());
    group(ip)
}

/// The source an IP counts as: an IPv6 address's /64 prefix, or an IPv4 address.
fn group(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(ip) => {
            let [a, b, c, d, ..] = ip.segments();
            IpAddr::V6(Ipv6Addr::new(a, b, c, d, 0, 0, 0, 0))
        }
        v4 => v4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_are_limited_in_all_and_per_source_and_given_back() {
        let openings = Openings::new(3, 2);
        let a: IpAddr = "192.0.2.1".parse().unwrap();
        let b: IpAddr = "192.0.2.2".parse().unwrap();
        let first = openings.try_open(a).unwrap();
        let _second = openings.try_open(a).unwrap();
        assert!(openings.try_open(a).is_none(), "per source");
        let _third = openings.try_open(b).unwrap();
        assert!(openings.try_open(b).is_none(), "in all");
        drop(first);
        assert!(openings.try_open(a).is_some());
    }

    #[test]
    fn ipv6_sources_are_grouped_by_prefix() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        assert_eq!(group(ip("2001:db8:1:2:3:4:5:6")), ip("2001:db8:1:2::"));
        assert_eq!(group(ip("::ffff:192.0.2.1")), ip("192.0.2.1"));
        assert_eq!(group(ip("192.0.2.1")), ip("192.0.2.1"));
    }
}
