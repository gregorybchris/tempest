# Messages

> **Status: Draft (version 1).**

This section defines the messages two peers exchange over a connection (see [connections.md](connections.md)) and the rules for exchanging them.

## Encoding

All messages are encoded in binary, using the following primitive types. All multi-byte integers are big-endian (most significant byte first).

| Type | Encoding |
| --- | --- |
| `u8`, `u16`, `u32`, `u64` | Unsigned integer of 1, 2, 4 or 8 bytes |
| `i64` | Signed integer (two's complement) of 8 bytes |
| `bool` | A `u8` that MUST be `0` (no) or `1` (yes) |
| `hash` | Exactly 32 bytes |
| `bytes` | A `u32` length *n*, followed by *n* bytes |
| `string` | Encoded like `bytes`. The contents MUST be valid UTF-8 |
| `list<T>` | A `u32` count *n*, followed by *n* values of type `T` |

A receiver MUST treat a value that breaks these rules (such as a `bool` of `2` or invalid UTF-8) as a protocol violation.

**Rationale.** Fixed-size big-endian integers read naturally in a hex dump and are simple to parse. Because every value has exactly one valid encoding, any data structure has exactly one byte form. This matters for the manifest, whose bytes determine the bundle ID.

### Framing

On streams, every message is sent as a **frame**:

```text
Frame {
  length: u32   // number of bytes that follow (type + body)
  type:   u8    // message type, from the table below
  body:   …     // fields of that message type, in order
}
```

A receiver MUST treat a frame as a protocol violation if its type is unknown, or if its body is shorter or longer than its fields require. The one exception is [Hello](#hello): a receiver MUST ignore any bytes after Hello's known fields, so future versions can add fields to Hello compatibly.

**Size limits.** A receiver MUST treat a frame whose `length` exceeds these limits as a protocol violation, and SHOULD check the limit before reading the body:

| Messages | Maximum `length` |
| --- | --- |
| Control messages | 16 MiB |
| Data | 1 + 4 + 32 × 32 (the largest proof) + 4 + `count` × 16 KiB |
| Manifest | 1 + 4 + 64 MiB |
| Reject | 1 + 2 + 4 + 1 KiB |
| Request, ManifestRequest | Exactly their fixed size |

**Rationale.** A length prefix lets a peer demand memory before sending anything, so every frame needs a bound. 16 MiB covers even a very fragmented Have list (about two million ranges).

### Common structures

```text
Range {
  start: u32    // first chunk index, inclusive
  end:   u32    // last chunk index, exclusive
}
```

A list of ranges MUST be sorted by `start`. Each range MUST be non-empty (`start < end`). Ranges MUST NOT overlap or touch: each range's `start` MUST be greater than the previous range's `end`. Every range MUST lie within the bundle's chunks (`end` ≤ the bundle's chunk count). These rules give every set of chunks exactly one encoding. A list that breaks them is a protocol violation. A peer that doesn't yet have the manifest checks the bounds once the manifest arrives.

```text
Address {
  family: u8    // 4 = IPv4, 6 = IPv6
  ip:     …     // 4 bytes if family is 4, 16 bytes if family is 6
  port:   u16
}
```

An IPv4 address MUST be encoded with family 4, even when a dual-stack socket reports it in IPv4-mapped IPv6 form (`::ffff:a.b.c.d`). Any other `family` value is a protocol violation.

## Chunk numbering

Chunks are numbered with a single index across the whole bundle. Files are taken in manifest order (only `file` entries count). Each file's chunks occupy the next consecutive indexes. A file with `N` chunks (see [content.md](content.md#chunks)) occupies `N` indexes, so an empty file occupies one.

A file's **first chunk** is the sum of the chunk counts of all files before it. A chunk's **position** is its index minus its file's first chunk.

**Rationale.** One number identifies any chunk, and one list of ranges describes everything a peer has. Files never share a chunk, so mapping an index to its file is a lookup.

## Streams

Each connection has one **control stream** and any number of **request streams**. All streams are bidirectional. Peers MUST NOT open unidirectional streams.

- The **control stream** is QUIC stream 0, the first bidirectional stream opened by the dialing peer. It stays open for the whole connection, except that a peer that is leaving MAY finish its side first (see [Closing a connection](#closing-a-connection)). It carries the messages in [Control messages](#control-messages).
- A **request stream** is any other bidirectional stream, opened by either peer acting as requester. The requester sends exactly one Request or ManifestRequest and then finishes its side of the stream. The provider sends exactly one response (Data, Manifest or Reject) and then finishes its side.
- A requester MAY cancel a request by resetting its stream. A provider that sees a request stream reset SHOULD stop preparing the response, and owes no answer.

**Rationale.** Giving each request its own stream means a lost packet only delays that request, and cancelling uses QUIC's built-in stream reset, so no "cancel" message is needed.

**Implementer notes.** Set QUIC's limit on bidirectional streams the other peer may open to at least `max_open_requests` + 1 (the extra one is the control stream). QUIC's own flow control then keeps the peer within the limit.

## Message types

| Type | Message | Stream | Sent by |
| --- | --- | --- | --- |
| `0x01` | [Hello](#hello) | control | both |
| `0x02` | [Have](#have) | control | both |
| `0x03` | [HaveUpdate](#haveupdate) | control | both |
| `0x04` | [Want](#want) | control | both |
| `0x05` | [Serving](#serving) | control | both |
| `0x06` | [Peers](#peers) | control | both |
| `0x07` | (retired: Goodbye is part of [closing](#closing-a-connection)) | — | — |
| `0x08` | [Candidates](#candidates) | control | both |
| `0x10` | [Request](#request) | request | requester |
| `0x11` | [Data](#data) | request | provider |
| `0x12` | [Reject](#reject) | request | provider |
| `0x13` | [ManifestRequest](#manifestrequest) | request | requester |
| `0x14` | [Manifest](#manifest) | request | provider |

## Control messages

### Opening sequence

1. The **dialing** peer opens the control stream and sends Hello, then Have.
2. The **accepting** peer reads them. If it participates in the bundle named in Hello, it replies with its own Hello, then Have. If it doesn't, it [closes the connection](#closing-a-connection) with `unknown_bundle`.
3. The opening is complete for a peer once it has both sent and received Hello and Have.

On the control stream, the first two messages each peer sends MUST be Hello and Have, in that order. A peer that receives anything else first on the control stream MUST close the connection with `protocol_violation`.

Because QUIC doesn't order different streams relative to each other, a request stream can arrive before the opening is complete. A peer MUST hold such a stream and handle it once the opening is complete. It MUST NOT treat the stream as a violation.

**Rationale.** The accepting peer can't describe its holdings until it knows which bundle the connection is for, so the dialing peer, who chose the bundle, speaks first.

### Hello

```text
Hello {
  bundle_id:          hash           // the bundle this connection is about
  capabilities:       list<string>   // optional features this peer supports
  client:             string         // client name and version; MAY be empty
  max_request_chunks: u32            // largest Request count this peer accepts
  max_open_requests:  u32            // most Requests this peer accepts open at once
  addresses:          list<Address>  // the sender's own direct addresses; MAY be empty
  relay:              list<Address>  // the sender's relay address: zero or one entries
}
```

- The accepting peer's `bundle_id` MUST equal the dialing peer's.
- A peer MUST NOT send an optional message unless the other peer listed the corresponding capability. This version of the protocol defines no optional capabilities.
- `max_request_chunks` MUST be a power of two from 1 to 1,024. `max_open_requests` MUST be at least 1.
- `addresses` lists addresses at which the sender accepts connections. It SHOULD include only addresses reachable from outside the sender's local network, unless the connection itself is on a local network. `relay` holds the relay address lent to the sender by its home relay ([relays.md](relays.md)), if it has one.
- A receiver MUST ignore bytes after the known fields (see [Framing](#framing)).

**Rationale.** A peer's own addresses let the other side reconnect later and pass the peer on to others through [Peers](#peers).

**Implementer notes.** Choose limits that keep the link busy. To use bandwidth *B* (bytes per second) over a round-trip time *R* (seconds), a requester needs about *B × R* bytes requested at any moment. At 1 Gbps and 100 ms that is about 12 MB, or about 760 chunks.

### Have

```text
Have {
  ranges: list<Range>   // every chunk this peer has and is willing to provide
}
```

Have describes the chunks the sender has and is willing to provide. It is sent once, as part of the opening sequence. It MAY be empty. A peer that lacks the manifest has no chunks, so it sends an empty Have.

A peer that has the manifest has every empty file's chunk (which is zero bytes long), and MUST include those chunks in Have.

### HaveUpdate

```text
HaveUpdate {
  added:   list<Range>  // chunks the sender now has
  removed: list<Range>  // chunks the sender no longer has
}
```

HaveUpdate changes the set of chunks described by the sender's Have and earlier HaveUpdates. `added` MUST NOT include chunks already in the set, and `removed` MUST include only chunks in the set. A peer that obtains the manifest after sending Have adds its empty files' chunks in a HaveUpdate.

**Implementer notes.** Batching several newly verified chunks into one HaveUpdate saves messages, at the cost of telling peers slightly later.

### Want

```text
Want {
  wanted: bool   // yes: you have chunks I'd like to request
}
```

At the start of every connection each peer is treated as not wanting (`wanted = no`). A peer SHOULD send Want (yes) when the other peer has chunks it lacks, and Want (no) when that stops being true.

**Rationale.** Providers decide whom to upload to. Knowing who actually wants something lets them make that decision before any requests arrive.

### Serving

```text
Serving {
  serving: bool   // yes: if you send Requests now, I intend to answer them with Data
}
```

At the start of every connection each peer is treated as not serving (`serving = no`). Which peers a provider serves, and when, is policy: the provider's upload strategy.

A requester SHOULD NOT send Requests to a provider that is not serving it. A provider that receives a Request while not serving the requester MAY answer it with Reject (`busy`). When a provider stops serving a peer, it MAY likewise reject that peer's Requests it hasn't started answering.

**Rationale.** Upload strategies such as BitTorrent's (serve a few peers at a time, chosen by reciprocity, plus a rotating newcomer) work best when requesters know in advance whether they'll be served. Strategies that decide request by request can simply serve everyone who wants data.

**Implementer notes.**

- The recommended default strategy serves about 4 peers chosen by how much they have recently uploaded to us, plus 1 rotating peer chosen at random so newcomers can get started.
- Want and Serving start at "no" on every new connection, including one that replaces a relayed connection. Re-send them as needed.

### Peers

```text
Peers {
  peers: list<PeerRecord>   // other peers in this swarm
}

PeerRecord {
  peer_id:   hash            // the peer's Ed25519 public key
  addresses: list<Address>   // direct addresses; MAY be empty
  relay:     list<Address>   // the peer's relay address: zero or one entries
}
```

- A peer MUST NOT include more than 50 PeerRecords in one Peers message, and SHOULD NOT send Peers more often than once per minute on a connection. The last Peers a peer sends before leaving the swarm is exempt from the rate limit.
- A leaving peer SHOULD send that last Peers on each connection, listing peers it is connected to first, then others it knows. A peer that knew the swarm only through the leaver would otherwise have no one left to find.
- A PeerRecord's `addresses` MAY include the addresses that peer advertised in its Hello, and the source address of a direct connection with it. They MUST NOT include a relay address: relay addresses go only in `relay`.
- A PeerRecord's `relay` MUST contain at most one address.

**Rationale.** Peer exchange is the cheapest way to grow from one known peer to many. The limits bound how much a peer can flood its neighbors with addresses.

### Candidates

```text
Candidates {
  addresses: list<Address>   // addresses at which the sender might be reached directly
}
```

Candidates helps two peers that are connected through a relay establish a direct connection. A peer MUST NOT include more than 16 addresses.

- A peer that receives Candidates, and hasn't yet sent its own on this connection, MUST reply with Candidates immediately.
- After sending and receiving Candidates, each peer SHOULD immediately try to connect directly to the other's addresses.

The two peers' simultaneous attempts punch through their routers. The full procedure is in [relays.md](relays.md#upgrading-to-a-direct-connection).

## Closing a connection

A peer closes a connection with QUIC's application CONNECTION_CLOSE frame ([RFC 9000 §19.19](https://www.rfc-editor.org/rfc/rfc9000#section-19.19)). Its error code is a **reason code** from the table below, and its reason phrase is a human-readable message (UTF-8, at most 1 KiB, MAY be empty). Tempest calls this a **Goodbye**.

| Code | Name | Meaning |
| --- | --- | --- |
| 0 | `done` | Normal close, e.g. the sender no longer needs or serves this bundle |
| 1 | `shutting_down` | The sender is shutting down |
| 2 | `unknown_bundle` | The sender isn't participating in the bundle named in Hello |
| 3 | `duplicate` | Another connection between these peers for this bundle is being kept |
| 4 | `protocol_violation` | The other peer broke a rule in this specification |
| 5 | `bad_data` | The other peer sent data that failed verification |
| 6 | `too_many_connections` | The sender has reached its connection limit |

- A receiver MUST treat an unknown reason code like `done`, and not as an error.
- A peer that sends messages just before leaving, such as the last Peers, finishes its side of the control stream after them, waits for the other peer to acknowledge it, and then sends the Goodbye. A receiver MUST NOT treat a finished control stream as a violation; it expects a Goodbye. A peer SHOULD close the connection with `protocol_violation` if none follows within a few seconds.
- Closing a connection ends all of its streams. Requests that were open on it are abandoned, and the requester MAY re-request them elsewhere. A peer that is leaving and waiting to send its Goodbye resets the request streams it won't answer; a requester MUST NOT treat such a reset as a violation if the Goodbye follows within a few seconds.
- "Close the connection with `protocol_violation`" anywhere in this specification means a Goodbye with that reason.

**Rationale.** QUIC discards stream data that hasn't been delivered when a connection closes. A goodbye sent as a stream message right before closing would usually be lost. The CONNECTION_CLOSE frame is delivered reliably as part of closing. A message that must arrive before it is sent on the control stream, which is then finished and acknowledged before the close.

## Request messages

### Request

```text
Request {
  start: u32   // first chunk index
  count: u32   // number of chunks; a power of two
}
```

A Request asks for the aligned range of chunks `[start, start + count)`. A valid Request satisfies:

- `count` is a power of two, and no larger than the provider's `max_request_chunks`.
- `start` lies within a file, and `start`'s position within that file is a multiple of `count`.
- The range does not extend past the end of the file's chunk tree (its width `W`; see [content.md](content.md#width-and-padding)). The range MAY extend past the file's last chunk into the tree's padding.

In other words, a valid Request names exactly one node of one file's chunk tree. A provider MUST answer a Request that isn't valid with Reject (`invalid`).

A provider MUST answer every Request whose stream isn't reset with exactly one Data or Reject. A Request is **open** from the moment the provider receives it until the provider finishes its response or sees the stream reset. A provider MUST answer with Reject (`busy`) a Request that would exceed `max_open_requests` open Requests (including ManifestRequests) from that requester.

**Implementer notes.** The requester's count of open requests can lag behind the provider's, because a reset takes time to arrive. Leave some headroom below `max_open_requests`.

### Data

```text
Data {
  proof:  list<hash>   // sibling hashes from the range's node up to the root
  chunks: bytes        // the requested chunks that exist, concatenated
}
```

`chunks` holds every requested chunk that exists in the file, in order. Chunks past the file's last chunk are omitted. Each chunk is 16 KiB except possibly the file's last, and an empty file's single chunk is zero bytes.

`proof` lists the sibling of each node on the path from the requested range's node up to (but not including) the root, starting with the lowest. It has exactly log₂(`W`) − log₂(`count`) hashes, so it is empty when the range's node is the root.

**Verifying Data.** Using the file's size and root from the manifest, a requester:

1. Checks that `proof` has exactly log₂(`W`) − log₂(`count`) hashes, and that `chunks` has exactly the number of bytes the requested existing chunks contain (from the file's size). If not, the Data fails verification.
2. Splits `chunks` into chunks and computes each leaf hash. For an empty file, the single leaf is `leaf` of the empty chunk, i.e. `SHA-256(0x00)`. Positions past the file's last chunk, up to `count` leaves, use padding leaves (32 zero bytes).
3. Combines the leaves pairwise, level by level, into the range's node (see [content.md](content.md#the-chunk-tree)).
4. Combines the running hash with each proof hash in order. Let *i* be `start`'s position divided by `count`. At each step, if *i* is even, the running hash is a left child: compute `node(running, proof hash)`. If *i* is odd, it's a right child: compute `node(proof hash, running)`. Then halve *i* (rounding down).
5. The result must equal the file's root.

If any check fails, the Data fails verification, and the requester MUST discard the chunks.

**Rationale.** Because a request is always one aligned node of the tree, its proof is just one path to the root. Every Data message can be verified on its own, the moment it arrives.

**Implementer notes.** QUIC's encryption guarantees data isn't corrupted in transit, so a failed verification means the provider sent wrong data. A reasonable policy is to close the connection with `bad_data` and refuse that peer for the bundle.

#### Test vectors

These use the 40,000-byte test file from [content.md](content.md#test-vectors), where byte *i* = *i* mod 256 (3 chunks, width 4, root `cf6700fc…62c2f8`). Starts are positions within the file.

| Range | Proof (lowest first) |
| --- | --- |
| start 2, count 1 | `0000000000000000000000000000000000000000000000000000000000000000` (padding leaf 3), `1b53726ec87fc16451460d18b88ed388f714a8f983e0ea37fb43620fc816e115` (node over chunks 0–1) |
| start 0, count 2 | `59aef01004079909de1326890705ff0009732e28a43a5c8a8f3d7e8ed7c1af7c` (node over chunk 2 and padding) |
| start 2, count 2 | `1b53726ec87fc16451460d18b88ed388f714a8f983e0ea37fb43620fc816e115` (node over chunks 0–1) |
| start 0, count 4 | (empty: the range is the whole tree) |

### Reject

```text
Reject {
  reason:  u16      // a reason code, from the table below
  message: string   // human-readable detail; MAY be empty; at most 1 KiB
}
```

| Code | Name | Meaning |
| --- | --- | --- |
| 0 | `busy` | The provider isn't serving the requester right now, or the requester has too many open requests |
| 1 | `dont_have` | The provider doesn't have (all of) the requested chunks, or the manifest |
| 2 | `invalid` | The request breaks a rule in this specification |

A receiver MUST treat an unknown reason code like `busy`.

### ManifestRequest

```text
ManifestRequest {}   // no fields
```

Asks the provider for the bundle's manifest.

- The provider answers with Manifest, or with Reject: `dont_have` if it doesn't have the manifest, or `busy`.
- A provider SHOULD answer ManifestRequest whether or not it is serving the requester, because a newcomer can do nothing without the manifest.
- ManifestRequest counts toward `max_open_requests`.
- A requester SHOULD ask for the manifest once per connection. A provider MAY refuse further ManifestRequests on a connection with `busy`, since a manifest can be large and the requester may not be one it serves.

An empty Have doesn't tell a requester whether the provider has the manifest. A requester simply asks, and accepts `dont_have`.

### Manifest

```text
Manifest {
  manifest: bytes   // the encoded manifest
}
```

`manifest` MUST NOT exceed 64 MiB. The requester MUST verify that `SHA-256(0x02 ‖ manifest) = bundle ID` before using it, and MUST discard it otherwise.

**Rationale.** Most manifests are small, so delivering one whole is simplest. The size limit bounds how much a cheating peer can waste before verification fails.

## Test vector: a framed Hello

A Hello from a dialing peer for the bundle in [content.md's manifest test vector](content.md#manifest-test-vector), with no capabilities, client `tempest/0.1.0`, `max_request_chunks` 64, `max_open_requests` 32, one direct address `203.0.113.5:4433` and no relay address. The frame is 81 bytes:

| Bytes (hex) | Meaning |
| --- | --- |
| `0000004d` | length = 77 |
| `01` | type = 0x01 (Hello) |
| `207855f013f5a4066c9987ee3fa4b0ae63c8eeee02035634587286457cf5b85a` | bundle_id |
| `00000000` | capabilities: count = 0 |
| `0000000d74656d706573742f302e312e30` | client = "tempest/0.1.0" |
| `00000040` | max_request_chunks = 64 |
| `00000020` | max_open_requests = 32 |
| `00000001` | addresses: count = 1 |
| `04` | ↳ family = 4 |
| `cb007105` | ↳ ip = 203.0.113.5 |
| `1151` | ↳ port = 4433 |
| `00000000` | relay: count = 0 |

Full frame:

```text
0000004d01207855f013f5a4066c9987ee3fa4b0ae63c8eeee02035634587286457cf5b85a000000000000000d74656d706573742f302e312e3000000040000000200000000104cb007105115100000000
```
