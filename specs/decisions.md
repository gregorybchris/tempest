# Decisions

A log of design decisions for Tempest. Each entry records the question, the options considered, what was decided, and why, so decisions don't get re-litigated and future contributors can see the reasoning.

Background research lives in `planning/research/` (not checked in).

---

## D-001: Purpose and primary use cases

**Date:** 2026-10-06 · **Status:** Decided

**Question:** Which use cases should the design be tuned for from the start?

**Options considered:**

- Direct sends: 1:1 or small-group transfers via a link
- Public swarms: classic sharing among strangers
- Private groups: gated swarms within a team or friend group
- Publisher distribution: big releases backed by the publisher's own server

**Decision:** Support direct sends and public swarms from the start. Tempest is primarily a teaching and research tool: clarity of concepts in the code comes first, speed second. The client is configurable so it can operate in different regimes, and so the simulator can measure the impact of features and parameter settings.

**Rationale:** "This project is mostly for teaching purposes, so while speed is important, most important is that the concepts are easy to understand from the code." Configurability doubles as a research instrument: the client is "a bit of a product, but also a bit of a didactic research tool."

**Implications:**

- Goal order in `index.md` becomes clarity → configurability → speed → usability.
- Optimizations that obscure the code need to justify themselves with simulation data.
- Behaviors like piece selection and upload allocation should be swappable, named strategies with parameters, not hard-coded.

---

## D-002: Anonymity

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** Anonymity is an explicit non-goal. Public sharing exposes a peer's IP address, and Tempest's docs and UI say so plainly.

**Rationale:** Anonymity layers cost a lot of speed and complexity (Tribler, Freenet) and would obscure the core concepts.

---

## D-003: Encryption

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** Every peer connection is encrypted and authenticated. Peers are identified by a public key. There is no unencrypted mode.

**Rationale:** Modern authenticated encryption costs almost nothing in speed, prevents tampering, and gives the spec one mode instead of two. BitTorrent's optional encryption (MSE/PE) is weak and is still detectable by traffic analysis.

---

## D-004: Protocol vs. policy, and protocol variants

**Date:** 2026-10-06 · **Status:** Decided

**Context:** The *protocol* is the contract two peers must agree on to interoperate (message formats, content identifiers, verification). A *policy* is a local choice each peer makes on its own (which piece to request next, whom to upload to, how many requests to keep in flight). Policies can differ between peers without breaking anything; protocol choices can't.

**Question:** How should experiments on protocol-level choices (e.g. Merkle vs flat hashes) be supported?

**Options considered:**

- Simulator-only variants: one spec'd protocol; alternative implementations exist in code only for simulation
- Negotiated optional features: peers on the real network can negotiate protocol alternatives
- One protocol, no variants: experiment only on policies

**Decision:** Simulator-only variants. The spec defines exactly one protocol. The codebase may hold alternative implementations of protocol components behind a common interface, used only in simulation to justify the spec's choices. Real clients speak only the spec'd protocol.

**Rationale:** Keeps the spec simple for implementers while preserving Tempest's value as a research tool.

**Implications:**

