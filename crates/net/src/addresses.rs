//! The direct addresses a peer advertises, in links and in Hello.
//!
//! A socket bound to one address advertises that address. A socket bound to the wildcard
//! address advertises the addresses of the machine's network interfaces in its family, leaving
//! out loopback, which other machines can't reach, and IPv6 link-local addresses, which need a
//! scope ID that an [`Address`] can't carry.

use std::io;
use std::net::{IpAddr, SocketAddr};

use tempest_protocol::Address;

/// The direct addresses for sockets bound to `bound`, from the machine's interfaces.
pub fn direct_addresses(bound: &[SocketAddr]) -> io::Result<Vec<Address>> {
    let interfaces: Vec<IpAddr> = if bound.iter().any(|addr| addr.ip().is_unspecified()) {
        if_addrs::get_if_addrs()?
            .into_iter()
            .map(|interface| interface.ip())
            .collect()
    } else {
        Vec::new()
    };
    Ok(advertise(bound, &interfaces))
}

/// The addresses to advertise for sockets bound to `bound` on a machine whose interfaces have
/// the addresses `interfaces`.
fn advertise(bound: &[SocketAddr], interfaces: &[IpAddr]) -> Vec<Address> {
    let mut addresses = Vec::new();
    for socket in bound {
        if socket.ip().is_unspecified() {
            let reachable = interfaces.iter().filter(|ip| {
                ip.is_ipv4() == socket.is_ipv4() && !ip.is_loopback() && !is_link_local_v6(ip)
            });
            addresses.extend(reachable.map(|&ip| Address::new(ip, socket.port())));
        } else {
            addresses.push(Address::from(*socket));
        }
    }
    let mut seen = Vec::with_capacity(addresses.len());
    addresses.retain(|address| {
        let new = !seen.contains(address);
        seen.push(*address);
        new
    });
    addresses
}

fn is_link_local_v6(ip: &IpAddr) -> bool {
    matches!(ip, IpAddr::V6(v6) if v6.is_unicast_link_local())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ips(text: &[&str]) -> Vec<IpAddr> {
        text.iter().map(|ip| ip.parse().unwrap()).collect()
    }

    fn sockets(text: &[&str]) -> Vec<SocketAddr> {
        text.iter().map(|addr| addr.parse().unwrap()).collect()
    }

    fn shown(addresses: Vec<Address>) -> Vec<String> {
        addresses.iter().map(ToString::to_string).collect()
    }

    const INTERFACES: &[&str] = &[
        "127.0.0.1",
        "192.168.1.20",
        "10.0.0.5",
        "::1",
        "fe80::1",
        "2001:db8::20",
    ];

    #[test]
    fn wildcards_advertise_interfaces_without_loopback_or_link_local() {
        let bound = sockets(&["0.0.0.0:4773", "[::]:4773"]);
        assert_eq!(
            shown(advertise(&bound, &ips(INTERFACES))),
            ["192.168.1.20:4773", "10.0.0.5:4773", "[2001:db8::20]:4773"]
        );
    }

    #[test]
    fn each_family_keeps_its_own_port() {
        let bound = sockets(&["0.0.0.0:5000", "[::]:6000"]);
        assert_eq!(
            shown(advertise(&bound, &ips(&["10.0.0.5", "2001:db8::20"]))),
            ["10.0.0.5:5000", "[2001:db8::20]:6000"]
        );
    }

    #[test]
    fn specific_binds_advertise_themselves_even_on_loopback() {
        let bound = sockets(&["127.0.0.1:4773", "[2001:db8::20]:4773"]);
        assert_eq!(
            shown(advertise(&bound, &ips(INTERFACES))),
            ["127.0.0.1:4773", "[2001:db8::20]:4773"]
        );
    }

    #[test]
    fn duplicates_are_dropped_and_no_interfaces_means_no_addresses() {
        let bound = sockets(&["0.0.0.0:4773"]);
        assert_eq!(
            shown(advertise(&bound, &ips(&["10.0.0.5", "10.0.0.5"]))),
            ["10.0.0.5:4773"]
        );
        assert!(advertise(&bound, &ips(&["127.0.0.1"])).is_empty());
    }

    #[test]
    fn this_machine_lists_its_addresses() {
        let addresses = direct_addresses(&sockets(&["0.0.0.0:4773"])).unwrap();
        assert!(
            addresses
                .iter()
                .all(|a| a.ip().is_ipv4() && !a.ip().is_loopback())
        );
    }
}
