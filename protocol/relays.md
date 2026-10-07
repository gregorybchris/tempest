# Relays

> **Status: Draft (version 1).**

A **relay** is a public server that helps peers behind routers. It has three jobs:

1. **Reachability:** it lends each registered peer a public **relay address** that anyone can dial.
2. **Forwarding:** it passes traffic between that address and the registered peer.
3. **Observation:** it tells a registered peer what its address looks like from the internet, which helps hole punching.

Traffic through a relay stays end-to-end encrypted between the two peers. A relay learns which addresses are communicating, but not what they send. See [connections.md](connections.md#reaching-peers-behind-routers) for how relays fit into connecting.

```text
 Bob (behind a router) ──── keeps a relay connection open ────► Relay
 Relay ── Registered: "your relay address is 198.51.100.1:40007" ──► Bob
 Bob advertises 198.51.100.1:40007 (in links, tracker announces, Peers)

 Alice ── QUIC packets to 198.51.100.1:40007 ──► Relay ── Forwarded (inside Bob's relay connection) ──► Bob
 Alice ◄── QUIC packets from 198.51.100.1:40007 ── Relay ◄── Forward (inside Bob's relay connection) ── Bob
```

## Relay identity

A relay has an Ed25519 key pair, like a peer. A relay is identified as:

```text
<relay key>@<host>:<port>
```

`<relay key>` is 64 lowercase hexadecimal characters, `<host>` is a DNS name or IP address (IPv6 in brackets), and `<port>` is a UDP port. The default port is **4775**.

For example: `9d1e…@relay.example.org:4775`.

A relay's identity appears in client configuration and in trackers' relay recommendations ([trackers.md](trackers.md)). Links, tracker announces and Peers messages carry a peer's **relay address** instead, which is all a dialing peer needs.

**Rationale.** Identifying a relay by its key works like identifying a peer, so self-hosting a relay needs no domain name or certificate.

## The relay connection

A peer registers by opening a QUIC connection to the relay:

- The peer MUST open the relay connection from the same UDP socket it uses for peer connections. The relay address then reaches the same endpoint, and the peer's router keeps one mapping alive for both.
- The ALPN identifier is `tempest-relay/1`. Both sides authenticate with TLS raw public keys, as in [connections.md](connections.md#peer-identity). The peer MUST close the connection if the relay's key doesn't match the configured identity.
- Both sides MUST support QUIC datagrams ([RFC 9221](https://www.rfc-editor.org/rfc/rfc9221)) and accept datagram payloads of at least 1,300 bytes. That fits a 1,200-byte peer packet (see [connections.md](connections.md#transport)) plus the wrapper.
- The peer SHOULD keep the connection alive, for example with QUIC keep-alive packets about every 15 seconds, so its router doesn't forget the mapping.

The relay connection has its own small set of messages, numbered separately from those in [messages.md](messages.md):

- **Control messages** travel on one bidirectional **control stream** (QUIC stream 0, opened by the peer), using the encoding and framing of [messages.md](messages.md#encoding).
- **Forward** and **Forwarded** travel as QUIC datagrams. They are not framed: each datagram is exactly one of them.

### Control messages

| Type | Message | Sent by |
| --- | --- | --- |
| `0x01` | Register | peer |
| `0x02` | Registered | relay |
| `0x03` | RelayReject | relay |

```text
Register {}                    // no fields

Registered {
  relay_address:    Address    // the address this relay lends to the peer
  observed_address: Address    // the peer's address, as the relay sees it
}

RelayReject {
  reason:  u16                 // 0 = not_allowed, 1 = full, 2 = invalid
  message: string              // human-readable detail; MAY be empty; at most 1 KiB
}
```

**Size limits.** A receiver MUST treat a control frame whose `length` exceeds its type's limit as a protocol violation:

| Message | Maximum `length` |
| --- | --- |
| Register | 1 (exactly its fixed size) |
| Registered | 1 + 2 × 19 (two IPv6 addresses) |
| RelayReject | 1 + 2 + 4 + 1 KiB |

`Address` is defined in [messages.md](messages.md#common-structures).

- The peer sends Register as the first message on the control stream. The relay answers with Registered or RelayReject. After RelayReject, the relay closes the connection.
- If the peer's observed address changes, the relay MAY send a new Registered with the same relay address and the new observed address.
- A peer MUST treat an unknown RelayReject reason like `full`.
- The relay address remains valid while the relay connection is open. If the same peer ID registers again within 5 minutes of disconnecting, the relay SHOULD lend it the same relay address.
- A relay MAY restrict registration to an allowlist of peer IDs (`not_allowed`). It MAY refuse when out of capacity (`full`).

**Implementer notes.** Each registration uses one of the relay's ports. An open relay should limit registrations per source IP address, so one host can't exhaust its ports.

### Forwarding datagrams

```text
Forward {                      // peer → relay
  kind:    u8 = 1
  to:      Address             // where the relay should send the packet
  packet:  …                   // the rest of the datagram: one UDP payload
}

Forwarded {                    // relay → peer
  kind:    u8 = 2
  from:    Address             // who sent the packet to the peer's relay address
  packet:  …                   // the rest of the datagram: one UDP payload
}
```

- When a UDP datagram arrives at a peer's relay address, the relay MUST deliver it to that peer as Forwarded, with `from` set to the datagram's source address.
- When a peer sends Forward, the relay MUST send `packet` as a UDP datagram from the peer's relay address to `to`, but only if `to` has sent a datagram to that relay address within the last 5 minutes. Otherwise the relay MUST drop it.
- A relay MAY drop datagrams to enforce bandwidth limits. QUIC on the inner connection recovers from the loss.
- A receiver MUST drop a datagram with an unknown `kind`.
- On a connection that passes through a relay, both peers MUST send QUIC keep-alive packets at least every 60 seconds, so the forwarding permission never lapses.

**Rationale.**

- **Why wrap at all?** The registered peer's router only accepts packets from the address its relay connection goes to (the relay's own `host:port`), so the relay can't send other peers' packets to it directly from the lent port. Wrapping lets them arrive inside the existing connection. The dialing peer needs nothing special: to it, the relay address is just an address.
- **Datagrams, not streams.** The inner QUIC connection already handles reliability and congestion. Datagrams avoid doing that work twice.
- **The forwarding rule.** It stops a registered peer from using the relay to send traffic to arbitrary hosts. TURN ([RFC 8656](https://www.rfc-editor.org/rfc/rfc8656)) uses the same kind of permission. The permission is granted by any incoming datagram, and a source address can be forged. So the rule limits abuse rather than preventing it entirely; QUIC's own anti-amplification limit (at most 3× what it received from an unverified address) bounds what a forged source can trigger.

**Implementer notes.**

- The registered peer's networking layer unwraps Forwarded datagrams and hands each `packet` to its QUIC endpoint as if it had arrived from the relay path, and wraps replies on that path in Forward. QUIC stacks route by address, so a simple way to keep relayed and direct paths to the same remote distinct is to map each relayed remote to a made-up local address inside the wrapping layer.
- The relay connection's own path must carry UDP payloads of about 1,300 bytes, enough for a 1,200-byte peer packet plus the wrapper and the relay connection's QUIC overhead.
- Tell the user when a connection is relayed, since it will usually be slower.

## Upgrading to a direct connection

Two peers connected through a relay SHOULD try to establish a direct connection:

1. Once the opening sequence completes, the **registered peer** (the one whose relay address was dialed) sends a [Candidates](messages.md#candidates) message on the control stream. The other peer replies with its own Candidates, as [messages.md](messages.md#candidates) requires. Each lists the addresses at which it might be reached directly: its observed public address from Registered, the external address of a port mapping, and IPv6 addresses. A tracker's `your_ip` may supply the IP, but not the port.
2. As soon as it has both sent and received Candidates, each peer opens a new connection for the same bundle to each of the other's candidates, and keeps retrying for a few seconds. The registered peer also dials the `from` address it sees on the other peer's Forwarded datagrams, which is usually the other peer's best candidate. Because both peers dial at about the same time, each one's outgoing packets open its own router to the other's packets. This is the hole punch.
3. A peer stops dialing further candidates once one direct connection has completed its opening sequence. The duplicate-connection rules then apply ([connections.md](connections.md#duplicate-connections)): the peer with the lower peer ID keeps the direct connection and closes the relayed one with Goodbye (`duplicate`). Requests still open on the relayed connection are re-requested on the direct connection.
4. If no direct connection opens, the peers continue over the relay.

**Rationale.** Opening a new direct connection works with standard QUIC. Moving the existing connection onto a new path would require QUIC extensions that aren't yet standard or widely implemented.
