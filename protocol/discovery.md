# Discovery

> **Status: Draft (version 1).**

This section defines how a peer that knows a bundle ID finds the addresses of other peers in that bundle's swarm.

## Mechanisms

Tempest defines four discovery mechanisms. A client MAY use any subset of them. Clients SHOULD run the mechanisms they use in parallel and connect to peers as soon as any mechanism returns them.

| Mechanism | How it works | Best for |
| --- | --- | --- |
| **Link addresses** | A link carries the sharing peer's peer ID, its direct addresses and its relay address | Direct sends: no lookup needed |
| **LAN discovery** | Peers announce their presence on the local network, then ask each other about bundles over encrypted connections | Peers on the same home or office network |
| **Trackers** | Peers report the bundles they're in to a tracker, which replies with other peers in the same swarm | Public swarms: fast lookups among strangers |
| **Peer exchange** | Connected peers tell each other about other peers in the swarm ([Peers](messages.md#peers)) | Growing from one peer to many |

Every peer that discovery returns SHOULD come with its peer ID, its direct addresses, and its relay address if it has one, so a connecting peer can reach it even when it is behind a router (see [connections.md](connections.md)).

**Rationale.** Each mechanism is fastest in a different situation, and no single one works everywhere. Running them in parallel gives the best time to first peer. A distributed hash table (DHT) is deliberately not part of the protocol yet. It is the most complex mechanism to build well, and the others cover direct sends and tracker-based public swarms. A DHT can be added later as another mechanism without changing the rest of the protocol.

## Links

A Tempest link is a URI with the scheme `tempest:` whose query parameters describe a bundle and, optionally, where to find it. For example:

```text
tempest:?bundle=3f9a…&peer=7c21…&addr=203.0.113.5:4433&relay=…&tracker=https://tracker.example.org&name=Holiday%20Photos
```

| Parameter | Meaning | Occurrences |
| --- | --- | --- |
| `bundle` | The bundle ID, as 64 lowercase hexadecimal characters | Exactly once (REQUIRED) |
| `peer` | The sharing peer's peer ID, as 64 lowercase hexadecimal characters | At most once |
| `addr` | A direct address of the sharing peer, as `ip:port`, with IPv6 addresses in brackets (`[2001:db8::1]:4433`) | Any number |
| `relay` | The sharing peer's relay address, as `ip:port` (lent by its home relay; see [relays.md](relays.md)) | At most once |
| `tracker` | The URL of a tracker that knows about the bundle | Any number |
| `name` | A display-name hint | At most once |

- Values are percent-encoded as in [RFC 3986](https://www.rfc-editor.org/rfc/rfc3986).
- `addr` and `relay` describe the peer named by `peer`, and are meaningless without it.
- A client MUST ignore parameters it doesn't recognize.
- The `name` hint is not verified. A client MAY display it until the manifest arrives, and MUST then use the manifest's `name` instead.

**Rationale.** A readable link can be inspected and edited by hand, and its parts can be taught one at a time. Hex matches the output of standard tools like `sha256sum`, so anyone can check a bundle ID themselves.

## LAN discovery

Peers on the same local network find each other with UDP multicast. Announcements reveal only that a Tempest peer is present, never which bundles it has. A peer that learns of another peer on the LAN connects to it and asks for a bundle by opening a connection that names it ([Hello](messages.md#hello)).

### Message

```text
LanMessage {
  magic:   4 bytes   // the ASCII bytes "TMPS"
  kind:    u8        // 1 = announce, 2 = query
  peer_id: hash      // the sender's peer ID
  port:    u16       // the UDP port of the sender's QUIC endpoint
}
```

The message is exactly 39 bytes, encoded with the primitives in [messages.md](messages.md#encoding). A receiver MUST silently ignore any datagram that is not exactly 39 bytes, doesn't start with the magic, or has an unknown `kind`. The sender's IP address is taken from the datagram's source address.

### Addresses

| | Group | Port |
| --- | --- | --- |
| IPv4 | `239.255.84.77` | UDP 4774 |
| IPv6 | `ff12::7465:6d70` | UDP 4774 |

### Behavior

- A participating peer SHOULD send an announce to the groups when it starts and about every 60 seconds afterwards.
- A peer MAY send a query to the groups, for example when it starts or when it begins downloading a bundle.
- A peer that receives a query SHOULD reply with an announce sent directly (unicast) to the query's source address and port. It SHOULD NOT reply to the same source more than once per second.
- A peer MUST ignore its own announcements.

**Rationale.**

- **Bundle IDs are not announced.** Announcing them would reveal what a peer shares to everyone on the network, including untrusted public Wi-Fi.
- **No signatures.** Anyone on the network can forge an announcement, but that only wastes one connection attempt, because connecting verifies the peer ID through TLS ([connections.md](connections.md#peer-identity)).
- **The query.** It lets a newcomer find peers in milliseconds instead of waiting for the next announcement.
- **The addresses.** `239.255.0.0/16` is the IPv4 local scope that applications may choose groups from without registration ([RFC 2365](https://www.rfc-editor.org/rfc/rfc2365)). `ff12::/16` is the IPv6 transient, link-local scope. Port 4774 is unassigned in the IANA registry.

**Implementer notes.**

- Several Tempest clients on one machine can all listen on port 4774 if each binds with address reuse enabled (`SO_REUSEADDR`, plus `SO_REUSEPORT` where available). On some systems, though, a unicast datagram to a shared port reaches only one of those sockets. So send queries from a socket with its own unique port (such as the QUIC socket), and replies will come back to it.
- IPv6 multicast is sent on a specific network interface. Send announcements and queries on each interface that has a local network.
- Multicast is often blocked on corporate and guest networks, so treat LAN discovery as one source among several.

## Trackers

Clients talk to trackers over HTTPS, with JSON request and response bodies. The tracker API is defined in [trackers.md](trackers.md).

**Rationale.** A tracker can then be explored with `curl` or a browser, and built with standard web tooling. The same server can also serve the tracker's website.
