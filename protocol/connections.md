# Connections

> **Status: Draft (version 1).**

This section defines how two Tempest peers establish a secure connection, and how each peer knows who it is talking to.

## Transport

Peers communicate over QUIC version 1 ([RFC 9000](https://www.rfc-editor.org/rfc/rfc9000)) using TLS 1.3 ([RFC 9001](https://www.rfc-editor.org/rfc/rfc9001)).

- Every connection is encrypted and authenticated. There is no unencrypted mode.
- A connection carries exactly one bundle (see [Connections and bundles](#connections-and-bundles)).
- During the TLS handshake, peers MUST use the ALPN ([RFC 7301](https://www.rfc-editor.org/rfc/rfc7301)) protocol identifier `tempest/1`. A peer MUST close a connection whose negotiated ALPN identifier it does not support.
- A peer MUST send all of its QUIC traffic (peer connections and its relay connection) from a single UDP socket per address family (IPv4, IPv6). Addresses observed by relays, trackers and other peers then describe the same endpoint, which hole punching depends on.
- A peer MUST NOT send UDP payloads larger than 1,200 bytes on a path until QUIC's path MTU discovery has confirmed that the path carries more. Client Initial packets MUST be exactly 1,200 bytes. This guarantees that packets fit through relays ([relays.md](relays.md)).

**Versioning.** The ALPN identifier carries the protocol's major version. An incompatible future version uses a new identifier (for example `tempest/2`). A client MAY offer several identifiers during a transition, and TLS selects one both peers support. Compatible additions don't change the identifier. They use capabilities announced in [Hello](messages.md#hello).

**Rationale.**

- QUIC provides reliable, ordered streams, congestion control and encryption in one standard transport.
- Many independent streams share one connection, so a lost packet only delays the stream it belongs to.
- QUIC runs over UDP, which makes it practical to connect peers behind home routers by hole punching.

## Peer identity

Each peer has an Ed25519 key pair ([RFC 8032](https://www.rfc-editor.org/rfc/rfc8032)). A peer's **peer ID** is its 32-byte Ed25519 public key.

- Peers MUST authenticate using TLS raw public keys ([RFC 7250](https://www.rfc-editor.org/rfc/rfc7250)) with their Ed25519 key. No X.509 certificates are used.
- Both sides MUST authenticate. A peer that does not present a valid raw public key MUST be rejected.
- The TLS details are:
  - both certificate-type extensions (`client_certificate_type`, `server_certificate_type`) offer `RawPublicKey`;
  - the accepting peer MUST send a CertificateRequest;
  - the signature scheme is `ed25519` (`0x0807`);
  - the key is carried as a SubjectPublicKeyInfo ([RFC 8410](https://www.rfc-editor.org/rfc/rfc8410)), from which the 32-byte peer ID is extracted. Its DER form is the 12 bytes `302a300506032b6570032100` followed by the key, and a peer MUST reject any other encoding;
  - the server name (SNI) MAY be omitted and MUST NOT be required.
- If the connecting peer expected a particular peer ID (for example, one given in a link), it MUST close the connection if the presented key does not match.

**Rationale.** In a peer-to-peer network there is no certificate authority to vouch for anyone, so the key itself is the identity. Raw public keys let TLS prove "I hold the private key for this public key" without the machinery of certificates.

**Implementer notes.**

- Generate the key once per installation and store it securely. Let the user rotate it. A stable peer ID lets direct-send links keep working and lets a client remember peers it has dealt with.
- A long-lived peer ID lets observers link a peer's activity across bundles. Tempest does not aim to provide anonymity.

## Connections and bundles

Each connection is about exactly one bundle. The bundle is named in the first message on the connection ([Hello](messages.md#hello)). Two peers that are in several swarms together open a separate connection for each bundle.

**Rationale.** Every message on a connection is about the same bundle, so messages never need to say which bundle they refer to. Peers rarely share more than one swarm, so the extra handshakes cost little.

### Duplicate connections

Two peers can end up with more than one connection for the same bundle, for example when both dial each other at the same moment during hole punching. The peer with the **lower peer ID** (comparing peer IDs as unsigned byte strings) resolves this. Once the opening sequence has completed on more than one of the connections, it keeps one and closes the others with Goodbye (`duplicate`). It chooses as follows:

1. A direct connection is kept in preference to one through a relay.
2. Among connections of the same kind, the one whose opening sequence completed first is kept.

The peer with the higher peer ID MUST NOT close a connection because it is a duplicate. It waits for the other peer's choice.

**Rationale.** If both peers chose independently, timing differences could lead each to close the connection the other kept. Letting exactly one peer decide avoids that.

## Reaching peers behind routers

Most peers sit behind routers that perform network address translation (NAT) and drop incoming connections they weren't expecting. Tempest uses three techniques, in combination:

1. **Port mapping.** Clients SHOULD ask their router to forward a port to them (UPnP-IGD, NAT-PMP or PCP).
2. **Relays.** A **relay** is a public server that lends peers a public **relay address** and forwards encrypted QUIC traffic sent to it. A peer with a configured relay keeps an outgoing connection to it (its **home relay**), and advertises its relay address alongside its direct addresses. A peer that wants to connect MAY dial the other peer's relay address immediately.
3. **Hole punching.** While connected through a relay, the two peers exchange their candidate addresses in [Candidates](messages.md#candidates) messages and dial each other directly at the same moment. If a direct connection opens, it replaces the relayed one.

Peers communicate over UDP only. Networks that block UDP entirely are not supported, and clients SHOULD tell the user when this appears to be the case.

Traffic through a relay remains end-to-end encrypted between the two peers. A relay learns which addresses are communicating, but not what they send.

**Rationale.** Hole punching always needs a helper that both peers can already reach. A relay is that helper, and also the fallback when hole punching fails, which happens for roughly 10–30% of peer pairs (mostly behind NATs that choose a new public port for every destination). Starting through the relay means data flows immediately, and switching to a direct path afterwards keeps relay load low.

**Implementer notes.**

- Keep a configurable list of known relays and pick the home relay with the lowest latency.
- Try direct addresses and the relay in parallel. Use whichever connects first, then upgrade.
- Tell the user when a connection is relayed, since it will usually be slower.
- A peer MAY refuse new connections before the handshake when too many are still opening, in total or from one source. The dialer sees a failed connection, not a Goodbye.

The relay protocol and the hole-punching procedure are defined in [relays.md](relays.md).

## Congestion control

Congestion control is a local choice (policy) and is not specified. Clients MAY offer different behaviors, such as a "fast" mode and a "background" mode that yields bandwidth to the user's other traffic.