- The spec should clearly separate normative protocol (MUST) from policy (each client's choice, perhaps with a recommended default).
- Policies are named, swappable strategies with parameters.

---

## D-005: Configuration style

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** Named profiles plus per-setting overrides. A profile (e.g. "balanced", "polite background", "classic BitTorrent", "cooperative") sets every strategy and parameter; any individual setting can then be overridden.

**Rationale:** Beginners pick a profile; experiments override exactly the settings under test. Profiles also give simulations meaningful baselines to compare.

---

## D-006: Explain mode (decision events)

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** First-class. Strategies emit structured decision events with human-readable reasons (e.g. "requested piece 412 because only 2 peers have it"). The UIs can show a live decision log, and the simulator records the same events for analysis.

**Rationale:** Directly serves the teaching goal: the client shows *why* it behaves as it does, not just *what* it does. Using one event stream for UIs and simulation keeps them consistent.

---

## D-007: Verification scheme

**Date:** 2026-10-06 · **Status:** Decided

**Question:** How does a downloader check that data from an untrusted peer is correct?

**Options considered:**

- Flat list: the file description lists one hash per chunk (BitTorrent v1)
- Merkle tree: chunk hashes are combined pairwise up to a single root; each small block is verified on arrival with a short proof (BitTorrent v2, iroh, THEX)

**Decision:** Merkle tree. A file's root hash is its identity, and every block is verified the moment it arrives. A flat-list implementation may exist as a simulator-only variant for comparison (D-004).

**Rationale:**

- Exact blame for bad peers.
- No wasted re-downloads of large chunks.
- Descriptions stay tiny regardless of file size.
- Identical files are recognizable across shares.
- It's a foundational idea worth teaching.
- Proof overhead is about 0.4% for range requests.

---

## D-008: Hash function

**Date:** 2026-10-06 · **Status:** Decided

**Options considered:** SHA-256 with a Tempest-specified tree; BLAKE3 (tree built into the hash, via `bao-tree`)

**Decision:** SHA-256, with the Merkle tree layout specified by Tempest and implemented in Tempest's own code.

**Rationale:**

- The tree and proof logic stays visible and readable in the codebase rather than hidden inside a library, which serves the clarity goal.
- SHA-256 is available in every language, which helps other implementers.
- It is the fastest option measured on Apple Silicon (2.3 GB/s per core on an M2), and well above network speeds elsewhere.

---

## D-009: Multi-file shares

**Date:** 2026-10-06 · **Status:** Decided

**Options considered:** per-file trees plus a manifest; concatenating all files into one stream (BitTorrent v1)

**Decision:** Each file gets its own Merkle tree and root. A share is described by a manifest listing each file's path, size and root. The share's ID is the hash of the manifest.

**Rationale:**

- Chunks never straddle file boundaries.
- Selecting individual files is clean.
- The same file is recognizable across different shares.

---

## D-010: Block size

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** The spec fixes the block (leaf) size at 16 KiB (16,384 bytes). In the client code, block size is a parameter so simulations can test other sizes, as a simulator-only variant (D-004).

**Rationale:**

- 16 KiB is the industry standard (BitTorrent's request size, BitTorrent v2's leaf size, iroh's chunk groups).
- Verification and blame are fine-grained.
- Existing tuning knowledge carries over.

---

## D-011: Tree shape

**Date:** 2026-10-06 · **Status:** Decided

**Options considered:**

- Pad to a perfect binary tree (BitTorrent v2)
- Uneven tree with promoted nodes (RFC 6962)

**Decision:** Pad to a perfect binary tree. Missing leaves use a fixed padding hash.

**Rationale:** Block *i* is always leaf *i*, and every position follows from simple arithmetic. The tree is easier to teach and implement.

---

## D-012: Domain separation

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** Leaf and inner-node hashes use different one-byte prefixes:

- Leaf: `SHA-256(0x00 ‖ block)`
- Inner node: `SHA-256(0x01 ‖ left ‖ right)`

**Rationale:** Without prefixes, an inner node's hash equals the leaf hash of a 64-byte "file" made of its two child hashes. One fingerprint then matches two different things. Prefixes make the tree safe by construction, without relying on every verifier knowing the file size. This is the approach Certificate Transparency (RFC 6962) takes.

**Trade-off accepted:** A single block can't be checked with plain `sha256sum`.

---

## D-013: Manifest contents

**Date:** 2026-10-06 · **Status:** Decided

**Context:** The share ID is the hash of the manifest, so every manifest field becomes part of the share's identity. Two shares of the same files that differ in any field form separate swarms.

**Decision:** The manifest includes the following.

- Per entry: path, size, root hash (files), executable flag, modification time
- Entry types: regular files, symlinks (path → target) and empty directories
- Share-level: name, comment/description, creation date and creator tool

**Rationale:** These fields noticeably improve the client experience and the UI. Swarm splitting from differing metadata is an accepted cost: "I don't care as much about the possibility of multiple people uploading the same file and splitting the swarm."

**Implications:**

- Per-file roots don't depend on metadata, so identical files stay recognizable across different shares (useful for a possible future "find this file in other swarms" feature).
- Symlinks need safety rules. A client must refuse targets that are absolute or that escape the share's folder.

---

## D-014: Protocol spec conventions and location

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- The protocol spec lives in a top-level `protocol/` folder, one file per topic.
- `specs/` holds project planning: objectives, decisions, plans. It may be deleted once the project is complete. Protocol docs must therefore be self-contained and must not depend on `specs/`.
- Binding rules use RFC 2119 / RFC 8174 keywords (MUST, SHOULD, MAY) in capitals only. Lowercase prose is explanation.
- Each section includes, by default: rationale, non-binding implementer notes, and worked examples / test vectors. (The owner didn't answer this sub-question; these are defaults to revisit.)

---

## D-015: Tree edge cases

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- An empty file is one empty block, so its root is `SHA-256(0x00)`.
- Padding leaves are 32 zero bytes, not hashed (as in BitTorrent v2).

---

## D-016: Core vocabulary (round 1)

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

| Concept | Tempest name | BitTorrent equivalent | Alternatives considered |
| --- | --- | --- | --- |
| One or more files offered together | **bundle** | torrent | share (also a verb), collection (implies many), drop, parcel (a bit cutesy), capsule (sounds like the container), bolt (confusing), payload (sounds small), shipment |
| Document describing a bundle's contents | **manifest** | .torrent file / info dictionary / metainfo | index, listing |
| 16 KiB unit of data that is requested and verified | **chunk** | block (inside a piece) | block, piece |
| Peers exchanging the same bundle | **swarm** | swarm | storm, crowd |

**Rationale:**

- Use plain words that newcomers understand.
- A name should not also be a common verb.
- A name should fit a single file as well as many.
- Tempest has a single data unit (the chunk), so BitTorrent's confusing piece/block split goes away.

---

## D-017: Core vocabulary (round 2)

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- **root**: the top hash of a file's chunk tree, which identifies the file's contents. Chosen over "file ID" and "fingerprint" because it teaches the tree.
- **proof**, **peer**, **entry** and **chunk tree** are kept as named.
- **provider** and **requester** are *roles in a single exchange*: in any one request, the requester asks and the provider sends. The same two peers swap roles constantly.
- **complete** and **incomplete** are the *states* of a peer that does or doesn't have the whole bundle (BitTorrent: seeder and leecher).

**Rationale:** "Seeder" and "leecher" aren't obvious to someone new to torrenting, and "leecher" is inaccurate, since incomplete peers upload constantly. Treating provider and requester as roles rather than peer types keeps the words precise and teaches the core idea of swarming: every peer is both.

---

## D-018: Transport

**Date:** 2026-10-06 · **Status:** Decided

**Options considered:**

- QUIC via `quinn`
- iroh: QUIC plus dial-by-key, hole punching, relays and discovery
- TCP + Noise
- Custom UDP (uTP-style)

**Decision:** QUIC (RFC 9000), implemented with `quinn` in the Rust client.

**Rationale:**

- A standard transport with built-in, mandatory TLS 1.3, which satisfies D-003.
- Independent streams avoid head-of-line blocking.
- Connections survive network changes.
- Pluggable congestion control in `quinn` supports experiments with "polite" vs "fast" modes.
- QUIC runs over UDP, which makes hole punching straightforward.
- The spec stays short, and Tempest still designs its own connectivity, rather than adopting iroh's whole protocol.

**Trade-offs accepted:**

- Implementers in other languages need a QUIC library that supports Tempest's peer-identity scheme.
- QUIC needs tuning (UDP GSO/GRO) to approach TCP speeds above about 1 Gbps, and Windows and macOS lag behind Linux.

---

## D-019: NAT traversal

**Date:** 2026-10-06 · **Status:** Superseded by D-026

**Options considered:**

- Full stack with relay fallback
- Port mapping + hole punching, no relays
- Defer

**Decision:** Port mapping (UPnP-IGD / NAT-PMP / PCP) plus hole punching. No relays that forward data.

**Rationale:** No data-carrying servers to run. Combined techniques reach roughly 70–90% connectivity. The remaining pairs can't connect, and the client should explain that plainly.

**Open question:** Hole punching needs a third party both peers can reach to coordinate timing. In a swarm, that can be a mutually connected peer. For a 1:1 direct send with no other peers, some lightweight rendezvous is still needed. To be decided with peer discovery.

---

## D-020: Identity in the TLS handshake

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** Peers authenticate with Ed25519 keys carried as TLS 1.3 raw public keys (RFC 7250). No certificates are used. Both sides authenticate.

**Rationale:** It's the cleanest to specify and explain, since the identity is just the key. It's supported by `rustls` and used by iroh.

**Trade-off accepted:** Fewer TLS libraries outside Rust support RFC 7250.

---

## D-021: Identity key lifetime

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** By default, one long-lived identity key per installation. The user can rotate it at any time.

**Rationale:**

- A stable identity makes reusable direct-send links possible.
- It also lets a client remember peers.

**Trade-off accepted:** An observer can link a peer's activity across bundles.

---

## D-022: Connections and bundles

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** Each connection carries exactly one bundle. The bundle is named when the connection opens. Two peers that share several swarms open one connection per bundle.

**Rationale:**

- It's the simplest mental model: messages never need to say which bundle they're about.
- Sharing several swarms with the same peer is rare in practice.

---

## D-023: Peer discovery mechanisms

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** The protocol defines these discovery mechanisms:

- addresses carried in links
- LAN discovery
- trackers
- peer exchange (PEX)

Clients may use any subset, run them in parallel, and take whichever answers first. The simulator can compare them.

---

## D-024: DHT deferred

**Date:** 2026-10-06 · **Status:** Decided (deferred)

**Options considered:**

- Borrow BitTorrent's Mainline DHT via BEP 44 / pkarr
- Our own Tempest DHT
- Borrow now, own later
- No DHT

**Decision:** No DHT for now. Get the basics working end to end first, then add complexity later for more power.

**Implications:** Public swarms rely on trackers, peer exchange, LAN discovery and links. A DHT can be added later as another discovery mechanism without changing the rest of the protocol.

---

## D-025: Tempest tracker (scope addition)

**Date:** 2026-10-06 · **Status:** Decided (design to come)

**Decision:** The repo includes a Tempest tracker that anyone can run. It has its own server and web frontend in Rust, branded consistently with the client. It has two parts:

1. **Peer-list service:** clients report what they serve, and the tracker tells them about other peers. This part is protocol.
2. **Catalog:** a web application with search, categories, ranking, comments, filtering and sorting. It is continuously updated from what clients report. This part is an application, not protocol.

**Consideration to revisit:** Research report 12 found that the legal risk for P2P software has historically come from indexing, curating and promoting content, not from peer-list services. The tracker software's positioning, defaults and documentation should be decided deliberately when the catalog is designed. This is not legal advice.

---

## D-026: Relays (supersedes D-019)

**Date:** 2026-10-06 · **Status:** Decided (refined by D-104, D-105)

**Context:** Hole punching always needs a helper that both peers can already reach. It tells each peer its public address and coordinates timing. Without a fallback, roughly 10–30% of peer pairs, mostly behind "symmetric" NATs on mobile and corporate networks, can't connect at all.

**Options considered:**

- Relay as helper and fallback
- Helper only, never forwarding data
- No relay servers (D-019)

**Decision:** Use the iroh/Tailscale recipe:

1. Each peer keeps an outgoing connection to a home relay.
2. A connecting peer reaches the other peer through its relay immediately.
3. Meanwhile, the relay coordinates hole punching.
4. Traffic moves to the direct path when punching succeeds.
5. Traffic keeps flowing through the relay when it doesn't.

Port mapping (UPnP-IGD / NAT-PMP / PCP) is still attempted.

**Rationale:** Connections almost always succeed, which matters most for 1:1 direct sends between two home users. Data starts flowing immediately. Relays see only encrypted traffic: who talks to whom, but not what is sent.

**Trade-off accepted:** Relay operators pay bandwidth for the connections where punching fails.

---

## D-027: Finding relays

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- The client's config holds a list of known relays: defaults plus any the user adds.
- Each peer picks a home relay (e.g. lowest latency) and advertises it alongside its direct addresses, in links and in tracker peer lists.

**Resolved by D-029:** no default relays are shipped.

---

## D-028: Relay packaging

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- Relay functionality is a library.
- The repo ships a small standalone relay binary.
- The tracker server can also enable relaying as one of its roles.

**Rationale:** A standalone relay can run on cheap hosts without the catalog's database. Tracker operators can turn on relaying with no extra deployment.

---

## D-029: Default relays

**Date:** 2026-10-06 · **Status:** Decided

**Options considered:**

- The project runs a small default set
- Ship none and make self-hosting easy
- A community list of volunteer relays

**Decision:** The client ships with no default relays. Running a relay is a one-command operation, and trackers can advertise relays.

**Rationale:** No ongoing hosting cost or uptime responsibility, which fits a teaching and research project.

**Trade-off accepted:** A direct send between two home users who can't hole punch needs someone to have configured a relay.

---

## D-030: Tracker build timing

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- The tracker's peer-list service is built alongside the client (phase 5).
- The catalog website comes after the client's web UI and reuses its design (phase 10).

**Rationale:** There is no DHT (D-024), so public swarms depend on a tracker's peer-list service to work at all.

---

## D-031: Tracker positioning

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- The tracker is a neutral tool. The project publishes software and does not run a public catalog itself.
- Documentation states that operators are responsible for their site's content.
- Documentation and examples lead with legitimate uses: datasets, ML models, open-source software, game patches and public-domain media.

**Not chosen for now:** built-in moderation tools (reporting, takedowns, bans, audit log) and a private invite-only mode. Both can be revisited when the catalog is designed.

**Note:** This follows the research on how P2P software projects have historically avoided liability. It is not legal advice.

---

## D-032: Bundle ID prefix

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** `bundle ID = SHA-256(0x02 ‖ manifest bytes)`.

**Rationale:** Every kind of hash in Tempest gets its own one-byte label: `0x00` for a leaf, `0x01` for an inner node, `0x02` for a manifest. A bundle ID can then never equal a file root or a tree node. This extends D-012.

---

## D-033: Path rules

**Date:** 2026-10-06 · **Status:** Decided

**Options considered:**

- Strict and portable
- Permissive, with each client renaming files when it writes them

**Decision:** Strict and portable. Paths are UTF-8 in Unicode NFC form and use `/` as the separator. Names invalid on Windows, macOS or Linux are forbidden. No two paths may differ only by letter case.

**Rationale:** If a bundle is valid, it downloads identically on every OS, so clients need no renaming rules.

**Trade-off accepted:** Creators must rename incompatible files (e.g. `a:b`) before sharing.

---

## D-034: Entry order

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** Entries form a flat list, each with its full path, sorted by the bytes of its UTF-8 path.

**Rationale:** The same folder always produces the same manifest, and it's the simplest structure to read and implement. (The alternative was a nested directory tree, as in BitTorrent v2.)

---

## D-035: Timestamp precision

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** Timestamps (modification time, creation date) are signed integer milliseconds since 1970-01-01 00:00:00 UTC.

---

## D-036: Protocol versioning

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- The TLS ALPN label carries the major protocol version, e.g. `tempest/1`.
- An incompatible change gets a new label (`tempest/2`), and clients can support several labels during a transition.
- Compatible additions use the extension mechanism, to be designed with the messages.

---

## D-037: Duplicate connections

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** If two peers end up with more than one connection for the same bundle, both keep the connection opened by the peer whose peer ID is lower. Peer IDs are compared as unsigned byte strings. The other connections are closed.

**Rationale:** Hole punching often makes both peers dial at once. A deterministic rule means both sides choose the same connection without having to coordinate.

---

## D-038: Congestion control

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** Congestion control is policy, and the spec is silent on it. Clients offer modes (e.g. "fast" and "background") through profiles, and the simulator can compare them.

---

## D-039: Networks that block UDP

**Date:** 2026-10-06 · **Status:** Decided (revisit later)

**Decision:** UDP only for now. Tempest doesn't work on networks that block UDP, and the client explains why. Relays reachable over HTTPS/TCP can be added later.

---

## D-040: Link shape

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** Links are readable URIs in the style of magnet links, e.g. `tempest:?bundle=…&peer=…&relay=…&tracker=…`.

**Rationale:** A readable link is transparent, editable by hand and teachable. The alternative was an opaque encoded ticket (iroh style).

**Trade-off accepted:** Links are long.

---

## D-041: Hashes as text

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** Hashes and keys (bundle IDs, roots, peer IDs) are written as 64 lowercase hexadecimal characters.

**Rationale:** Identical to `sha256sum` output, so learners can verify values by hand. The alternatives were base32 (52 characters) and base58 (44 characters).

---

## D-042: LAN discovery

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- Tempest defines its own simple multicast announcement.
- It announces presence only: peer ID and port.
- Which bundles a peer has is asked over the encrypted connection.

**Rationale:**

- It's simple and fully specified by Tempest.
- Announcing bundle IDs would reveal what a peer shares to everyone on the network.
- The alternative, mDNS/DNS-SD, brings in a larger standard.

---

## D-043: Tracker API

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** Clients talk to trackers over HTTPS with JSON bodies.

**Rationale:**

- It can be inspected with `curl` and a browser, and uses standard web tooling.
- The same server process can serve the catalog.
- The alternative, QUIC with Tempest's own messages, would be more consistent but harder to inspect.

---

## D-044: Encoding

**Date:** 2026-10-06 · **Status:** Decided

**Options considered:**

- Hand-specified binary
- CBOR
- Protobuf
- JSON with raw chunk bytes

**Decision:** Peer messages and the manifest use a hand-specified binary encoding. The spec defines every field's bytes using a small set of primitive types.

**Rationale:**

- The encoding is canonical by construction. Each manifest has exactly one byte form, which the bundle ID requires (D-032).
- Every byte can be shown and annotated for learners.
- This is the approach of BitTorrent, QUIC and WireGuard.

**Trade-off accepted:** More spec text, and implementers write small parsers.

---

## D-045: Stream layout

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- Each connection has one long-lived control stream for state messages.
- Each request and its response travel on their own QUIC stream.
- A request is cancelled by resetting its stream.

**Rationale:**

- A lost packet stalls only the request it belongs to.
- Cancelling uses a built-in QUIC operation rather than a separate message.

---

## D-046: Extensions

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- In its opening message, each peer lists the optional capabilities it supports.
- A peer sends optional messages only if the other side announced the matching capability.
- Receiving an unknown message type is a protocol error.

**Rationale:** Explicit and easy to reason about. The alternative was Lightning's "it's OK to be odd" rule.

---

## D-047: Primitive types

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** All multi-byte integers are big-endian and fixed-size. The primitives are:

| Type | Encoding |
| --- | --- |
| `u8`, `u16`, `u32`, `u64` | Unsigned integer, 1, 2, 4 or 8 bytes |
| `i64` | Signed integer (two's complement), 8 bytes |
| `bool` | `u8` that MUST be 0 or 1 |
| `hash` | Exactly 32 bytes |
| `bytes` | `u32` length, then that many bytes |
| `string` | Same as `bytes`, but MUST be valid UTF-8 |
| `list<T>` | `u32` count, then that many items of type `T` |

**Rationale:**

- Big-endian reads naturally in hex dumps and is the traditional network byte order.
- Fixed sizes are simple to parse and diagram.
- A strict `bool` keeps encodings canonical.

---

## D-048: Chunk numbering

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- Chunks are numbered with a single bundle-wide index, 0 to N−1.
- The index runs across all files in manifest order.
- Every file starts at a new chunk, so a chunk never spans two files.

**Rationale:**

- Every reference to a chunk is one number.
- One have-list covers the whole bundle.
- Mapping a chunk to its file is a short lookup.

---

## D-049: Have-lists as ranges

**Date:** 2026-10-06 · **Status:** Decided

**Options considered:**

- Bitmap (1 bit per chunk)
- Ranges
- Both, with the sender picking the smaller
- A bitmap over coarser groups

**Decision:** A peer describes the chunks it has as a list of `[start, end)` ranges.

**Rationale:**

- A complete or sequentially downloading peer needs only a few bytes.
- A bitmap for a 100 GB bundle is about 800 KB per peer, since 16 KiB chunks make about 6.5 million of them.

**Trade-off accepted:** Range lists can grow large when holdings are very scattered. The simulator should measure how large they get in practice.

---

## D-050: Upload signals

**Date:** 2026-10-06 · **Status:** Decided

**Context:** BitTorrent's choking bundles together an algorithm (whom to upload to, which is policy) and signals that announce its decisions (protocol). The research found:

- Concentrating upload on a few peers is well proven.
- A rotating slot for strangers is proven necessary, though it's also the main thing cheaters exploit.
- Reciprocity works in practice but can be gamed: BitTyrant gained +70% and BitThief free-rode.
- The "interested" signal is cheap and universally useful.
- Better-designed alternatives (PropShare, FairTorrent, T-Chain) are proven mostly in papers.

**Decision:** The protocol includes three things:

1. An *interested / not interested* signal.
2. A *slot granted / revoked* signal (BitTorrent's unchoke/choke, to be renamed).
3. A rule that every request gets exactly one answer: the data or a refusal with a reason (as in BitTorrent's BEP 6).

**Rationale:** This supports BitTorrent's proven slot-based algorithm efficiently, and also lets per-chunk strategies such as FairTorrent run on equal terms in the simulator.

---

## D-051: Default upload strategy

**Date:** 2026-10-06 · **Status:** Decided (revisit with simulation data)

**Decision:** The default upload strategy is BitTorrent's deployed choking algorithm: about 4 slots given by reciprocity, plus 1 rotating slot for strangers. PropShare, FairTorrent and "cooperative / no incentives" are alternative strategies that the simulator compares.

---

## D-052: Message names

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** Messages with an on/off state are one message with a yes/no field, not a pair of messages.

| Stream | Message | Job | BitTorrent equivalent |
| --- | --- | --- | --- |
| Control | **Hello** | First message: bundle ID, capabilities | handshake + extended handshake |
| Control | **Have** | Every chunk range I have | bitfield / have all / have none |
| Control | **HaveUpdate** | Ranges added to or removed from what I have | have / lt_donthave |
| Control | **Want** (yes/no) | You have chunks I'd like / not anymore | interested / not interested |
| Control | **Serving** (yes/no) | If you send requests now, I intend to answer them with data / not | unchoke / choke |
| Control | **Peers** | Other peers in this swarm | ut_pex |
| Control | **Goodbye** | I'm closing the connection, and why | (none) |
| Request | **Request** | Please send these chunks, with the proof I need | request |
| Request | **Data** | The requested chunks and proof | piece |
| Request | **Reject** | No, and here's why | reject (BEP 6) |
| Request | **ManifestRequest** / **Manifest** | Ask for / deliver the bundle's manifest | ut_metadata |

**Notes:**

- "Serving" was chosen over "Slot (granted/revoked)". Slots only exist under some upload strategies (they don't exist under FairTorrent or cooperative strategies), while "Serving" names the promise that holds under every strategy.
- "Want" replaces BitTorrent's "interested". "Have" keeps BitTorrent's familiar word.

---

## D-053: Pipelining limits

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** In its Hello, each peer announces two limits:

- the maximum number of chunks one Request may cover (a power of two)
- the maximum number of Requests a peer may have open with it at once

Requests beyond these limits are rejected.

**Rationale:**

- Keeping enough data in flight (bandwidth × round-trip time) is the most important per-connection speed lever.
- Letting providers announce their limits adapts to fast and slow peers.
- The simulator can sweep the values.

---

## D-054: Full proofs

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** Every Data message carries the full proof from the requested range up to the file's root.

**Rationale:**

- Each Data message is self-contained and verifiable on its own.
- The overhead is about 0.03% for a 1 MiB request in a 1 GiB file.

---

## D-055: Opening sequence

**Date:** 2026-10-06 · **Status:** Superseded by D-110

**Decision:** On the control stream, both peers immediately send Hello and then Have. Have may be empty. Any other opening is a protocol violation.

**Rationale:** Every conversation starts identically, so there's no "unknown holdings" state to handle.

---

## D-056: Manifest delivery

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- A manifest is delivered whole, in one Manifest message.
- The receiver verifies it against the bundle ID.
- The spec sets a maximum manifest size of 64 MiB.

**Rationale:**

- It's the simplest approach.
- The size limit bounds how much a cheating peer can waste before verification fails.
- Chunked, verifiable manifests can be revisited if huge bundles need them.

---

## D-057: Aligned requests

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** A Request covers an aligned range of chunks within a single file:

- the count is a power of two
- the start (relative to the file's first chunk) is a multiple of the count

**Rationale:**

- An aligned range sits under exactly one node of the file's chunk tree, so its proof is just the path from that node to the root: one sibling hash per level.
- This is much simpler to specify, implement and teach than proofs for arbitrary ranges.

**Trade-off accepted:** Requesters split unaligned needs into several aligned requests.

---

## D-058: Proposed protocol defaults accepted

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** The owner accepted the defaults proposed while drafting the protocol:

- message type numbers (`0x01`–`0x07` control, `0x10`–`0x14` request)
- Goodbye reason codes (`done`, `shutting_down`, `unknown_bundle`, `duplicate`, `protocol_violation`, `bad_data`)
- Reject reason codes (`busy`, `dont_have`, `invalid`)
- the manifest's leading `version` byte
- peer exchange limits: at most 50 peers per message, at most one message per minute
- "peer ID", "relay" and "home relay" as final names

---

## D-059: Chunk selection

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- **Strategies implemented:** random; rarest-first (with the first few chunks chosen at random); sequential; streaming window (priority near a playback position, rarest-first elsewhere).
- **Default:** rarest-first with random first few chunks.

**Rationale:**

- Rarest-first keeps every chunk alive and gives peers something to trade, and research found it close to ideal.
- Picking the first few chunks at random gets newcomers something to trade quickly.
- Sequential is kept mainly as an experimental contrast.

---

## D-060: Request size and in-flight amount

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- Request size is one configured size (e.g. 16 chunks), capped by the provider's limit. Requests are smaller only when needed: at file ends and during endgame.
- The amount in flight per peer is adaptive: the measured rate × round-trip time, within the provider's limits.

---

## D-061: Endgame

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** Two endgame strategies, compared in simulation:

- **Limited** (default): once every missing chunk has been requested, request each remaining one from up to *k* additional peers (default *k* = 2).
- **Classic**: request the remaining chunks from every peer that has them.

In both, duplicates are cancelled by resetting their streams when the first copy arrives.

---

## D-062: Upload strategies

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** All four are implemented behind one interface. Each is essentially a different rule for ranking peers.

- **BitTorrent** (default, per D-051): every ~10 s, serve the ~4 peers that uploaded to us fastest over the last ~20 s; every ~30 s, rotate one extra slot to a random interested peer.
- **PropShare:** split upload among peers in proportion to what each gave us.
- **FairTorrent:** always send the next chunk to the peer we owe most (bytes received minus bytes sent).
- **Cooperative:** serve everyone who wants data; for trusted swarms.

**Rationale:** The owner asked for the option with the best research results, and for several if they're simple to build. They are.

- In papers, FairTorrent had the best fairness and the best speed for high contributors.
- PropShare has the strongest resistance to gaming.
- Only BitTorrent's algorithm is proven at internet scale.

Which one should be the default is one of the first questions for the simulator.

---

## D-063: Complete peers and superseeding

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- A complete peer serves interested peers round-robin by default.
- Superseeding is available as an opt-in option. When a bundle is brand new and has only one complete peer, that peer offers each chunk to only one peer until every chunk has been sent once.

---

## D-064: Connections per bundle

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- The default is at most 50 connections per bundle (configurable).
- When over the limit, a client prefers peers that are exchanging data with it and low-latency peers, and drops idle ones.

---

## D-065: Lingering

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- After completing a public-swarm download, a client keeps sharing at least until it has uploaded as much as it downloaded (ratio 1.0), or until nobody has wanted anything from it for a while.
- It then stops, unless the user chose to keep sharing.

**Rationale:** Hajek and Zhu showed that swarms can become unstable if peers leave the moment they finish; staying long enough to upload about one extra chunk fixes it. The ratio rule satisfies that while respecting users.

---

## D-066: Direct sends after completion

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** In a direct send, the sender stops sharing once the recipient's Have covers every chunk.

---

## D-067: Bad data

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** When a peer sends a chunk that fails verification, the client closes the connection with Goodbye (`bad_data`) and refuses that peer ID for this bundle.

**Rationale:**

- QUIC/TLS guarantees data isn't corrupted in transit.
- Each chunk is verified exactly.
- A failure therefore proves misbehavior or a bug, never bad luck.

---

## D-068: Simulation architecture

**Date:** 2026-10-06 · **Status:** Decided

**Options considered:**

- A sans-IO core with our own simulator
- Real binaries under Shadow
- Swapping the async runtime (turmoil / madsim)

**Decision:**

- Protocol and strategy logic are written "sans-IO": plain state machines that never touch sockets, clocks or global randomness. They are told what happened and at what time, and answer with actions and their next timer.
- Production runs them under a thin tokio + QUIC driver.
- Simulation runs the same code under Tempest's own discrete-event simulator, with seeded randomness, so runs are reproducible.
- Real binaries are checked occasionally under network emulation (Shadow or Linux network namespaces).

**Rationale:**

- One codebase serves production, simulation, and possibly in-browser learning-guide widgets.
- Simulated time runs as fast as the CPU allows.
- Runs replay deterministically.
- The logic is easier to read without async plumbing.

**Discipline required:** The core must never read the real clock or real randomness.

---

## D-069: Simulator network model

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- Start with a message-level network model: per-link bandwidth, latency and loss, with fair sharing of capacity. It scales to 10,000+ peers.
- Later, add a packet-level mode that runs real QUIC (`quinn-proto`, itself sans-IO) through the simulated network, for small validation scenarios.

---

## D-070: Simulator model scope (v1)

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** Beyond bandwidth and latency, the first version models:

- churn and arrivals
- NAT reachability and relays (including hole-punch success)
- disk write speed and hashing CPU cost
- misbehaving peers: free riders, bad data and strategic clients

---

## D-071: Simulator scale

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** The simulator targets swarms of up to about 10,000 peers on a single machine.

---

## D-072: Scenario and experiment files

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- One YAML file describes both a scenario (network, peer classes, bundles, arrivals, events over time) and the experiment run in it (the variants to compare, as a grid, and the number of seeds).

---

## D-073: Statistics

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** Statistical practice is built in.

- Each variant runs with many seeds.
- Variants are compared with paired seeds, so both arms see the same random world.
- Reports show medians with confidence intervals, and whether differences are meaningful.

---

## D-074: Results storage and viewing

**Date:** 2026-10-06 · **Status:** Decided (storage resolved by D-075)

**Decision:**

- Simulation results go into a local database that a Rust web UI reads and displays. There is no per-report generated HTML.
- The UI is held to a high bar: beautiful, with high-quality interactive charts.
- Parquet was initially chosen for output data. D-075 replaces it with DuckDB for everything.

**Rationale:** One results UI rather than duplicated HTML per report. A real database makes results easy to test and keeps the simulator's output production quality.

**Gate:** Before heavy implementation, build a test UI in Rust to judge whether Rust can meet the quality bar (recorded in `index.md`).

---

## D-075: Results database

**Date:** 2026-10-06 · **Status:** Decided

**Options considered:**

- DuckDB for everything
- SQLite for the catalog and summaries, plus Parquet event logs
- SQLite for everything

**Decision:** DuckDB for everything. Experiments, runs, summary metrics and full event logs all live in one local DuckDB file, which the results UI queries.

**Rationale:**

- One local file, queried with SQL.
- Built to scan and aggregate very large event tables quickly, so it suits interactive charts.
- Has Rust bindings.
- Can export Parquet if needed.

**Trade-off accepted:** Slower compile times, since DuckDB is bundled.

---

## D-076: Shared design system

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** The client web UI, the tracker catalog and the simulation results viewer share one Rust design-system crate: colors, typography, components and chart styles.

---

## D-077: Regression scenarios

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** A small set of fast "golden" scenarios runs automatically on every change, against saved baselines. Significant drops in speed or fairness are flagged.

---

## D-078: Simulator validation

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** An early milestone is to reproduce, qualitatively, the classic results of Bharambe et al. and Legout et al.: that rarest-first plus choking performs near-optimally. The simulator isn't trusted for new conclusions until it does.

---

## D-079: Process model

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- A background engine (daemon) does all transfers and exposes an API.
- The web UI, the TUI and the CLI are clients of that API.
- Everything ships as one `tempest` binary with subcommands (e.g. `tempest daemon`, `tempest tui`, `tempest send`, `tempest get`).
- The TUI and CLI can start an embedded engine when none is running.

---

## D-080: Engine API

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- The engine exposes a versioned HTTP + JSON API.
- It also exposes a resumable server-sent event stream for live progress and decision events, with event IDs so a client can resume after disconnecting.

**Rationale:**

- The API can be explored with `curl` and a browser, and matches the tracker's API style.
- Push updates avoid the polling that Transmission and qBittorrent rely on.

---

## D-081: Client config format

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** The client's config file is YAML, the same format as simulation scenarios, so profiles look identical in both.

---

## D-082: Client state storage

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** The client stores its state in SQLite, via `rusqlite` with bundled SQLite. State includes bundles, verified chunks (for fast resume), settings and known peers.

---

## D-083: Workspace layout

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** A fine-grained Cargo workspace:

| Crate | Contents |
| --- | --- |
| `protocol` | Wire types, encoding, chunk tree, proofs, manifest, links. No async, no I/O |
| `core` | Sans-IO peer and bundle logic, strategies, decision events |
| `net` | QUIC driver, NAT/port mapping, relay client, LAN discovery, tracker client |
| `engine` | The daemon: bundles, SQLite state, config and profiles, HTTP API and events |
| `sim` | Simulator, scenarios, DuckDB results |
| `ui-kit` | Shared design system |
| `web` | Client web UI |
| `tui` | Terminal UI |
| `sim-web` | Simulation results UI |
| `tracker` | Tracker server and catalog |
| `relay` | Relay library and binary |
| `tempest` | The `tempest` binary (CLI) |

**Rationale:** `protocol` and `core` sit at the center, pure and dependency-light. `protocol` is the reference other implementers can read. Everything with real I/O sits around them.

---

## D-084: Async runtime

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** tokio, multi-threaded.

---

## D-085: Concurrency style

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- The engine's drivers are actors: tasks that each own their state and talk through bounded channels. For example, one task per bundle drives its sans-IO state machine; connection tasks move bytes; a disk task writes chunks.
- There are no shared mutable structures behind locks.

**Rationale:** No locks and no data races, and each actor reads top to bottom. This is the pattern recommended by tokio's maintainers and used by cratetorrent and rqbit.

---

## D-086: Disk I/O

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** Chunks are read and written with positioned I/O (read-at/write-at) on a thread pool, with batched syncs to disk.

**Rationale:** libtorrent's move to memory-mapped I/O in 2.0 caused memory and speed problems, and 2.1 returned to positioned I/O as the default.

---

## D-087: Web UI prototype candidates

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** The Rust UI quality prototype compares three approaches:

- Leptos (WASM)
- Dioxus (WASM)
- server-rendered HTML from axum with Datastar or htmx

The framework for all Tempest web UIs is chosen after the comparison.

---

## D-088: Fallback if Rust UIs fall short

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- All-Rust UIs are strongly preferred, including the charts.
- If the prototype shows that Rust-built charts and design can't meet the owner's quality bar, the fallback is a Vite + TypeScript + React frontend.

---

## D-089: Retro palette exploration

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** The prototype explores four color directions:

- green phosphor
- amber phosphor
- neon vector (Tron / Atari *Tempest* arcade)
- beige-era computing (a light theme)

All four keep a modern, scannable layout with the retro feeling in the skin. Effects respect reduced-motion settings and contrast requirements.

---

## D-090: Terminal UI

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** The TUI uses ratatui, with a k9s/lazygit-style keyboard model:

- panes
- vim keys plus arrow keys
- `/` to filter, `:` for commands, `?` for help
- a persistent hint bar showing available keys

---

## D-091: Learning guide deferred

**Date:** 2026-10-06 · **Status:** Deferred

**Decision:** The learning guide's structure, tooling and timing are decided after the client works. The guide is written after the client works (phase 9, unchanged).

**Ideas captured for later** (from research report 11):

- a chain of broken models, where each section starts from a problem the reader has just seen
- interactive widgets running the real sans-IO core compiled to WebAssembly, each with a static fallback
- an "Illustrated Tempest Connection" generated from a real client trace
- an optional "build a tiny client" track

---

## D-092: Roadmap

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** Implementation follows these milestones (detailed in [roadmap.md](roadmap.md)):

| Milestone | Scope |
| --- | --- |
| M0 | Rust UI quality prototype |
| M1 | Protocol crate |
| M2 | Core + simulator skeleton |
| M3 | Simulator validation |
| M4 | First real transfer |
| M5 | Engine + public swarms |
| M6 | Getting through routers |
| M7 | Terminal UI |
| M8 | Web UIs |
| M9 | Tracker catalog |
| M10 | Learning guide |

Once the owner is happy with M0, the remaining milestones are implemented with a Claude workflow.

---

## D-093: Finish the protocol spec before implementation

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** Every remaining protocol detail is designed before implementation begins, rather than just in time for each milestone.

The remaining details:

- message size limits
- path length limit
- link parameters
- peer exchange privacy
- LAN announcements
- tracker API
- relay protocol

---

## D-094: Message size limits

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** Each message type has its own size limit:

| Messages | Maximum size |
| --- | --- |
| Control messages | 16 MiB per frame |
| Data | The requested chunks (count × 16 KiB) plus the proof |
| Manifest | 64 MiB (D-056) |
| Request, ManifestRequest | Their fixed sizes |

A peer that announces a larger frame is committing a protocol violation.

**Rationale:**

- A length prefix lets a peer demand memory, so every frame needs a bound.
- 16 MiB covers even very fragmented Have lists (about 2 million ranges).

---

## D-095: Path length

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** A manifest path is at most 4,096 bytes in total, in addition to the 255-byte limit per segment. On Windows, clients use long-path support.

---

## D-096: Link parameters

**Date:** 2026-10-06 · **Status:** Decided

**Note:** D-108 clarified that `relay` carries the sharer's relay address (`ip:port`), not a relay identity.

**Decision:** Links use these query parameters:

| Parameter | Meaning | Repeatable |
| --- | --- | --- |
| `bundle` | Bundle ID (required) | No |
| `peer` | Sharing peer's peer ID | No |
| `addr` | Direct address `ip:port`, with IPv6 in brackets | Yes |
| `relay` | The sharer's home relay | No |
| `tracker` | Tracker URL | Yes |
| `name` | Display-name hint | No |

The `name` hint is unverified. Clients show it only until the manifest arrives, then use the manifest's name.

---

## D-097: Peer exchange privacy

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** There is no opt-out. Peers may share any connected peer's address through Peers messages, as in BitTorrent.

**Rationale:** Anonymity is a non-goal (D-002), and an opt-out couldn't be enforced anyway.

---

## D-098: LAN discovery pattern

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- Peers multicast an announce message on startup and every 60 seconds.
- A newcomer can multicast a query, and every peer replies with an announce sent directly back to it.

**Rationale:** With a query, discovery takes milliseconds rather than up to the announcement interval. BitTorrent's BEP 14 announces only every 5 minutes.

---

## D-099: LAN message and addresses

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- **Message:** `"TMPS"` magic, kind (1 = announce, 2 = query), peer ID, QUIC port.
- **IPv4 group:** `239.255.84.77`, in the local-scope range applications may choose for themselves (RFC 2365).
- **IPv6 group:** `ff12::7465:6d70`, a transient, link-local group.
- **Port:** UDP 4774, which is unassigned in the IANA port registry.

**Rationale:**

- Announcements aren't signed. A forged announcement only wastes one connection attempt, because TLS verifies the peer ID when connecting.
- The magic bytes filter out unrelated traffic on the same group or port.

---

## D-100: Tracker announces

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- A peer announces when it starts, when it completes and when it stops.
- It also re-announces at an interval chosen by the tracker.
- There is no live push stream.

**Rationale:** The tracker stays simple and stateless, and scales well. The catalog's view of the network is at most one interval stale.

---

## D-101: Unsigned announces

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** Announces are not signed. The tracker records the address it observes from the HTTPS connection.

**Rationale:** A fake peer ID only wastes connection attempts, since TLS verifies the peer ID on connect.

---

## D-102: Tracker stats endpoint

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** The tracker provides an endpoint returning a bundle's complete and incomplete peer counts without announcing. BitTorrent calls this "scrape".

---

## D-103: Tracker endpoints

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** Endpoints live under a versioned path prefix:

- `POST /tempest/v1/announce`
- `GET /tempest/v1/bundles/{bundle}`
- Errors are an HTTP status plus a JSON body: `error`, `message` and an optional `retry_after_secs`.

The full field list is in `protocol/trackers.md`.

---

## D-104: Relay forwarding model

**Date:** 2026-10-06 · **Status:** Decided

**Options considered:**

- Lend a public address (TURN-style)
- Envelopes addressed by peer ID (iroh/Tailscale-style)

**Decision:** Lend a public address.

- A registered peer gets a **relay address** (relay IP + a port) on its relay.
- Anyone can dial that address like any other address.
- The relay hands those packets to the registered peer wrapped inside its relay connection (QUIC datagrams: Forward / Forwarded).

**Rationale:** The dialing side needs no special code. Only the registered peer wraps and unwraps packets, as in TURN.

**Trade-off accepted:**

- Relays use one port per registered peer.
- Registered peers need a small wrapping layer beneath QUIC.

---

## D-105: Upgrading relayed connections

**Date:** 2026-10-06 · **Status:** Decided

**Options considered:**

- Open a new direct connection
- Migrate the existing connection to a direct path

**Decision:**

1. Peers connected through a relay exchange their direct candidate addresses in a **Candidates** control message.
2. Both then immediately dial each other's candidates. The crossing dials are the hole punch.
3. If a direct connection opens, it's kept and the relayed one is closed. In-flight requests are re-requested.
4. The duplicate-connection rule (D-037) gains a preference: direct beats relayed.

**Rationale:** Works with standard QUIC (no multipath or NAT-traversal extensions) and is easy to explain.

---

## D-106: Relay identity

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- Relays authenticate with Ed25519 raw public keys, like peers.
- A relay is identified as `<key>@host:port`, so self-hosting needs no domain or certificate.
- Peers register using ALPN `tempest-relay/1`.

---

## D-107: Relay access

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- A relay's operator may configure an allowlist of peer IDs. With no allowlist set, anyone may register.
- Relays may cap bandwidth per peer.

---

## D-108: Relay details

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- **Registering:** Register / Registered on the relay connection. Registered returns the peer's relay address and its observed public address.
- **Forwarding rule:** a relay forwards a registered peer's packets only to addresses that recently sent packets to that peer's relay address.
- **Stable addresses:** a peer that reconnects within 5 minutes SHOULD get the same relay address.
- **Default port:** UDP 4775.
- **Advertising:** links, tracker announces and Peers carry a peer's relay *address*. A relay's identity (`<key>@host:port`) appears only in client config and in trackers' relay recommendations.

---

## D-109: Candidates message

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** A new control message, **Candidates**, carries a peer's direct candidate addresses for hole punching.

**Rationale:** "Candidates" is the standard term in NAT traversal (ICE).

---

## D-110: Opening sequence, revised (supersedes D-055)

**Date:** 2026-10-06 · **Status:** Decided

**Context:** A protocol review found D-055 unworkable. The accepting peer can't send Hello until it learns from the dialer's Hello which bundle the connection is for. Also, QUIC doesn't order different streams relative to each other.

**Decision:**

1. The dialing peer sends Hello then Have.
2. The accepting peer reads them, then replies with its own Hello then Have. If it isn't in that bundle, it closes the connection instead.
3. The "must come first" rule applies to the control stream only. A request stream that arrives before the opening finishes is held until it does, not treated as a violation.

---

## D-111: Resolving duplicate connections (refines D-037 and D-105)

**Date:** 2026-10-06 · **Status:** Decided

**Context:** "Keep the connection opened by the lower peer ID" can't choose between two connections that the same peer opened.

**Decision:**

- Of two peers with duplicate connections for a bundle, only the peer with the lower peer ID decides which connection to keep. It prefers a direct connection over a relayed one, then the earliest established. It closes the others with reason `duplicate`.
- The higher-ID peer never closes a duplicate on its own initiative.
- A peer stops dialing candidates once one direct connection is established.

---

## D-112: Goodbye travels in QUIC's close frame

**Date:** 2026-10-06 · **Status:** Decided

**Context:** In QUIC, closing a connection discards stream data that hasn't been delivered, so a Goodbye sent on the control stream and then followed by a close is usually lost.

**Decision:**

- Goodbye is not a stream message.
- A peer says goodbye by closing the connection with QUIC's application CONNECTION_CLOSE frame: the error code is the reason code and the reason phrase is the message. QUIC delivers both reliably as part of closing.
- Message type `0x07` is retired.

---

## D-113: Self-advertised addresses in Hello

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** Hello carries the sender's own direct `addresses` and `relay` address, so every connection learns how to reach the sender, and peer exchange can pass those addresses on.

---

## D-114: Hello is extensible

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- Receivers ignore extra bytes after Hello's known fields, so a future version can add fields compatibly.
- Every other message must still match its fields exactly.

---

## D-115: Size caps on chunk counts

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- `max_request_chunks` is at most 1,024 (16 MiB per request).
- A bundle may contain at most 2³² − 1 chunks, since chunk indexes are `u32`. Larger manifests are invalid.

---

## D-116: Protocol review fixes

**Date:** 2026-10-06 · **Status:** Decided

**Context:** A full review of `protocol/` found these mechanical issues. All were fixed without changing earlier design intent.

**Fixes:**

- **Packet size:** peers send UDP payloads of at most 1,200 bytes until path MTU discovery confirms more, so packets fit through relays. Relays accept datagram payloads of at least 1,300 bytes.
- **Empty files:** a peer with the manifest has every empty file's chunk and includes it in Have. Verification handles the empty chunk explicitly.
- **Manifest requests:** a provider SHOULD answer ManifestRequest regardless of Serving, MAY Reject (`busy`), and ManifestRequest counts toward `max_open_requests`.
- **"Open" requests:** these are counted from the provider's view. A reset stream needs no answer.
- **Tracker `your_addr`:** renamed `your_ip`. It comes from an HTTPS (TCP) connection, so its port is meaningless for UDP.
- **Relayed connections:** both peers send keep-alives at least every 60 seconds, so the relay's forwarding permission doesn't lapse.
- **Candidates:** a peer that receives Candidates replies with its own if it hasn't sent them yet. The registered peer also dials the source address it observes.
- **One UDP socket:** all of a peer's QUIC traffic uses one UDP socket per address family.
- **Manifest validity:**
  - A precise reserved-name rule and Unicode simple case folding.
  - Case-insensitive "inside a file or symlink" checks.
  - Directory entries only for empty directories.
  - Size-0 files must have the empty root.
  - Symlink targets must not pass through other symlinks.
  - Clients create symlinks last.
- **Unknown reason codes** are treated as generic, not as violations.
- **Streams:** unidirectional streams are not used, and the control stream is QUIC stream 0.
- **Addresses:** IPv4-mapped IPv6 addresses are encoded as family 4.
- **New Goodbye reason:** `too_many_connections` (6).
- **Terminology:** the term "relay address" (`ip:port`, lent by a relay) is used consistently. It is distinct from a relay's identity, `<key>@host:port` (refines D-096).
- **Bad data:** the spec requires discarding chunks that fail verification. Closing the connection and banning the peer remains client policy (D-067).
- **Message types:** D-058's list gains `0x08` (Candidates, D-109). `0x07` is retired (D-112).
- **Test vectors:** added for a manifest with its bundle ID, and for a framed Hello.

---

## D-117: Web UI framework: Leptos

**Date:** 2026-10-06 · **Status:** Decided

**Context:** M0 built the same live page in Leptos 0.8.22, Dioxus 0.7.10, and server-rendered axum + Datastar (`prototypes/ui/`). All three met the quality bar, so Rust UIs are confirmed and the React fallback (D-088) isn't needed. Research compared Leptos and Dioxus for Tempest. The reports are `leptos-vs-dioxus.md` and `ai-agents-leptos-dioxus.md` in `planning/research/`.

**Decision:** All Tempest web UIs (client, tracker catalog, simulation results) use Leptos.

**Rationale:**

- **Performance:** smoothest in the prototype (under 1 ms per update) and the smallest download (129 KB gzipped WASM vs 181 KB). It updates the DOM directly, with no virtual DOM.
- **Catalog fit:** mature server-side rendering, streaming and islands suit the tracker catalog. Dioxus is client-rendered first and has no islands mode.
- **Engine fit:** `leptos_axum` integrates directly with the engine's axum server.
- **Stability:** 0.8 has been stable for 17 months, and 0.9 is described as needing little migration. Dioxus has had a breaking release about once a year, and its 0.8 breaks again.
- **Stars:** Dioxus's larger star count mostly reflects its desktop and mobile scope.
- **AI agents:** no meaningful difference in how well coding agents handle either.

**Risks accepted:**

- Leptos's author describes it as feature-complete and "lightly maintained". Dioxus has its own reduced focus (its team joined Cognition), so neither is strongly backed.
- Removing a subscriber in Leptos's `reactive_graph` costs O(n). Many per-item memos subscribed to one shared signal therefore become quadratic.

**Guardrails:**

- Pin Leptos 0.8, and move to 0.9 when it is released.
- No per-item memos subscribed to a shared signal. Instead, use one effect that diffs into per-item signals, `Selector`, or keyed stores. This rule is recorded in `specs/interfaces.md` (Leptos guidelines).
- Keep a performance benchmark of a 1,536-cell live view in CI.
- Keep UI logic in plain Rust modules so the framework layer stays thin.
- Revisit if Leptos maintenance visibly stalls, e.g. no 0.9 release by mid-2027.

---

## D-118: Client feature set (v1)

**Date:** 2026-10-06 · **Status:** Decided

**Context:** The owner reviewed qBittorrent-style features after trying the dummy TUI (`prototypes/tui/`). The same features apply to the TUI and the web UI.

**In:**

- **Transfer controls:**
  - start/stop
  - force start (bypasses the queue)
  - remove, keeping or deleting files
  - rename
  - open the download folder
- **Queue:** limits on active downloads and uploads; queued transfers can be reordered.
- **Recheck:** re-verify data on disk against the manifest.
- **Speed limits:** global and per transfer, plus an alternative-limits mode that can be toggled or scheduled.
- **Sharing limits:** stop or remove a transfer at a ratio or after a sharing time.
- **Organizing:**
  - tags (many per transfer)
  - categories (one per transfer, each with a default save folder)
  - move storage
  - custom columns and saved views
- **Finding transfers:** filter by name or tag; filter by status; sort by name, size, progress, status, download rate, upload rate, peers serving us, time left, ratio and date added.
- **Sharing:** create a bundle from a local file or folder and get a `tempest:` link (direct sends).
- **Notifications:** notify when a transfer completes or errors.

**Out for now:**

- **Files:** file selection, file priority, streaming/preview, renaming files.
- **Adding and discovery:** watched folders, feeds, tracker search from the client.
- **Monitoring:** trackers tab, global stats, client-wide event log.
- **Peer management:** ban, add, disconnect, peer details.
- **Automation:** run a command on completion, start on login and tray icon, keep awake, battery and metered awareness.

**Statuses:** downloading, sharing, stalled, queued, stopped, completed, errored, plus checking (recheck) and moving (move storage).

---

## D-119: TUI and web UI are designed together

**Date:** 2026-10-06 · **Status:** Decided

**Context:** The owner reviewed the dummy TUI (`prototypes/tui/`) and found it "really great", with little specific feedback.

**Decision:**

- The TUI and the client web UI share a feature set (D-118), vocabulary, statuses, state explanations and information hierarchy.
- Feedback on one, starting with the planned layout rethink, is considered for the other wherever it makes sense.
- Full parity is not a goal. Each keeps its natural affordances: the TUI is keyboard-first and dense, and the web UI has pointer interactions and richer charts.
- The client layout rethink happens before M7, so the TUI and web UI start from the same layout thinking.

---

## D-120: Implementation scope for the workflow

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** A Claude workflow implements milestones M1 through M9 in one run. M10 (the learning guide) remains deferred (D-091). Before the run, open questions are worked through with the owner.

---

## D-121: License

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** `MIT OR Apache-2.0`, the Rust ecosystem's standard dual license.

---

## D-122: Toolchain

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- `rust-toolchain.toml` pins the latest stable Rust.
- Nightly is used only where a tool requires it, such as `cargo-fuzz`.

---

## D-123: Crate names and CI

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- Crates are named `tempest-*` (e.g. `tempest-protocol`, `tempest-core`). Their folders keep short names under `crates/`.
- GitHub Actions CI runs from the first commit of code: formatting, clippy, tests and `cargo-deny`.
