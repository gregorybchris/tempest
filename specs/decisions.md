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

**Date:** 2026-10-06 · **Status:** Decided (design to come; revised by D-234)

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

**Date:** 2026-10-06 · **Status:** Decided (revised by D-223)

**Decision:**

- A complete peer serves interested peers round-robin by default.
- When a bundle is brand new and has only one complete peer, that peer offers each chunk to only one peer until every chunk has been sent once; D-223 sets when this is on by default.

---

## D-064: Connections per bundle

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- The default is at most 50 connections per bundle (configurable).
- When over the limit, a client prefers peers that are exchanging data with it and low-latency peers, and drops idle ones.

---

## D-065: Lingering

**Date:** 2026-10-06 · **Status:** Decided (revised by D-224)

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

**Date:** 2026-10-06 · **Status:** Decided (revised by D-236)

**Decision:**

- A background engine (daemon) does all transfers and exposes an API.
- The web UI, the TUI and the CLI are clients of that API.
- Everything ships as one `tempest` binary with subcommands, except the simulator (D-214) (e.g. `tempest daemon`, `tempest tui`, `tempest send`, `tempest get`).
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

**Date:** 2026-10-06 · **Status:** Decided (revised by D-226)

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

**Date:** 2026-10-06 · **Status:** Superseded by D-213

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

---

## D-124: CLI vocabulary

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** The CLI uses Tempest's own verbs:

| Command | Purpose |
| --- | --- |
| `tempest share <path>` | Create a bundle and print its link |
| `tempest get <link>` | Download a bundle |
| `tempest list` | Show transfers |
| `tempest start` / `stop` / `remove` | Control a transfer |
| `tempest daemon` | Run the engine |
| `tempest tui` | Open the terminal UI |
| `tempest web` | Open the web UI |
| `tempest relay` | Run a relay |
| `tempest tracker` | Run a tracker |

This replaces the placeholder command names in earlier roadmap text (`make`, `send`).

---

## D-125: Engine API access

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- The engine's API and web UI bind to localhost by default.
- Remote access is an opt-in setting that requires a token or password and HTTPS.

---

## D-126: Default ports

**Date:** 2026-10-06 · **Status:** Decided

**Decision:** All ports are configurable.

| Port | Use |
| --- | --- |
| UDP 4773 | Peers (QUIC) |
| UDP 4774 | LAN discovery (D-099) |
| UDP 4775 | Relays (D-108) |
| TCP 4776 | Engine API and web UI |
| TCP 4777 | Tracker |

---

## D-127: Undecided questions during implementation

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- When an implementing agent meets a design question no decision covers, it picks the option most consistent with existing decisions and continues.
- It records the choice in this log with **Status: Provisional**, the options considered, and its rationale.
- The owner reviews every provisional decision afterwards.

---

## D-128: Catalog accounts

**Date:** 2026-10-06 · **Status:** Decided (revised by D-234)

**Decision:**

- Browsing a tracker catalog is open.
- Publishing, comments and votes require signing in with a Tempest key: the client signs a challenge to prove ownership of its peer key. There are no passwords.
- Operators may require invites.

---

## D-129: Catalog listing is opt-in

**Date:** 2026-10-06 · **Status:** Decided (revised by D-234)

**Options considered:**

- Opt-in listing
- Explicit publish only
- Auto-list everything announced

**Decision:**

- A bundle appears in a catalog only when its sharer opts in, via a per-tracker setting or `tempest share --publish`.
- Opted-in announces carry a listing flag. The tracker then fetches the manifest from peers and lists the bundle automatically.
- Title, description and category can be edited in the web form or the client.

**Rationale:** Auto-listing everything would publish private bundles that only use the tracker to find peers, and would make operators index all content by default (D-031).

**Implications:**

- The tracker API (`protocol/trackers.md`) gains an optional listing field on announces.
- The listing is attributed to the signed-in key that requested it.

---

## D-130: Catalog search and ranking

**Date:** 2026-10-06 · **Status:** Decided

**Decision:**

- Search uses Tantivy: relevance ranking, typo tolerance, and facets for categories and tags.
- Ranking and sorting use live swarm health from the tracker's own announces, votes from signed-in users, and downloads over time (trending).

---

## D-131: Client layout direction

**Date:** 2026-10-06 · **Status:** Decided (revised by D-234)

**Context:** The owner felt the M0 layout needed rethinking. The direction applies to both the TUI and the web UI (D-119).

**Decision:**

- **List first, detail on demand.** The transfers list is the main surface: full width, with more columns. A transfer's detail opens in a drawer or split only when selected or expanded.
- **App-level navigation.** A clear navigation covers the app's areas, such as transfers, share, trackers/catalog and settings (limits, queue, categories).
- **Less at once.** Everyday information comes first. The chunk map, charts, peers and decisions sit one level deeper.

---

## D-132: Testing bar

**Date:** 2026-10-07 · **Status:** Decided

**Decision:** A milestone counts as done only when its code has:

- **Unit and property tests:** for protocol and core logic, including the spec's test vectors.
- **Fuzzing:** a `cargo-fuzz` target for every parser of untrusted input, run briefly in CI.
- **End-to-end tests:** real peers, a relay and a tracker on localhost in CI, covering share → get → verify, including a relayed connection.
- **Simulation baselines:** golden scenarios run against saved baseline results, to confirm transfer speed isn't regressing. The default run is quick enough to run between milestones. A setting runs them longer for more certainty.

UI screenshot tests are not required.

---

## D-133: Releases

**Date:** 2026-10-07 · **Status:** Decided

**Decision:**

- `cargo-dist` builds GitHub releases for macOS, Linux and Windows: shell and PowerShell installers, a Homebrew tap, and MSI. No code signing yet.
- The README documents installing from a release. Configuration details are in D-211.

---

## D-134: Workflow quality gates

**Date:** 2026-10-07 · **Status:** Decided

**Decision:**

- After each milestone, an independent review checks the work against the specs and runs the tests and CI checks. Fixes are attempted.
- Remaining failures are logged and the workflow continues, which maximizes progress.
- Each milestone is committed and pushed.

---

## D-135: Shipped palettes

**Date:** 2026-10-07 · **Status:** Decided

**Decision:** All four palettes ship as switchable themes in the TUI and web UIs. Green phosphor is the default.

---

## D-136: Four-area app navigation, Transfers as home

**Date:** 2026-10-07 · **Status:** Decided (revised by D-234)

**Context:** D-131 asks for app-level navigation over transfers, share, trackers/catalog and settings.

**Options:** a sidebar; a top tab bar; a command palette only.

**Decision:** A top tab bar with four areas (Transfers, Share, Catalog, Settings) in both interfaces, collapsing to a bottom bar on phones. A sidebar costs width the list needs, and a palette alone hides the app's scope from new users. The `:` palette remains as a shortcut in the TUI.

---

## D-137: Detail is a drawer or split with the summary first

**Date:** 2026-10-07 · **Status:** Decided

**Decision:** The detail opens as a right drawer (wide web), a bottom split (TUI) or a full sheet (narrow web). Its first view is a plain-language summary. Files, peers and decisions are tabs. The chunk map and rates chart are a further level, entered explicitly and left with `esc`. This follows D-131's "less at once" and keeps the list visible while a transfer is inspected.

---

## D-138: List columns degrade by priority

**Date:** 2026-10-07 · **Status:** Decided

**Decision:** Name, progress and status are always shown. Size, ↓ and ETA come next, then ↑. Narrow terminals and windows drop columns in reverse order rather than wrapping or truncating the core ones. Custom columns and saved views (D-118) add to this set.

---

## D-139: Share and Catalog are screens, not dialogs

**Date:** 2026-10-07 · **Status:** Decided (revised by D-234)

**Decision:** Creating a bundle and browsing trackers are full areas, not modal dialogs, because each has a multi-step flow (build, link; browse, search, add). Adding a transfer from a link stays a quick modal on Transfers, since it is one input.

---

## D-140: Strict decoding of non-canonical addresses and hex

**Date:** 2026-10-07 · **Status:** Decided

**Context:** The spec says an IPv4 address MUST be encoded with family 4, and that hashes are written as lowercase hex, but not what a receiver does with an IPv4-mapped IPv6 address sent as family 6, or with uppercase hex.

**Options:** accept and normalize; reject as a protocol violation.

**Decision:** Reject both. Every value then has exactly one valid encoding, as the encoding section intends, and a decoded structure always re-encodes to the same bytes, which the fuzz targets check.

---

## D-141: Toolchain pin and minimum Rust version

**Date:** 2026-10-07 · **Status:** Decided

**Context:** D-122 pins the latest stable Rust. Crates also declare a minimum supported version (`rust-version`).

**Options:** an older MSRV tested separately; MSRV equal to the pin.

**Decision:** `rust-version` equals the pinned toolchain, and CI installs the toolchain from `rust-toolchain.toml`, so the pin is the one source of truth. An older MSRV would need its own CI job and is not useful for a binary distributed through releases.

---

## D-142: Implicit directories count in case-insensitive path checks

**Date:** 2026-10-07 · **Status:** Decided

**Context:** `protocol/content.md` forbade two entries whose paths differ only by case, but said nothing about the directories that implicitly contain entries. Files `A/b` and `a/c` were valid, yet macOS and Windows would write both into one folder, so the bundle could not be written identically everywhere.

**Options:** leave it to clients; forbid case-insensitively equal directory paths too.

**Decision:** Forbid it. No two different paths, counting implicit directories, may be equal when compared case-insensitively. This keeps the Paths section's promise that a valid bundle is written the same way on every system, and makes "a directory entry must be empty" and "nothing inside a file or symlink" unambiguous.

---

## D-143: Bundles made from a single file

**Date:** 2026-10-07 · **Status:** Decided (revised by D-221)

**Context:** A manifest's entries are relative to the bundle's top level, and its `name` is typically the top-level folder. Nothing said what sharing a single file produces.

**Options:** a bundle named after the file with one entry of the same name; a special "single file" manifest form.

**Decision:** Sharing a file creates a bundle named after it, with one entry whose path is the file's name. Sharing a folder creates a bundle named after the folder, whose entries are its contents. The protocol needs no special case. Where a downloader writes a one-file bundle (for example, directly in the download folder rather than in a folder of the same name) is client policy, decided with the download path (M4).

When building a manifest, the client converts file names and symlink targets to NFC and refuses anything else the path rules forbid, naming the offending path, rather than renaming it.

---

## D-144: Link formatting and parsing details

**Date:** 2026-10-07 · **Status:** Decided

**Context:** `protocol/discovery.md` says link values are percent-encoded per RFC 3986 and unknown parameters are ignored, but not which characters are encoded, how `+` is read, or what a malformed known parameter does.

**Decision:**

- Writers percent-encode everything except unreserved characters and `! $ ' ( ) * , ; : @ / ?`, so addresses and tracker URLs stay readable. `&`, `=`, `+`, `#`, spaces, brackets and non-ASCII text are encoded.
- Readers decode `%XX` escapes and take every other character literally. `+` is a plus, not a space (RFC 3986, not HTML forms).
- The scheme is matched case-insensitively. A fragment is ignored.
- A link is rejected if `bundle` is missing, a parameter allowed once appears twice, a known parameter has a malformed value, or `addr`/`relay` appear without `peer`. Rejecting rather than skipping bad values means a mangled link fails loudly.

---

## D-145: Size limits for relay control messages

**Date:** 2026-10-07 · **Status:** Decided

**Context:** `protocol/relays.md` said relay control messages use the framing of `messages.md`, whose size limits are defined per peer message type, but gave no limits for its own types.

**Options:** reuse the 16 MiB control-message limit; limit each type to its largest valid encoding.

**Decision:** Each type is limited to its largest valid encoding: Register is exactly its type byte, Registered holds two IPv6 addresses at most, and RelayReject's message is at most 1 KiB, like Reject's. A relay connection is opened by strangers, so no frame should let one demand more memory than a valid message needs.

---

## D-146: Relay identity text details

**Date:** 2026-10-07 · **Status:** Decided

**Context:** `protocol/relays.md` defines `<relay key>@<host>:<port>` with a default port of 4775, but not whether the port may be omitted or what counts as a valid host.

**Decision:**

- Readers accept a missing `:<port>` as 4775. Writers always write the port.
- A host is an IPv4 address, a bracketed IPv6 address, or an ASCII DNS name (internationalized names as punycode): dot-separated labels of 1–63 letters, digits and hyphens, not starting or ending with a hyphen, at most 253 characters, no trailing dot, and a last label that is not all digits. Names are lowercased; IPv4-mapped IPv6 addresses become IPv4.
- The port is 1–65535, written without leading zeros or a sign.

Strict parsing makes a mistyped relay in a config file or tracker response fail loudly, as with links (D-144).

---

## D-147: Tracker JSON details

**Date:** 2026-10-07 · **Status:** Decided

**Context:** `protocol/trackers.md` lists the API's fields but leaves some reading rules open.

**Decision:**

- In a response's peer entries, `peer`, `addrs` and `relay` are all required, and `relay` may be `null`, as in the request.
- An unknown `error` code is accepted and kept, so a newer tracker's codes still parse. The client then acts on the HTTP status.
- A malformed ID, address or relay identity anywhere in a body makes the whole body invalid, rather than skipping the bad value.
- Text forms are the same as in links: IDs are lowercase hex and IPv4-mapped IPv6 addresses are read as IPv4.


---

## D-148: The core's session and driver boundary

**Date:** 2026-10-07 · **Status:** Decided

**Context:** D-068 makes the core sans-IO but doesn't say what one state machine covers or who handles chunk bytes. The simulator must model 1,000-peer swarms of large bundles, which it can't do if every Data carries and hashes real bytes.

**Options:** the core receives and sends whole messages, including chunk bytes, and verifies Data itself; the core sees every message except chunk payloads, and the driver reads, verifies and stores chunks; the core calls a synchronous storage trait.

**Decision:**

- One `Session` per bundle: one peer's participation in one swarm, over all of its connections for that bundle. The engine's bundle actor drives one, and so does each simulated peer.
- The session never sees chunk bytes. It asks the driver to send Data for a range, and the driver builds it from storage. When Data arrives, the driver verifies it (the session offers a helper that uses the manifest), stores it, and reports only the outcome. Manifests do pass through the session, which checks them against the bundle ID.
- Request streams are named on both sides: the session names the streams it opens, the driver names the ones the other peer opens. The driver reports when it has finished sending Data, so the session can limit concurrent uploads and let the upload strategy order them.
- Every random choice comes from a ChaCha8 generator seeded by the driver, which gives the same sequence on every platform. Session state uses ordered maps, so iteration order never depends on hashing.

**Rationale:** Storage I/O stays in the driver, where it belongs in the engine's actor design (D-085), and the simulator can model payloads by size alone. A synchronous storage trait would block the bundle actor on disk.

---

## D-149: Session policies the strategies spec leaves open

**Date:** 2026-10-07 · **Status:** Decided

**Context:** Implementing the session needed answers to small policy questions not covered by `specs/strategies.md` or the protocol.

**Decision:**

- **Requests from peers not being served** are rejected with `busy`. Requests already accepted are still answered after this peer stops serving the requester.
- **After a Reject**, this peer sends that provider no further requests for a configurable delay (default 1 s), instead of retrying at once.
- **Have before the manifest:** Have and HaveUpdates received before the manifest are kept as received and checked once it arrives. A peer that sends more than a configured number of ranges before then is disconnected with `done`.
- **A manifest that doesn't match the bundle ID, or doesn't decode,** counts as bad data (D-067): the sender is closed with `bad_data` and banned.
- **A banned peer that connects again** is closed at once with `bad_data`.
- **Only one ManifestRequest is in flight at a time.** A peer that answers `dont_have` isn't asked again until it announces chunks.
- **Complete peers don't dial.** They wait for incomplete peers to connect.
- **Candidates** is answered with this peer's own direct addresses. Hole punching itself is left to the driver.

**Rationale:** Each choice is the simplest that avoids busy loops or unbounded memory, and each can be revisited with simulation data.

---

## D-150: Settings keys and overrides

**Date:** 2026-10-07 · **Status:** Decided

**Context:** D-005 calls for profiles plus per-setting overrides, and the simulator's scenario files vary settings by dotted key (`specs/simulation.md`). The key names were undecided.

**Options:** one flat list of settings; settings grouped by topic and addressed by dotted key; a free-form string map interpreted by each strategy.

**Decision:**

- Settings are grouped by topic: `strategies` (which strategy makes each decision), then one group per topic for parameters (`selection`, `requests`, `endgame`, `upload`, `connections`, `lingering`). A setting's key is its group and name, such as `strategies.chunk_selection` or `upload.reciprocal_slots`. The core's `Config` type is the source of truth for the list.
- Strategy names are written in kebab case (`rarest-first`, `bittorrent`), and durations as a number and a unit (`"10s"`, `"250ms"`), or a bare number of seconds.
- A profile and a map of overrides together resolve to a config. An unknown key, a key naming a whole group, or a value out of range is an error, and leaves the config unchanged.
- Strategy choice and strategy parameters are separate keys, so an experiment can vary the strategy without restating its parameters.

**Rationale:** Grouping keeps the list readable as it grows, and the same keys serve scenario files, a settings screen and a CLI. A free-form map would push validation into every strategy.

---

## D-151: Profile names and contents

**Date:** 2026-10-07 · **Status:** Decided (revised by D-224)

**Context:** `specs/strategies.md` left profile names and their settings open, suggesting "balanced", "background", "classic BitTorrent" and "cooperative".

**Options:** the suggested names; names after use cases only ("public", "direct", "trusted"); names after strategies only.

**Decision:** Five profiles, each a complete config:

- `balanced` (default): the defaults in `specs/strategies.md`.
- `classic`: the classic BitTorrent client as a baseline: one-chunk requests, a fixed pipeline of 5, classic endgame, and the same upload strategy once complete.
- `cooperative`: serve everyone (Cooperative upload) and keep sharing.
- `background`: polite sharing: 20 connections, 2 uploads at once, 2 reciprocity slots, and a shorter idle timeout for lingering.
- `streaming`: the streaming-window chunk selection.

D-224 removes the lingering from `cooperative` and `background`: profiles no longer change when a download stops sharing, which the `sharing.*` settings alone decide.

**Rationale:** The suggested names cover beginners' choices and the simulator's baselines. `streaming` is added because it is the one user-facing reason to change chunk selection.

---

## D-152: Adaptive pipelining

**Date:** 2026-10-07 · **Status:** Decided

**Context:** D-060 keeps "measured rate × round-trip time" bytes requested from each provider, but how to measure either was open.

**Options for the rate:** a sliding-window average of bytes received; the delivery rate of each response (BBR-style); libtorrent's "rate × fixed queue time".

**Decision:**

- **Requests kept open** = ⌈factor × delivery rate × RTT ÷ full request size⌉, with factor 2, at least 2, and at most the provider's `max_open_requests` less headroom. Before the first measurement, 4. A snubbing provider (D-153) gets 1. Each change is a decision event with its reason.
- **Delivery rate:** for each Data response, the bytes received from that provider between sending the request and receiving its response, divided by that time. The estimate is the largest of the last 8 samples.
- **RTT:** the smallest time from sending any request to its response, unless the driver reports the transport's RTT through `Session::handle_rtt`, which then replaces it.
- A setting turns adaptation off, keeping a fixed number of requests open.

**Rationale:** A factor above 1 lets a pipeline-limited provider's measured rate grow, so the pipeline roughly doubles every round trip until the link is the limit. A sliding-window rate lagged so far behind that ramp-up took seconds in simulation; per-response samples cover exactly one round trip. Response times include the provider's queueing, so the minimum is the best estimate of RTT the core has on its own.

---

## D-153: Endgame and snubbing details

**Date:** 2026-10-07 · **Status:** Decided

**Context:** D-061 sets the endgame strategies; which chunk to duplicate first, when to cancel, and what to do about a provider that never answers were open.

**Decision:**

- **Endgame picks** the missing chunk with the fewest open requests that the provider has, at random among equals, and never asks one provider twice for a chunk. Requests grow around it like any other, so they shrink as fewer chunks qualify.
- **Cancelling:** when Data arrives, every other open request whose chunks are now all stored is cancelled by resetting its stream. A request that still covers a missing chunk stays open.
- **An `off` endgame** strategy, which never duplicates requests, exists as an experimental contrast.
- **Snubbing:** a provider that sends no Data for 60 s while requests to it are open is snubbing. Its requests are cancelled and become available to others, and until it sends Data again it gets one request at a time, only for chunks no other connected peer has.

**Options for snubbing:** no timeout (requests wait forever); cancel and keep requesting normally; cancel and restrict the provider (chosen).

**Rationale:** Duplicating the least-requested chunks first spreads the extra copies evenly. Restricting a snubbing provider to chunks nobody else has keeps it useful without letting it hold up chunks others could send: without the restriction, a provider that never answers could take the last chunk again after every timeout.

---

## D-154: Upload strategy details

**Date:** 2026-10-07 · **Status:** Decided

**Context:** D-051, D-062 and D-063 name the upload strategies; their behavior between reviews, how PropShare divides upload, and what "round-robin" means for a complete peer were open.

**Decision:**

- **BitTorrent:** at each review (10 s), the 4 interested peers with the highest receive rate over 20 s are served, ranked at random among equal rates, so the slots are filled even before anyone has uploaded. One more interested peer is served optimistically, rotated every 30 s. Between reviews, the strategy only stops serving peers that want nothing more and fills free slots at random, so a newcomer is served without waiting for the next review.
- **PropShare:** serves every interested peer that uploaded to this peer within the rate window, plus 1 optimistic peer rotated every 30 s. Responses are ordered so that each contributor's share of upload follows its share of what this peer received, with 20% of upload kept for optimistic peers (the share Levin et al. use).
- **FairTorrent:** serves every interested peer and answers next the peer with the largest deficit (received minus sent, counting Data being sent).
- **Round-robin (complete peers):** serves every interested peer and answers their requests in turn: the next response goes to the peer whose last one was longest ago.
- A setting chooses whether a complete peer switches to round-robin or keeps its downloading strategy.

**Rationale:** Serving and ordering are the two levers the session gives a strategy: Serving says who may request, and the order of responses divides the upload. Rotating Serving over time, as BitTorrent seeds do, would leave most requesters waiting; answering in turn spreads a complete peer's upload, which rarest-first then spreads further.

---

## D-155: How superseeding works

**Date:** 2026-10-07 · **Status:** Decided (revised by D-223)

**Context:** D-063 has a brand-new bundle's only complete copy offer each chunk to only one peer until every chunk has been sent once. How to "offer" was open.

**Options:** reject requests for chunks already sent to someone; hide chunks from Have and announce them one range at a time (BitTorrent's approach).

**Decision:**

- A session that starts with every chunk superseeds if the setting is on (D-223 adds the `crowd` value). Its Have lists only empty files' chunks, which every Have must include. Have means "chunks the sender has and is willing to provide" (`protocol/messages.md`), so hiding the rest is allowed.
- Each connected peer is offered one range of the configured request size, with a HaveUpdate, preferring chunks not yet sent and not on offer to anyone else. When the peer announces the whole range, it is offered another.
- Requests for chunks not announced to the requester are rejected with `dont_have`.
- Superseeding ends when every chunk has been sent once, or when a connected peer turns out to have every chunk. The rest is then announced to everyone and the complete peer's upload strategy takes over.

**Rationale:** Rejecting requests would waste requesters' round trips, since rarest-first makes many of them pick the same chunks. Offering the next range when the recipient has the last one, rather than when a third peer has it as in BitTorrent, is simpler and still sends each chunk once.

---

## D-156: Connection, peer exchange and lingering details

**Date:** 2026-10-07 · **Status:** Decided (revised by D-224)

**Context:** D-064 to D-066 set the connection limit, lingering and direct-send rules; the protocol limits Peers to 50 records at most once a minute. The details were open.

**Decision:**

- **Idle connections:** a connection is idle when no Data has gone either way for 60 s and no request is open in either direction. At the limit, a new connection replaces the idle one with the highest RTT (unknown counts as highest); without an idle one it is refused with `too_many_connections`. An incomplete peer at its limit with peers waiting to be dialed also drops one idle connection every 30 s.
- **Two complete peers:** the peer that learns, from a Have or HaveUpdate, that the other has every chunk while it has every chunk too closes the connection with `done`. A peer that has just completed doesn't close, so its last HaveUpdate isn't cut off.
- **Peer exchange:** Peers is sent when a connection opens and then every 60 s, with up to 50 of the other connected peers chosen at random, using the addresses from their Hello. At most 1,000 peers are remembered for dialing; a new one replaces a random one that isn't connected or being dialed.
- **Lingering** applies only to a session that downloaded: one that started complete, such as the origin, shares until told to stop. It stops when uploaded ≥ 1.0 × downloaded, or when no connected peer has wanted anything for 10 minutes, unless set to keep sharing. Stopping closes every connection with `done`.
- **Direct sends:** the sender stops once a peer that lacked chunks while connected has every chunk and no connected peer still lacks any.

**Rationale:** Each rule uses only what a session observes and keeps its state bounded. The timing values are starting points for simulation.

---

## D-157: Scenario file fields

**Date:** 2026-10-07 · **Status:** Decided (revised by D-230)

**Context:** D-072 puts a scenario and its experiment in one YAML file; `specs/simulation.md` left the field names open.

**Options:** separate world and experiment files; one file with the world at the top level and the experiment under `experiment` (chosen); a free-form map interpreted by the simulator.

**Decision:**

- One file per scenario. The world sits at the top level: `bundle`, `network` (defaults for every peer's link), `tracker`, `settings` (a profile and overrides, as in D-150), and `peers`, a list of classes with a count, bandwidth, latency, loss, what they start with, a behavior, arrivals and churn. `experiment.vary` maps dotted setting keys to the values to try; every combination is a variant. The simulator's `scenario` module is the source of truth for the fields; `crates/sim/scenarios/` has examples.
- `seeds: N` runs seeds 1 to N. The same seed in every variant gives paired runs (D-073).
- Settings apply in order of increasing specificity: the profile, the scenario's overrides, the variant's values, then the class's overrides, so that a class pinned to a strategy keeps it while the experiment varies the rest.
- Quantities carry units: sizes (`4MiB`, `1GB`), rates in bits (`20Mbps`) or bytes (`2MB/s`), fractions (`0.5%`) and durations as in D-150. A bare number is in the base unit.
- `payload: virtual` (the default) sends sizes only, and the simulator decides whether Data passes verification. `payload: real` gives every peer the bundle's bytes and verifies every Data, for small scenarios.
- Unknown fields, YAML 1.1 booleans such as `off` (read as strings), duplicate keys and unusable values are errors that name the field and line.

**Rationale:** One file keeps a scenario and the variants compared in it together. Ordering overrides by specificity is the rule settings files usually follow. Virtual payloads are what make thousand-peer swarms affordable (D-148); real ones check that the simulator and the core agree on actual bytes.

---

## D-158: Simulated network details

**Date:** 2026-10-07 · **Status:** Decided

**Context:** D-069 sets a message-level network with fair sharing, but not how capacity is shared, how loss and ordering work, or what counts as a message.

**Options for sharing:** global max-min fairness, recomputed over every flow at each change; processor sharing at each end, recomputed only for the flows at the two ends of the change (chosen); a fixed rate per connection.

**Decision:**

- Each peer has an access link: upload and download capacity, a one-way latency to the core of the network, and a loss rate. Two peers are as far apart as their two latencies added; a message between them is lost if either link loses it.
- Data and Manifest responses are flows. A flow's rate is the smaller of its sender's upload divided by its sender's flows and its receiver's download divided by its receiver's flows. It arrives one latency after its last byte leaves.
- Other messages are small and sent ahead of bulk data, as a transport prioritizes control streams: each takes its size ÷ the narrower link, then the latency, without slowing flows. They arrive in the order sent on their connection.
- Loss never drops a message, since QUIC is reliable: each loss resends it after one round trip plus 25 ms, QUIC's default maximum ACK delay.
- Message sizes are their protocol encodings. Data's size is computed from the manifest: its framing, proof and payload.
- A dial takes one round trip; a dial to a peer that isn't there fails after 5 s. Closing a connection stops its transfers both ways.

**Rationale:** Global max-min fairness would recompute thousands of flows on every response in a large swarm. Processor sharing at both ends costs work proportional to two peers' connections, and access links, where it applies, are where home connections are limited. It can leave some capacity unused when a flow is limited at its other end, which is acceptable for comparing strategies and can be revisited when the packet-level mode (D-069) validates it.

---

## D-159: Simulated peers and runs

**Date:** 2026-10-07 · **Status:** Decided

**Context:** D-070 lists what the first simulator models; how misbehaving peers, the tracker and a run's randomness and end work was open.

**Decision:**

- **Randomness:** each run has one seed. Separate ChaCha8 streams serve the world (arrival times, session lengths and each session's seed), the network (loss) and the tracker. The world is drawn before the run, the same way whatever the settings, so every variant sees the same arrivals and churn for a seed.
- **Arrivals:** `at-once`, `steady` (one per interval) or `flash-crowd` (exponential delays after a start). **Churn:** each peer stays for an exponential time with the given mean, then shuts down. A peer whose session stops sharing (D-156) leaves too.
- **Free riders** answer every chunk Request with a `busy` Reject, so they never upload. **Bad-data peers** send Data that always fails verification (with real payloads, a flipped byte).
- **Tracker:** a peer announces on arrival and every interval, and gets up to the configured number of present peers chosen at random. Leaving peers are forgotten at once.
- **A run ends** when every peer that started without the bundle has completed or left, or at the scenario's duration.

**Rationale:** Drawing the world up front makes variants comparable seed by seed, which is what D-073's paired comparisons need. Each behavior is the simplest that exercises the matching defense: snubbing and reciprocity for free riders, banning for bad data.

---

## D-160: The `tempest sim` command

**Date:** 2026-10-07 · **Status:** Decided (revised by D-214: the command is `tempest-sim run`)

**Context:** D-124 lists the CLI's verbs but has none for the simulator, which D-083 builds into the `tempest` binary.

**Options:** a separate `tempest-sim` binary; `tempest sim run <scenario.yaml>` (chosen); `tempest simulate <scenario.yaml>`.

**Decision:** `tempest sim run <scenario.yaml>` runs every variant with each of the scenario's seeds, several runs at once, and prints a summary per variant. `--seeds N` and `--seed S` override the seeds; `--jobs` sets how many runs go at once. Further simulator commands, such as viewing results, become siblings under `tempest sim`.

**Rationale:** A noun group keeps simulation commands apart from the client's verbs, and leaves room for more without crowding the top level.

---

## D-161: Results database layout

**Date:** 2026-10-07 · **Status:** Decided

**Context:** D-075 puts experiments, runs, summary metrics and event logs in one DuckDB file, but not how they are laid out or written.

**Options for event logs:** one row per log entry with typed columns and a readable description (chosen); one row per entry with the decision event as JSON; only digests and summaries, with logs written on request.

**Decision:**

- Tables for experiments, variants, runs, peers, traffic per message type and events. The simulator's `results` module is the source of truth for the columns; `specs/simulation.md` describes what each table holds.
- An experiment records the scenario as parsed, the scenario file's path and SHA-256, and the git commit the simulator was built from (read at build time; uncommitted changes aren't detected). Each variant records the scenario as it runs it: its varied values folded into the overrides. A run points to its variant, so its resolved scenario is stored once, not once per seed.
- IDs continue from those already in the file, so one file collects many experiments. Run IDs are fixed by variant and seed, not by which run finishes first.
- Each event-log entry becomes one row: time, peer, kind, the other peer by index, connection, message, chunk range or count, bytes, a short reason name and, for decision events, the readable description. The digest of a run's log is computed over these rows, so the digest checks exactly what is stored.
- Rows are written while the run goes, in batches through DuckDB's appender, so no run's log has to fit in memory.
- The database records its schema version; a build refuses a file with another version.
- `tempest sim run` writes to `tempest-sim.duckdb` by default. `--db` names another file, `--no-db` skips recording, `--no-events` keeps summaries only, and `--digests` writes each run's log digest to a file for comparison.
- DuckDB sits behind the simulator's default `results` feature, so builds that only run scenarios, such as fuzzing, don't compile it.

**Rationale:** Typed columns make the common questions (who sent what to whom, why a chunk was requested) plain SQL, and DuckDB compresses repeated names well. Writing during the run keeps memory flat for large swarms. Storing the resolved scenario per variant avoids duplicating it across seeds.

---

## D-162: Headline metric definitions

**Date:** 2026-10-07 · **Status:** Decided

**Context:** `specs/simulation.md` lists the headline metrics without defining them precisely.

**Decision:**

- Completion time, time to first peer and time to first byte are measured from a peer's arrival, for peers that started without the bundle: to its Completed event, its first opened connection, and its first verified payload. Percentiles use the nearest-rank method.
- **Upload utilization** is the payload bytes peers uploaded over the upload capacity of every peer with a limited upload, for as long as each was present.
- **Fairness** is Jain's index over the upload/download ratios of the peers that downloaded anything, so free riders lower it.
- **Overhead** is every byte sent that isn't chunk payload, by message type; for Data, its framing and proofs.
- **Endgame duplicate bytes** are payload bytes received of chunks the peer already had.
- The availability of the rarest chunk over time is not yet measured; it needs periodic sampling, which can come with the results UI.

**Rationale:** Measuring from arrival makes peers that arrive at different times comparable. Counting capacity only while a peer is present keeps churned peers from diluting utilization.

---

## D-163: Golden baselines and the determinism check

**Date:** 2026-10-07 · **Status:** Decided; the golden scenario set and the long check are amended by D-171

**Context:** D-077 and D-132 call for golden scenarios against saved baselines, with a setting for longer runs; `specs/simulation.md` calls for a CI check that the same seed gives the same event log.

**Options for baselines:** exact digests (any change fails); aggregate metrics with tolerances (chosen); statistical tests across many seeds.

**Decision:**

- A baseline saves a scenario's headline metrics for each seed. A check runs the first few seeds and compares the mean of each metric with the baseline's mean over the same seeds, so both sides see the same worlds (D-073). Each metric has a tolerance (the `baseline` module's table): within it the check passes; a larger regression fails, and a larger improvement asks for the baseline to be updated.
- The first golden scenario is small and varied. By default its test runs 5 seeds in about a second; an environment variable runs up to all saved seeds, and another rewrites the baseline.
- CI builds the release binary, runs the 1,000-peer swarm and the golden scenario twice each in separate processes, the second time on one thread, and fails if the log digests differ. The same job runs the slow tests, including the 1,000-peer swarm into DuckDB, and the golden baseline over every saved seed.

**Rationale:** Runs are deterministic, so exact digests would fail on every deliberate change to a strategy; tolerances flag what D-077 cares about, significant drops in speed or fairness. Paired seeds keep the comparison fair with few runs. Separate processes catch nondeterminism that one process would hide, such as hash-map ordering seeded per process.

---

## D-164: An unreproduced failure of the 1,000-peer DuckDB test

**Date:** 2026-10-07 · **Status:** Decided

**Context:** One run of `cargo test --release -p tempest-sim -- --include-ignored` failed in `results.rs` with the panic message lost; reruns passed.

**Investigation:** The full command passed 20 times in a row, and the `results` binary passed 20 times more with four copies running at once to add load. Nothing in `tempest-sim` or `tempest-core` uses hash-map iteration order, wall clocks or global randomness. Run IDs are fixed by position, so thread scheduling doesn't change what lands in the database. The tests share no files: the swarm test uses its own temporary directory and the others use in-memory databases.

**Decision:** No code change. The cause is unknown, most likely machine load or a one-off outside the test code. If CI shows the failure again, keep the full panic message first: a digest mismatch would mean the recorder perturbs the engine; a DuckDB error would point to shared state.

**Rationale:** A speculative change would hide the cause without confirming it. The determinism job in CI already compares digests across separate processes and thread counts.

---

## D-165: Simulated routers, relays, disks and sessions

**Date:** 2026-10-07 · **Status:** Decided

**Context:** D-070 asks the simulator to model NAT reachability with relays and hole punching, disk and hashing speed, and churn. D-159 covered only exponential sessions and three arrival patterns.

**Options for routers:** a NAT type per peer (full cone, restricted, symmetric) with per-pair rules; one chance per peer that its router is punchable, a pair punching when both are or when the dialer is reachable (chosen); one success rate per pair. **For the hole punch:** a new direct connection replacing the relayed one, as the protocol does; the same connection switching its path at the time the direct one would open (chosen). **For relays:** relays as network nodes whose capacity all relayed flows share; a detour latency and a per-connection forwarding cap (chosen).

**Decision:**

- A peer class may sit behind a router (`nat`). Nobody can dial such a peer directly. With a home relay (the default) it is dialed through the relay; without one, dials to it fail after the dial timeout, though it can still dial out.
- A relayed connection goes direct when the dialer is directly reachable (it is dialed back) or when both routers are punchable, each with the class's chance (default 85%, so about 72% of such pairs punch, within the 70–90% the protocol cites). The switch happens one relayed and one direct round trip after the connection opens: the Candidates exchange, then the direct handshake. Both sessions then measure the direct round trip, and the switch is logged as `upgraded` for both peers.
- Relays (`relay` in the scenario) add their latency twice to each one-way trip and forward each direction of a relayed connection at up to a set rate (default 10 Mbps), shared by that connection's transfers.
- A peer hashes each Data it receives at its `hashing` rate, then writes it at its `disk` rate unless it failed verification; the two stages overlap. Its session hears of the Data once it is stored. Both default to unlimited.
- Sessions may follow a Weibull distribution (`churn.shape`; 1, the default, is exponential), and arrivals may be a Poisson process (`pattern: poisson`).
- Whether each peer's router is punchable comes from its own random stream, so scenarios without routers replay exactly as before.

**Rationale:** Measurement studies explain most punching failures by symmetric NATs on either side, which a per-peer punchable chance captures without modeling NAT behavior in detail. Keeping the connection when it upgrades avoids modeling duplicate-connection resolution and re-requests, whose cost is a round trip, small next to a transfer. A cap per connection matches the protocol's allowance for relays to limit bandwidth; shared relay capacity can come with the packet-level mode. Delivering Data only once stored makes slow disks throttle requests, as a client's bounded write queue would. Weibull sessions with shape below 1 fit measured swarms better than exponential ones.

---

## D-166: Simulated strategic clients

**Date:** 2026-10-07 · **Status:** Decided

**Context:** D-070 asks the simulator to model strategic clients (BitTyrant-style), so that upload strategies can be tested against peers that game them.

**Options for where it lives:** a core upload strategy named in the settings; a core strategy the settings can't name; a strategy in the simulator, installed through a session hook (chosen). **For estimating what an unknown peer would give:** its announced download rate split among BitTorrent's slots, as BitTyrant does; nothing, so unknown peers get what known ones leave (chosen). **For prices:** absolute rates against an estimated capacity; shares of the capacity (chosen).

**Decision:**

- A peer class may have `behavior: strategic`. Such peers keep twice the configured connections and, while downloading, use the simulator's `strategic` upload strategy. Once complete they follow `strategies.when_complete` like anyone else.
- The strategy follows Piatek et al. Each peer has a price, in equal shares of upload capacity among BitTorrent's slots, starting at one. Every review interval it serves the interested peers with the best ratio of what they last gave to their price, until the prices fill the slot count; between reviews it fills what peers that stop wanting leave. Responses are ordered so each served peer gets upload in proportion to its price. A served peer that doesn't serve back costs 20% more next time; one that serves back for three reviews in a row, 10% less. These constants are BitTyrant's.
- The session gains a hook to replace its downloading upload strategy, and upload strategies now see whether each peer is serving them.

**Rationale:** The strategy exists to attack the real ones, so it should not be selectable in the real client. Announce-based estimates were tried first: in the simulator, a peer's download comes mostly from seeds and fast peers, so they overrated every unknown peer, and the strategic client spread its upload over peers that never paid back. Serving everyone until the capacity was measured, or leaving slots empty between reviews, did similar harm at the start of a download, when BitTorrent peers settle whom they reciprocate; prices in shares avoid needing the capacity at all. In a swarm with uneven upload capacities where peers leave once complete, strategic peers then finish a 256 MiB download about 15% sooner than they would as BitTorrent peers on the same seeds (`crates/sim/tests/strategic.rs`). For downloads of a minute or less the prices have no time to settle, and the difference is not meaningful.

---

## D-167: Statistics across seeds

**Date:** 2026-10-07 · **Status:** Decided

**Context:** D-073 asks for medians with confidence intervals, paired seeds and meaningful-difference reporting.

**Options for intervals:** normal approximation (assumes a symmetric distribution, which completion times rarely have); exact order-statistic intervals for the median (no paired-difference counterpart that is as simple); the percentile bootstrap (chosen). **For comparisons:** difference of medians; median of per-seed differences (chosen), which uses the pairing. **For "meaningful":** interval excludes zero; that and a minimum effect size (chosen).

**Decision:**

- Each run gives one value per headline metric. A variant's median across seeds comes with a 95% percentile-bootstrap interval from 10,000 resamples.
- Another variant is compared with a reference variant seed by seed: the median of the per-seed differences, with its bootstrap interval, and that median relative to the reference's.
- A difference is meaningful when at least 5 seeds are paired, the interval excludes zero and the effect is at least 2% of the reference's median. It is reported as better or worse according to the metric's direction.
- The resampler is seeded with a constant, afresh for each interval, so reports are reproducible and don't depend on what else was estimated.
- `tempest sim run` prints the comparison after the summary, with the reference variant chosen by `--reference` (default: the first). The comparison is derived from the results database's `runs` table and these constants, so the database stores nothing more.

**Rationale:** The bootstrap needs no assumptions about the shape of the distribution and works the same for medians and paired differences. Pairing removes the world-to-world variation that dominates between seeds. With fewer than 5 pairs a percentile bootstrap can't produce a useful interval. A minimum effect keeps consistent but negligible differences, common with deterministic paired runs, from being called meaningful; 2% is below what a user would notice and above the noise of most metrics.

---

## D-168: Availability of the rarest chunk

**Date:** 2026-10-07 · **Status:** Decided

**Context:** D-162 left the availability of the rarest chunk unmeasured. Validating rarest-first (D-078) needs it.

**Options for measuring:** tracking the minimum as chunks spread; sampling every chunk's count periodically (chosen). **For the headline value:** the minimum over the run; the mean over samples (chosen); the count as a fraction of the peers present.

**Decision:**

- The simulator keeps, for each chunk, how many present peers hold it. Every `sample_interval` of simulated time (a scenario setting, default 10 s) until the run ends, it records the peers present, the count of the rarest chunk and the mean count. A sample sees everything that happened before its time.
- The headline metric **rarest copies** is the mean of the rarest chunk's count over a run's samples. The results database keeps it with each run, and every sample in an `availability` table (schema version 2).
- Comparisons across seeds (D-167) also report it, and the fraction of downloaders that completed.

**Rationale:** Sampling costs one pass over the chunks per sample and needs no bookkeeping beyond the counts. The mean over time separates a swarm that keeps every chunk alive from one where a chunk lingers at one copy or disappears, which a single minimum can't. Counts rather than fractions keep the value readable; the samples hold the peers present for anyone who wants fractions.

---

## D-169: A limit on dials in progress

**Date:** 2026-10-07 · **Status:** Decided

**Context:** In the first validation flash crowd, 100 peers arriving together each dialed up to 50 peers from the tracker. The first 50 filled each other's connection limits; the others filled theirs with dials that were then refused, and meanwhile refused the peers dialing them. Half the swarm waited for the 60 s redial delay before getting a byte.

**Options:** a shorter redial delay after a `too_many_connections` refusal (peers retrying at the same moments collide again); counting only open connections against the limit (lets a peer exceed it); a cap on connections being dialed or dialed and not yet open, like libtorrent's half-open limit (chosen).

**Decision:** A session dials at most `connections.max_dialing` peers at once (default 8), counting dials in progress and dialed connections not yet open. It dials more as those open or fail. The rest of the connection limit stays free for peers that dial it.

**Rationale:** The cap leaves room for incoming connections whatever the timing, and costs a newcomer only a few round trips to fill its connections. In a flash crowd of 100 peers fetching 16 MiB, the last peer then finished in 30 s instead of 78 s.

---

## D-170: Choking that withholds from peers it doesn't serve

**Date:** 2026-10-07 · **Status:** Decided

**Context:** Validation against Legout et al. (D-078) found free riders downloading as fast as contributors under the BitTorrent strategy (D-154). Two leaks caused it: requests accepted while a peer was served were still answered after it stopped being served, and slots freed between reviews went to waiting peers at random, free riders included, which happened far more often than reviews.

**Options for waiting requests:** answer them (as before); refuse them with `busy` when the peer stops being served, as BitTorrent's choke discards pending requests (chosen). **For slots freed between reviews:** fill them at random (as before); by receive rate, ties at random, as BitTorrent's rechoke does (chosen).

**Decision:**

- When a provider stops serving a peer, it refuses that peer's requests it hasn't started answering with Reject (`busy`). `protocol/messages.md` (Serving) now says a provider MAY do this.
- Between reviews, the BitTorrent strategy fills free slots with the waiting peers that upload to this peer fastest, ties at random. This amends D-154, which filled them at random.

**Rationale:** Serving is how a provider says whom it will answer; answering after it said no undoes the decision. Ordering by rate keeps reciprocators ahead of peers that give nothing, while newcomers still get free slots when no reciprocator is waiting, and the optimistic slot. With both changes, free riders take about 15% longer than home peers in the free-rider validation scenario, against no difference under the cooperative strategy. Strategic clients (D-166) still finish meaningfully sooner than BitTorrent peers, by about 13%, but with more spread across seeds: their test now runs 20 seeds.

---

## D-171: A set of golden scenarios, checked quickly or seed by seed

**Date:** 2026-10-07 · **Status:** Decided

**Context:** D-163 had one golden scenario, predating the model of routers, relays, disks, Weibull sessions and strategic clients (D-165, D-166). D-132 asks for a quick check by default and a longer, more certain one.

**Options for the long check:** the quick check's means over more seeds; the paired comparison of D-167, whose 2% minimum effect would fail small deliberate changes; a paired comparison with each metric's tolerance as its minimum effect (chosen). **For running it:** an environment variable read by the test, as before; a `tempest sim baseline` command (chosen), which also runs in release and rewrites the baselines.

**Decision:**

- Golden scenarios are the scenario files in one directory, each with one variant and named after its file. Each one's per-seed baseline is the JSON file of the same name in the baselines directory. A generic test finds them all. They cover a varied swarm, peers behind routers, heavy-tailed sessions with bad data, strategic clients and free riders, and a crowd arriving at once.
- The quick check runs the first few seeds of each scenario and compares means with D-163's tolerances; it runs in the normal test job. The long check runs every saved seed and compares seed by seed: a metric regresses when the median per-seed change is worse than its tolerance and its bootstrap interval excludes no change. CI's simulation job runs it with the release binary. Update mode rewrites every baseline from every saved seed.
- The rarest chunk's availability (D-168) is now a checked metric.
- Tests cut every peer's upload to 60% and require each golden scenario to fail both checks on completion time.

**Rationale:** One scenario per model feature keeps a regression in one feature from hiding in averages over others, and small swarms keep the quick check to a few seconds in debug. Medians of paired changes ignore one unusual seed, and the interval keeps noise from failing the check, while tolerances, not mere significance, decide what counts as a regression, as in D-163. A command rather than an environment variable makes the long check and updates a single, discoverable step between milestones.

---

## D-172: QUIC endpoint, TLS and identity file details

**Date:** 2026-10-07 · **Status:** Decided

**Context:** M4 needs concrete choices for the parts of `protocol/connections.md` it leaves to implementations: how the key is encoded and stored, how rustls and quinn are set up, and the transport's timers.

**Options for the key file:** raw seed bytes; hex text; a PKCS#8 PEM file, as `openssl genpkey -algorithm ed25519` writes (chosen). **For the crypto provider:** aws-lc-rs (rustls's default, needs a C toolchain and CMake on some platforms); ring (chosen). **For sockets:** one dual-stack socket; one IPv4 socket and one IPv6-only socket (chosen). **For an unexpected peer ID:** close after the handshake; fail the handshake in the certificate verifier (chosen).

**Decision:**

- The identity is a PKCS#8 PEM file created with owner-only permissions. Loading it never replaces a file that fails to parse.
- A raw public key must be exactly the DER form of an Ed25519 SubjectPublicKeyInfo (RFC 8410); anything else fails the handshake. `protocol/connections.md` now spells out that form.
- TLS uses rustls on ring, TLS 1.3 only, Ed25519 signatures only. Session resumption and 0-RTT are off. SNI is not sent.
- An endpoint binds one IPv4 socket and one IPv6-only socket. If the IPv6 socket can't be bound for a reason other than its port being in use, the endpoint carries on with IPv4 only.
- A dialing peer that expects a peer ID rejects any other key in the certificate verifier, before presenting its own key.
- Initial and minimum MTU are 1,200 bytes with path MTU discovery on. Peers may open `max_open_requests` + 1 bidirectional streams and no unidirectional ones. Connections time out after 30 s of silence and send keep-alives every 10 s.

**Rationale:** PEM PKCS#8 is the standard, inspectable format for keys. ring builds everywhere without extra tools. Separate sockets per family make "one socket per address family" literal and avoid platforms without dual-stack sockets. Without resumption every connection proves both keys, so a connection's peer ID is always from this handshake. Failing in the verifier closes the connection with a TLS alert, the earliest possible point. Keep-alives every 10 s stay inside routers' usual UDP mapping timeouts at negligible cost.

---

## D-173: Bundle storage details

**Date:** 2026-10-07 · **Status:** Decided

**Context:** D-086 fixes positioned I/O on a thread pool with batched syncs. M4 needs the rest: which task owns the files, how proofs are built, how syncs are batched, how a download is laid out, and what happens to files already on disk.

**Options for ownership:** one disk task for every bundle; one storage task per bundle (chosen). **For proofs:** rebuild a range's proof from disk on each request; keep each file's chunk tree in memory (chosen), learning a download's nodes from the leaves and proofs of the Data it stores. **For verification:** the bundle task verifies, then storage writes; storage verifies on the blocking pool before writing (chosen). **For resume:** chunk-level, which needs verified chunks recorded across runs; file-level, keeping existing files whose contents hash to their roots (chosen for now); none.

**Decision:**

- Each bundle has a storage task. It owns the open files, the known nodes of each file's chunk tree and the set of chunks present, and runs reads, writes and syncs on tokio's blocking pool, a configurable number at a time. Its command channel is bounded.
- Sharing opens the files read-only and hashes each one, refusing files that are missing or no longer match the manifest.
- Data is verified against the manifest before it is written. Data for chunks already present is verified but not written again.
- Writes are synced once a configurable number of bytes or a configurable time has passed since the last sync, on every flush and close, and when the last chunk is stored. The write that completes a download returns after that sync and after the files' modification times are restored.
- A download creates its directories, its files at full size with their executable bits, and then its symlinks. It never follows or replaces anything else already in the destination: files are opened without following symlinks, and a symlink, or anything else in the way of an entry, is an error.
- Resume is per file: an existing file of the right size whose contents hash to its root counts as complete, and any other existing file is resized and downloaded again. Chunk-level resume waits for the client state store (D-082).
- Encoded manifests are saved by writing a temporary file, syncing it and renaming it over the old one, and checked against the bundle ID when loaded.

**Rationale:** A task per bundle keeps each task's state small and lets the bundle task own its storage handle, so a slow disk under one bundle doesn't stall another's reads. A tree in memory costs about 0.4% of a bundle's size, and makes every proof a lookup. A downloader learns enough to prove any range it holds, because the Data that brought a range also brought the siblings of its ancestors. Verifying in storage puts hashing on the blocking pool and makes unverified writes impossible. Restoring modification times only at completion keeps later writes from changing them. Per-file resume is simple and needs no stored state, while trusting a partial file's chunks needs a record of which ones were verified.

---

## D-174: Production driver details

**Date:** 2026-10-07 · **Status:** Decided

**Context:** D-068 and D-085 fix the shape of the engine: sans-IO sessions driven by actors over bounded channels. M4 needs the details: how tasks are split, how they avoid deadlock, how incoming connections find their bundle, and what the driver does about problems only it can see.

**Options for routing:** one endpoint per bundle; one endpoint per installation, routing each accepted connection by the bundle ID in its Hello (chosen). **For channel cycles:** await every send; let the bundle task await nothing but storage, handing work out with `try_send` and oneshots (chosen). **For decision events:** a bounded mpsc that blocks the session when full; a broadcast channel whose slow subscribers miss events and are told how many (chosen). **For violations only the driver sees** (frames that don't decode, request streams that end without a message): close the connection in the driver and report it lost; a session method that closes it like any other violation (chosen).

**Decision:**

- A node task accepts connections and owns the table of bundles. A routing task per incoming connection reads Hello and hands the connection to that bundle's task, or closes it with `unknown_bundle`.
- A bundle task owns the session and its storage. Connection tasks, request-stream tasks and dial tasks report to it through its bounded inbox, awaiting a send, so a busy bundle slows reading from the network. The bundle task never waits on them: control messages go to a connection's bounded queue with `try_send`, and answers and cancellations go through oneshots.
- A peer whose control-stream queue fills, because it isn't reading, is disconnected with `done` and reported to the session as lost.
- Cancelling a request stops the receiving side and resets the sending side. A provider that sees its response stream stopped resets it and reports the request reset.
- Data that storage can't read after the session chose to send it is answered with Reject, and reported to the session as a reset request so the upload slot frees without counting bytes as sent.
- A download's storage is laid out when the session stores the manifest. A storage failure other than failed verification stops the bundle with that error.
- `Session::handle_violation` lets the driver report protocol violations the session can't see.
- Decision events go out on a broadcast channel, and a watch channel carries a progress summary.
- Each session's random seed comes from the operating system.

**Rationale:** One endpoint per installation is what links advertise (D-126), and Hello is the first thing that names a bundle. Keeping the bundle task's only waits on storage rules out cycles of full channels without unbounded queues. A peer that doesn't read control messages is broken or hostile, and any bound on queued messages has to end somewhere. Logging and progress displays prefer a gap they are told about to a stalled transfer. Reporting driver-seen violations through the session keeps one place that closes connections and records why.

---

## D-175: `tempest share` and `tempest get` before the engine

**Date:** 2026-10-07 · **Status:** Decided (revised by D-224, D-228)

**Context:** M4 runs `share` and `get` (D-124) as standalone processes, before the engine (D-079) exists. They need an identity file location, the addresses a link advertises, where `get` writes, and when it gives up or exits.

**Options for the identity location:** next to the binary; the per-user config directory; the per-user data directory (chosen), overridable by flag or environment variable for tests and multiple identities. **For finding that directory:** the `dirs` crate (its dependency `option-ext` is MPL-2.0, outside the license allowlist); a few lines following the same platform conventions (chosen). **For advertised addresses:** the bound addresses only; every interface address including loopback; non-loopback interface addresses for wildcard binds, and exactly the bound address otherwise (chosen). **For where `get` writes:** straight into `--output`; a new folder named after the bundle inside it (chosen), as `protocol/content.md` suggests. **For an existing folder of that name:** write into it, resuming per file (D-173); refuse (chosen). **For `get`'s exit:** linger per D-065; exit as soon as every chunk is stored and synced (chosen).

**Decision:**

- The identity is `identity.pem` in a `Tempest` (macOS, Windows) or `tempest` (elsewhere) folder in the per-user data directory: `LOCALAPPDATA` on Windows, `~/Library/Application Support` on macOS, and `XDG_DATA_HOME` or `~/.local/share` elsewhere. `--identity` or `TEMPEST_IDENTITY` replaces it. `share` and `get` use the same identity.
- A socket bound to the wildcard address advertises the machine's interface addresses in its family, without loopback or IPv6 link-local addresses (an address can't carry their scope ID). A socket bound to a specific address, loopback included, advertises that address. A link with no addresses gets a warning.
- `share` binds UDP 4773 on both families by default; `--port` and `--bind` change that. It serves until Ctrl-C.
- `get` binds a port the system picks, since nobody needs to dial it before it exits. It needs a link with a peer and at least one address until trackers (M5) and relays (M6) arrive. It writes into a new folder named after the bundle inside `--output` (default: the current directory), and fails if that folder already exists.
- `get` exits 0 once every chunk is stored and synced, without lingering, and prints the folder's path. It fails if nothing progresses for `--timeout` seconds (default 30), naming the addresses it tried and the last reason a connection failed or closed.
- On exit, both wait at most a second for their Goodbyes to drain, since a dial still handshaking with an unreachable address takes several seconds to.

**Rationale:** The identity is state, not configuration, and outlives any single bundle. Advertising what peers on other machines can reach keeps links short and useful, while an explicit loopback bind is how tests and single-machine setups ask for loopback. A fresh folder per download can never overwrite unrelated files that happen to share a name with a bundle's entries; resuming into existing folders can come back with the engine's state store (D-082). A one-shot `get` that lingers would surprise someone using it in a script; lingering is the engine's job.

---

## D-176: Tracker server details

**Date:** 2026-10-07 · **Status:** Decided

**Context:** `protocol/trackers.md` defines the peer-list API but leaves the server's own policies open: transport security, rate limits, memory bounds, errors outside the listed codes, and how the observed IP is found behind a proxy.

**Options for HTTPS:** built into the tracker with its own certificate settings; plain HTTP, with HTTPS from a reverse proxy in front (chosen). **For rate limiting:** none; per IP; per peer, on `update` only (chosen). **For bounding memory:** a cap per swarm; one cap across all swarms (chosen). **For shared state:** an actor owning the swarms; a mutex around plain data (chosen).

**Decision:**

- The tracker serves plain HTTP on TCP 4777 (D-126). HTTPS is a reverse proxy's job. With `--behind-proxy`, the observed IP is the last `X-Forwarded-For` entry, which the proxy appends; otherwise it is the TCP peer address. IPv4-mapped addresses are read as IPv4.
- The default interval is 300 seconds. An `update` sooner than the minimum interval (default 60 seconds) after the same peer's last announce is refused with `rate_limited` and `retry_after_secs`. `started`, `completed` and `stopped` are never refused as early, since each reports a change.
- `num_want` is capped at 200. An announce with more than 16 `addrs`, or with port 0, is `invalid_request`. Bodies are limited to 16 KiB; a larger one is refused with status 413 and `invalid_request`.
- At most 1,000,000 peers are listed across all swarms. Beyond that, new peers are refused with `unavailable` and a 60-second `retry_after_secs`; listed peers can still announce.
- A peer's listed addresses are its observed address followed by its `addrs`, without duplicates. A later announce replaces them, so anyone can overwrite a listing by forging its peer ID; as `protocol/trackers.md` notes, that only wastes connection attempts.
- Unknown paths and methods answer 404 `not_found` and 405 `method_not_allowed`, in the same JSON error shape. Every refusal with `retry_after_secs` also sets `Retry-After`.
- A private tracker serves only the bundles given with `--allow`, refusing others with `bundle_not_allowed`. Recommended relays come from `--relay`.
- The swarms are plain data behind a mutex, taking the time and observed IP as arguments so the rules are tested without a server. A sweep every minute (or every interval, if shorter) frees silent peers and empty swarms. The random choice of listed peers is seeded from the operating system.

**Rationale:** A reverse proxy is the usual way to give an HTTP service a certificate, and keeps certificate handling out of the tracker. Only `update` has a schedule a client can break; the other events are rare and each changes what the tracker knows. One global cap bounds memory however announces are spread across bundles. Every announce takes the lock for microseconds, so an actor would add complexity for no gain.

---

## D-177: Tracker client details

**Date:** 2026-10-07 · **Status:** Decided

**Context:** Clients announce to trackers (`protocol/trackers.md`) from `tempest-net`. The spec fixes the events and the duty to honor `retry_after_secs`, but not the HTTP stack, back-off, limits or which addresses to announce.

**Options for HTTP:** hyper directly; reqwest (chosen), with rustls on ring and the platform's certificate verifier, matching the peer TLS stack. **For the announce loop:** one task per tracker per bundle (chosen); one task per tracker serving every bundle.

**Decision:**

- `TrackerClient` makes requests with a 30-second timeout in the CLI and reads at most 1 MiB of response. HTTP and HTTPS URLs are both accepted.
- An `Announcer` per bundle and tracker sends `started` (with `complete` from the bundle's current status), `completed` as soon as the bundle completes, `update` every interval, and `stopped` when the bundle stops or the announcer is stopped or dropped. `stopped` is skipped if the tracker never accepted `started`, or while the tracker's `retry_after_secs` is still running.
- A failed announce is retried after `retry_after_secs` if given, otherwise after 15 seconds doubling up to 30 minutes. The event that failed is the one retried. A tracker interval below 30 seconds is raised to 30.
- Only addresses that may be reachable from the internet go in `addrs`: no loopback, private, link-local, shared (100.64.0.0/10), unique local or documentation addresses. The tracker adds the observed address itself.
- The peers in each response are handed to the bundle's session, as a link's peers are. The announcer's state (accepted announces, swarm counts, last error) is published on a watch channel for UIs.
- `tempest share --tracker URL` names the tracker in the link and announces to it. `tempest get` asks every tracker the link names, plus any given with `--tracker`, and needs either a peer with an address or a tracker.

**Rationale:** reqwest is the standard async HTTP client and lets the TLS configuration match the rest of Tempest's. A task per bundle and tracker keeps each announce schedule independent and easy to follow; trackers see the same requests either way. Announcing only public addresses follows the protocol's advice not to leak local network details.

---

## D-178: LAN discovery details

**Date:** 2026-10-07 · **Status:** Decided (revised by D-227)

**Context:** `protocol/discovery.md` fixes LAN discovery's messages, groups and reply rate, but leaves open which interfaces to use, which socket to send from, what a query reveals, and how heard-of peers reach downloads.

**Options for interfaces:** the system's default multicast interface; every interface; the interfaces of the addresses the endpoint advertises (chosen). **For sending:** from the shared listening socket; from the QUIC socket (quinn owns it); from a socket per interface bound to its address on a port of its own (chosen). **For queries:** only prompt replies; also make the querier known (chosen). **For delivery:** every heard-of peer to every bundle; to downloads only, until they complete (chosen).

**Decision:**

- LAN discovery announces on the interfaces of the addresses the endpoint advertises (D-175): every non-loopback interface for a wildcard bind, and exactly the bound address otherwise, loopback included. Each gets a sending socket bound to that address, so a datagram's source is an address the endpoint is reachable at and replies to queries come back to it. A listening socket per family on the group port, with address and port reuse, joins the group on each interface.
- It announces and queries when it starts, announces every 60 seconds, and queries when a download is watched, at most once a second.
- A query reveals its sender as an announce does. Senders with link-local IPv6 source addresses are answered but not dialed, since an address can't carry their scope ID.
- Peers heard of are handed to watched downloads, never to complete bundles. They are remembered for 3 minutes after last heard, up to 1,024 peers and 8 addresses each, so a download that starts later gets them at once.
- `tempest share` and `tempest get` run LAN discovery unless `--no-lan` is given, and `--lan-port` changes the group port. `get` accepts a link with only a bundle ID while LAN discovery runs. If it can't start, they warn and carry on.

**Rationale:** Announcing only where the endpoint is reachable keeps every heard-of address dialable. A socket bound per interface follows the protocol's advice to query from a unique port, and needs no per-packet source selection. A querier is a peer on the LAN like any other, and is likely downloading something. Seeds don't dial, so only downloads need peers.

---

## D-179: Chunk-level resume

**Date:** 2026-10-07 · **Status:** Decided

**Context:** D-082 keeps verified chunks for fast resume, and D-173 left chunk-level resume for the state store. A downloader can only serve a chunk it can prove, and the proof hashes it learned came with the Data it stored, so a list of chunks alone can't be served after a restart without hashing again.

**Options for what is saved:** the chunks only, rebuilding proofs by hashing on resume; the chunks plus each file's known tree nodes (chosen). **For trusting the saved chunks:** always; while each file's size and modification time are unchanged since the save, checking the saved chunks against their saved leaf hashes otherwise (chosen); never. **For when it is saved:** after every write; every 30 seconds while downloading and whenever storage closes (chosen).

**Decision:**

- Storage produces a checkpoint after syncing: for each file, its size and modification time, the chunks stored, and the known tree nodes that can't be computed from known children. The engine saves it in the state store, encoded with the wire primitives and a version byte.
- Resuming rebuilds each file's tree from the saved nodes and accepts it only if it hashes to the manifest's root and knows a leaf for every saved chunk; otherwise the file is opened as a fresh download would be. An unchanged file's saved chunks are trusted. A changed file of the right size keeps the saved chunks whose contents still match their saved leaf hashes; one of the wrong size is resized and starts over.
- A shared or completed bundle resumes the same way, so restarting the engine hashes nothing that hasn't changed.

**Rationale:** Saving the tree costs about 0.2% of the bundle per save and keeps every stored chunk servable at once. The size and modification time catch the usual ways files change between runs, and checking a changed file against saved leaves costs one read and no download. Checkpoints only record synced data, so a crash loses at most the chunks written since the last one.


---

## D-180: Engine structure

**Date:** 2026-10-07 · **Status:** Decided (revised by D-220)

**Context:** D-079 to D-082 and D-118 fix what the engine does. M5 needs how: who owns what, what the queue counts, how statuses are decided, and what the event stream carries.

**Options for ownership:** shared state behind locks; one engine task owning the node, the store and every record, with helper tasks reporting back (chosen, per D-085). **For the queue:** one global limit; separate limits for incomplete and complete transfers, with force-started ones outside both (chosen). **For "completed":** any complete transfer that isn't running; one that stopped sharing on its own (chosen), while one the user stopped is "stopped". **For the event stream:** snapshots only; snapshots and decision events, numbered per run, with a bounded replay buffer (chosen).

**Decision:**

- The engine task owns the node, LAN discovery, the tracker client and the SQLite connection, and handles commands one at a time. Opening storage, which may hash, runs in a task of its own while the transfer is "checking". A supervisor task per running bundle owns its handle and tracker announcers, forwards its decision events, takes checkpoints and reports how it ended.
- Transfers are identified by bundle ID; adding a bundle twice is refused. Each run gets a generation number, so reports from an earlier run are ignored.
- The queue is one order over all transfers. Up to `max_active_downloads` incomplete and `max_active_uploads` complete transfers run, first in queue order; force-started transfers run regardless and take no slot. Errored and completed transfers wait until started again.
- A running download with no progress for 20 seconds is stalled, with a reason from its connection and peer counts. A complete transfer whose session stopped by itself, after lingering (D-065), is completed.
- Every half second the engine reads running bundles' statuses, records the manifest when it arrives and completion when it happens, and emits a snapshot of each transfer that changed. Every event is numbered from 1 per engine run; the last 4,096 are kept, so a subscriber can resume after the last one it saw, or learns that it missed some.
- Discovery for each running transfer uses, at once, the link's peer, the peers saved for the bundle, the link's and the config's trackers, LAN discovery, and peer exchange in the session.
- Known peers are saved from links only, until the driver reports the addresses of peers it reached.
- The config file is YAML with `settings` (a profile and overrides, as in scenarios), `peers`, `lan`, `api`, `data_dir`, `save_folder`, `queue` and `trackers`. A transfer's own overrides apply after the config's. The state database and the identity live in the data directory.

**Rationale:** One owner keeps every change to a transfer in one place, in order, and makes the store's writes simple. Separate limits match how qBittorrent and Transmission users think about slots, since finished transfers cost little. Telling "stopped" from "completed" says whether the user or the swarm rule ended a transfer. Numbering events per run is enough for SSE resume, since a restarted engine's subscribers must refresh their view anyway.

---

## D-181: Engine API details

**Date:** 2026-10-07 · **Status:** Decided (revised by D-222, D-231)

**Context:** D-080, D-125 and D-126 fix an HTTP + JSON API with resumable server-sent events, on loopback TCP 4776 by default, with remote access behind a token and HTTPS. M5 needs the routes, the error shape, how resuming works, how access is checked, how local clients find the API, and where settings changed through it live.

**Options for routes:** one `PATCH` per transfer for every field; an action or sub-resource per control (chosen), so each request is one engine command that succeeds or fails whole. **For resuming too late:** replay what is left; send an explicit `reset` with every transfer (chosen), also used as the first event of a new stream. **For local access:** no checks; checking `Host` and `Origin` and requiring JSON bodies (chosen). **For remote access:** a separate HTTPS listener beside the loopback one; one listener that switches to HTTPS and a token for every request (chosen). **For the token and certificate:** in the config file; generated into the data directory unless a certificate is configured (chosen). **For finding the API:** the config's address; a file the engine writes in its data directory while serving (chosen). **For settings changed through the API:** rewrite the config file; keep them in the state database over the file (chosen).

**Decision:**

- Routes are under `/v1`, as `specs/api.md` lists. Every error body is `{"error", "message"}` with a stable code. Bodies must be `application/json`.
- Events are numbered per engine run (D-180). `Last-Event-ID` resumes after an event the engine still keeps; otherwise the stream starts with `reset`, carrying every transfer. A new stream starts with `snapshot`, the same shape. Neither is numbered. A subscriber that lags gets `reset` and carries on.
- Without remote access, requests must name a loopback host and, if from a browser, come from a page on one.
- With `api.remote` on, the one listener serves HTTPS (rustls on ring, through tokio-rustls) and every request needs `Authorization: Bearer <token>`. The token is 32 random bytes in hex, made on first use and kept, readable only by its owner, in the data directory. The certificate is `api.tls_cert`/`api.tls_key`, or a self-signed one (rcgen) kept there. The daemon prints its fingerprint. A non-loopback `api.bind` requires remote access.
- While serving, the engine writes a discovery file in the data directory with the API's URL and certificate. Local clients read it and the token from there, trust exactly that certificate, and treat a URL that doesn't answer as no daemon.
- `PATCH /v1/settings` applies dotted keys all or none and saves them in the state database, which apply over the config file on every start. A saved change the file no longer allows is dropped. The engine never writes the config file. `data_dir` can't be changed this way. The response says which of `peers`, `lan` and `api` differ from what the engine started with.
- One engine uses a data directory at a time: it holds a lock file there while it runs.
- Transfers gain derived fields for UIs: progress, time left while downloading, and ratio (uploaded per byte downloaded, or per byte of the bundle when nothing was downloaded).

**Rationale:** One command per request keeps each answer exact and needs no partial-failure rules. A full snapshot is the simplest correct recovery when events are gone, and a client starting from nothing needs the same thing. Checking `Host` and `Origin` closes the classic attacks on local HTTP services (cross-site requests and DNS rebinding) at no cost to `curl` or local UIs. A token on every request, loopback included, means a reverse proxy or port forward on the same machine can't bypass it. Keeping secrets out of the config file lets the file be shared or committed. A discovery file works with any port, including the ephemeral ones tests use. Never rewriting the config file keeps the user's comments and layout.

---

## D-182: The CLI and the daemon

**Date:** 2026-10-07 · **Status:** Decided (revised by D-228)

**Context:** D-079 says the CLI talks to the engine's API and can start an embedded engine when none is running. D-175 made `share` and `get` standalone processes before the engine existed. A one-shot command can't run transfers for long, and `share` can't bind the peer port the daemon holds.

**Options for commands without a daemon:** fail; run an embedded engine with its transfers until Ctrl-C; open the engine's state without running transfers, make the change, and say so (chosen). **For `share` and `get` with a daemon:** stay standalone; always use the daemon; use it unless an option asks for a process of its own (chosen).

**Decision:**

- `tempest daemon` runs the engine and its API with `--config` and `--data-dir` (or `TEMPEST_CONFIG`, `TEMPEST_DATA_DIR`). The config file defaults to `config.yaml` in the data directory. `--set key=value` overrides a setting for one run. Ctrl-C or SIGTERM stops every transfer (telling peers and trackers) and saves state before the API closes.
- `tempest list`, `start`, `stop` and `remove` use the daemon's API if it answers. Otherwise they open the engine's state held: no transfer runs, the peer endpoint binds a loopback port the system picks, and LAN discovery is off. They then note that no daemon is running. Transfers are named by bundle ID, a unique prefix of at least four characters, or name.
- `tempest share` and `tempest get` hand their transfer to a daemon that answers. `share` prints the link and returns; `get` follows the download until it completes, fails or makes no progress for `--timeout`, and Ctrl-C leaves it to the daemon. `--standalone`, or any option only a process of its own can honor (`--port`, `--bind`, `--identity`, `--relay`, `--no-lan`, `--no-port-mapping`, `--lan-port`), runs them standalone as in D-175. The daemon honors `share`'s `--created` and `--tracker` itself.
- Ctrl-C during `get` exits with status 130 and says what became of the download. A config file named with `--config` must exist; only the default one may be missing.

**Rationale:** A held engine makes `start` and `stop` mean the same thing with or without a daemon, and the note keeps anyone from expecting progress that nothing will make. Handing transfers to a running daemon avoids two processes competing for the peer port, while explicit options keep scripts and tests that want a process of their own working unchanged.

---

## D-183: Speed limits

**Date:** 2026-10-07 · **Status:** Decided

**Context:** D-118 asks for global and per-transfer speed limits, plus alternative limits that can be toggled or scheduled. Nothing says where limits are enforced, what they count, or how the toggle and the schedule combine.

**Options for enforcement:** QUIC's flow control windows; pacing the UDP socket; token buckets on Data payloads in the driver (chosen). **For downloads:** throttle before sending a request, by its expected size; throttle after the Data arrives, before storing it and telling the session (chosen). **For the toggle and the schedule:** the schedule flips the toggle at its edges, as qBittorrent does; either one turns the alternative limits on (chosen).

**Decision:**

- Limits count Data payload bytes. The node has one download and one upload limit over every bundle, and each bundle has its own; a transfer waits for both. Uploads wait before sending Data; downloads wait after receiving it, holding the request's slot, so the session's adaptive pipeline shrinks to what the limit allows.
- Each limit is a token bucket that saves up a quarter second's worth and may go into debt, so a frame of any size passes and the rate over time stays within the limit plus one burst and one frame. Limits change while transfers run.
- The config's `limits.download` and `limits.upload` cover every transfer together; a transfer's own `limits.download` and `limits.upload` cover it alone, none by default. Rates are bytes per second, `null` for none.
- `limits.alternative` holds replacement limits, `enabled` to switch them on, and an optional weekly `schedule` (days, `from` and `to` in local time; a window ending earlier than it starts runs past midnight). They apply while switched on or while the schedule's window is open. The engine checks every half second.

**Rationale:** Throttling Data payloads in the driver is exact, works the same on every platform, and leaves the protocol and the sans-IO core untouched. Holding a received response throttles the sender too, through the pipeline, without guessing sizes. "Either turns them on" is one rule a user can predict from the settings alone, with no hidden state.

---

## D-184: Sharing limits, recheck, move storage and notifications

**Date:** 2026-10-07 · **Status:** Decided (revised by D-221, D-224, D-226)

**Context:** D-118 asks for sharing limits (stop or remove at a ratio or after a sharing time), recheck, move storage and notifications on completion and on error, at the engine level for M5. Their details are open.

**Options for sharing time:** time since completion; time spent sharing while complete, over every run (chosen). **For recheck granularity:** chunk by chunk; file by file (chosen, see below). **For a move's failure:** fail the request; answer at once and show the failure on the transfer (chosen). **For moving a share:** move its base folder; move its entries into the new folder, which becomes their base (chosen). **For notifications:** desktop notifications; a configured command (chosen).

**Decision:**

- `sharing.ratio`, `sharing.time` and `sharing.action` (`stop` or `remove`, keeping files) are defaults in the config, and a transfer can override each. A complete running transfer that reaches either limit stops, showing "completed", or is removed. Time spent sharing is kept in the state database.
- Recheck forgets the transfer's saved checkpoint, stops it if running, and opens its storage as a fresh download would, hashing every file ("checking"). The chunks found are saved, and it runs again if the queue says so, fetching the rest. A file whose root doesn't match is fetched again whole: the manifest gives only each file's root (D-013), so a damaged chunk can't be told from its neighbours without proofs from peers. A share rechecked this way can repair itself from the swarm.
- Move storage stops the transfer, moves a download's bundle folder, or a share's top-level entries, into the new folder ("moving"), and runs it again. Each item is renamed; across file systems it is copied with its modification time, and originals are deleted only once everything is in place. Nothing is moved onto something that exists. On failure everything is put back and the transfer shows an error saying where the files are. A download without its manifest just changes folder.
- `notifications.command` is a program and its arguments, run without a shell when a transfer completes during a run or newly fails. `TEMPEST_EVENT`, `TEMPEST_ID`, `TEMPEST_NAME`, `TEMPEST_KIND`, `TEMPEST_SAVE_FOLDER`, `TEMPEST_LINK`, and when known `TEMPEST_SIZE`, `TEMPEST_CONTENT_DIR` and `TEMPEST_ERROR` describe the transfer.

**Rationale:** Counting time spent sharing measures what the user gave, not how long the engine was off. Per-file recheck is all the manifest allows, and still fetches only damaged files. Answering a move at once keeps API requests short, and an error on the transfer is where a UI looks. Moving entries rather than the base keeps a single-file share from dragging its parent folder along. A command covers desktop notifications, chat hooks and scripts alike, and needs no platform code in the engine.

---

## D-185: The relay server

**Date:** 2026-10-07 · **Status:** Decided (revised by D-236)

**Context:** M6 builds the relay of `protocol/relays.md`. The architecture table had the relay crate depend on `protocol` only, but a relay needs the same raw-public-key TLS, identities and framed streams as peers, which live in `net`. Several details the protocol leaves open had to be settled: how a relay closes connections, a peer registering again while still connected, how kept addresses count against capacity, which IP a lent address carries, and how the tracker takes on the relay role (D-028).

**Options for sharing the QUIC and TLS code:** depend on `net` (chosen); copy it; move it into a new shared crate. **For closing codes:** reuse Goodbye's reason codes (chosen); define relay-specific codes. **For a second registration while the first is open:** refuse it until the old connection times out; replace the old connection, keeping the address (chosen). **For kept addresses at capacity:** count them and refuse newcomers; count them but give up the oldest kept one first (chosen). **For the lent IP:** require it in the config; default to the IP each peer reached the relay at, with an override for relays behind NAT (chosen).

**Decision:**

- `tempest-relay` depends on `protocol` and `net`. The binary is `tempest-relay` (D-236).
- A relay connection closes with Goodbye's reason codes: `protocol_violation`, `duplicate` for a registration replaced by a newer one from the same peer, and `shutting_down`. A newer registration takes over the older one's lent address. Both are now in `protocol/relays.md`.
- A peer that leaves has its lent socket kept bound for 5 minutes, so the same address is certain to be free when it returns. Kept sockets count toward the relay's capacity, and the one closest to expiring is given up to admit a newcomer.
- Limits: an allowlist, a cap on registrations and on registrations per source IP, and an optional per-peer byte rate in each direction, over which datagrams are dropped. Forwarding permissions per peer are capped in number, so forged sources can't exhaust memory.
- A tracker recommends a relay that runs separately, named with `--relay` (D-236).

**Rationale:** Depending on `net` keeps one implementation of the handshake rules. Replacing an old registration lets a peer whose network changed get its address back at once instead of waiting for a timeout. Keeping the socket bound makes stable addresses a guarantee rather than a hope that the port stays free.

---

## D-186: Client relay support

**Date:** 2026-10-07 · **Status:** Decided (revised by D-227)

**Context:** M6 lets a peer register with a home relay and be reached through it (`protocol/relays.md`). The protocol leaves the client's side open: how relayed packets reach QUIC, how the home relay is picked, where tracker-recommended relays fit, and when the relay address is published.

**Options for the wrapping layer:** a second quinn endpoint for relayed traffic; a custom socket beneath the one endpoint that maps each relayed sender to a made-up local address (chosen), as the implementer note suggests. **For the made-up addresses:** a lookup table only; ranges that never carry real traffic, `240.0.0.0/8` on IPv4 sockets and the discard prefix `100::/64` (RFC 6666) on IPv6 ones (chosen). **For picking the home relay:** ping each relay first; open handshakes with all candidates at once and register with the first to finish (chosen). **For tracker-recommended relays:** ignore them; use them by default, with a setting to turn that off (chosen). **For when to publish the relay address:** at once; after path MTU discovery makes room for a wrapped 1,200-byte packet, or after a few seconds (chosen).

**Decision:**

- The wrapping layer is a quinn socket around each real UDP socket. Forwarded packets enter quinn from a virtual address per relay and sender; quinn's packets to a virtual address leave as Forward datagrams, one per GSO segment. A virtual address is recognized by its range alone, so direct traffic never waits on the layer. The table of virtual addresses is bounded, and the least recently used one is forgotten first.
- A connection accepted from a virtual address, or dialed to a peer's relay address, is relayed. Status counts relayed connections, and the session's events carry each connection's path.
- Dialing a peer tries its direct addresses in turn and its relay address alongside, and keeps whichever opens first.
- The engine config's `relays.known` lists relay identities (empty by default, D-029); `relays.from_trackers` (on by default) adds the relays trackers recommend. The relays apply at once when changed. `share`, `get` and `daemon` take `--relay`.
- The home relay is the first relay to complete a handshake, which approximates the lowest latency. If it refuses, the next to finish is tried. A lost or failed registration is retried after 2 s, doubling to 5 minutes; a registration that lasted a minute resets the pause.
- The relay connection uses the peer socket, the relay ALPN, keep-alives every 15 s and a 45 s idle timeout. Peer connections keep alive at least every 60 s, whatever the endpoint's setting.
- Hello takes the relay address current when a connection opens. Announcers send the current relay address and announce again when it changes, within the minimum interval.

**Rationale:** One endpoint keeps one socket per family, as the protocol requires, and lets duplicate resolution compare relayed and direct connections by path. Racing handshakes measures latency with the connection that will be used anyway. Using recommended relays by default is what makes relays work without configuration, since none ship. Waiting for MTU discovery avoids advertising an address whose first packets would be dropped.


---

## D-187: Hole punching and the upgrade to direct

**Date:** 2026-10-07 · **Status:** Decided

**Context:** M6 upgrades relayed connections to direct ones (`protocol/relays.md`, Upgrading to a direct connection). The protocol leaves open how the work splits between session and driver, how long and how often to dial, what "stops dialing" means for attempts already under way, and how an upgrade fits the connection limit.

**Options for the split:** the driver runs the whole exchange; the session decides when to send Candidates and when to punch, and the driver dials (chosen). **For dialing:** one attempt per candidate, relying on QUIC's retransmissions; one attempt per candidate, started again if it fails, for a fixed time (chosen); a new attempt per candidate at a fixed interval. **For stopping:** abandon attempts under way; start no new ones but let those under way finish (chosen). **For the connection limit:** count the upgrade like any connection; let in a peer's first direct connection while it has only relayed ones (chosen).

**Decision:**

- The session sends Candidates on a relayed connection it accepted, once opened, and acts only on the first Candidates it receives on a connection. Its candidates are the driver's external addresses (the address the home relay observes) followed by the addresses it accepts connections at.
- Once it has sent and received Candidates on a relayed connection, with no direct connection open to the peer, the session asks the driver to punch; when a direct connection with the peer opens, it asks the driver to stop.
- The driver dials every candidate at once, the Forwarded `from` address first, for 5 s. A failed attempt is started again after 250 ms. Stopping starts no new attempts; attempts under way finish and are reported, and duplicate resolution sorts them out, so the higher-ID peer never drops a connection the other side may have completed.
- A peer's first direct connection is let in over the connection limit while every other connection with that peer is relayed, since it replaces one.
- `NodeConfig::punch_time` sets the time and turns punching off.

**Rationale:** Keeping the decisions in the session makes them testable without sockets and lets the simulator share them. QUIC already retransmits a pending handshake, so one attempt per address is enough while it lasts; restarting covers attempts that fail at once. Abandoning attempts under way would close connections the other peer may consider open.

---

## D-188: Port mapping

**Date:** 2026-10-07 · **Status:** Decided (revised by D-227)

**Context:** `protocol/connections.md` says clients SHOULD ask their router to forward a port (UPnP-IGD, NAT-PMP or PCP), and `protocol/relays.md` lists a port mapping's external address among the Candidates. M6 leaves open the libraries, the order of protocols, which sockets to map, lease times, what to do with a useless mapping, and where the external address goes.

**Options for the protocols:** one crate for all three (iroh's `portmapper`, `crab_nat`); `igd-next` for UPnP-IGD and PCP and NAT-PMP written by hand (chosen). **For the order:** all at once; PCP, then NAT-PMP if the router answers in it, then UPnP-IGD (chosen). **For the default:** off; on (chosen).

**Decision:**

- Only the IPv4 socket is mapped, and only when bound to every interface or a private address. IPv6 has no NAT, and a socket on loopback or a public address has nothing to map.
- PCP's MAP request goes to the default gateway (found with `netdev`). A NAT-PMP answer, or PCP's unsupported-version result, switches to NAT-PMP for that router. If neither maps the port, UPnP-IGD searches the network. Each failure is reported with its protocol in the endpoint's port mapping status; attempts that all fail are retried after a minute, doubling to an hour.
- Mappings ask for a 2-hour lifetime and are renewed halfway, with the same PCP nonce and external port. A failed renewal is retried halfway to expiry until the lease runs out. UPnP routers that only grant permanent mappings get one, asked for again every 30 minutes.
- A mapping whose external address is not public (double NAT, carrier-grade NAT) is removed and reported as a failure.
- Mappings are removed when the endpoint closes or mapping is turned off, giving the router 2 s to answer.
- The external address is advertised first among a peer's direct addresses: in Hello, and so in Candidates and Peers, in tracker announces (announcing again when it changes) and in share links.
- The engine config's `port_mapping.enabled` (on by default) applies at once; `share` and `get` take `--no-port-mapping`. `share` waits for the outcome like it waits for the home relay.

**Rationale:** PCP and NAT-PMP are small fixed-format protocols, so writing them keeps the dependency tree small and the parsers fuzzable; UPnP-IGD needs SSDP, HTTP and SOAP, which `igd-next` already does well. Trying PCP first asks the router that answers fastest with the most precise protocol, and the SSDP search only runs when it must. Mapping by default follows the protocol's SHOULD and is what lets two home users reach each other directly without a relay.

---

## D-189: The TUI's engine

**Date:** 2026-10-07 · **Status:** Decided (revised by D-228)

**Context:** D-079 and `specs/interfaces.md` let the TUI talk to the engine's API or run an embedded engine. M7 needs to choose how the embedded case works and how the TUI follows changes.

**Options for the embedded engine:** call `Engine` directly, behind a trait shared with the API client; run the engine in the TUI's process and serve its API there, the TUI using the API either way (chosen). **For following changes:** poll `GET /v1/transfers`; follow `GET /v1/events`, resuming with `Last-Event-ID` (chosen).

**Decision:**

- `tempest tui` uses the daemon if one answers. Otherwise it opens the engine, which runs transfers, and serves its API as the daemon would, recording it in the data directory. The engine stops when the TUI quits.
- The TUI follows the event stream and reopens it after a second when it breaks, resuming after the last numbered event. While it is broken the screen says so.
- Requests the TUI sends return no state of their own; the change shows when its event arrives.

**Rationale:** One path to the engine means one set of behaviors to test, and the event stream's resume and reset rules already cover a client that falls behind or reconnects. Serving the API from the TUI's engine also lets `tempest list`, `share` and `get` reach it while the TUI runs, as they would a daemon.

---

## D-190: Narrow lists drop ↑ first

**Date:** 2026-10-07 · **Status:** Decided

**Context:** D-138 drops columns in reverse priority (↑ first, then ETA), while the TUI sketch in `specs/interfaces.md` said ETA, then ↑.

**Decision:** D-138's order holds: ↑, then ETA, ↓ and size go as the terminal narrows; name, progress and status stay. Peers and ratio fill extra width after ↑. The sketch's text now points to D-138.

**Rationale:** Time left answers one of the list's four questions ("how long"), and ↑ answers none of them.

---

## D-191: TUI keys for transfer controls, the queue and finding

**Date:** 2026-10-07 · **Status:** Decided

**Context:** D-118's controls and finding features need keys in the TUI. `1`-`4` and `[ ]` switch areas (D-136), so the prototype's status-filter keys (`0`-`7`, `[ ]`) clash, and the spec reserves `c` and `g` for the chunk map and rates chart in the detail pane (M7.3).

**Options for the status filter:** cycle with a pair of keys; open a picker with counts (chosen). **For queue moves:** `K`/`J` plus `ctrl` variants; `K`/`J` one place and `<`/`>` to the top or bottom (chosen); palette only.

**Decision:**

- One table binds every command to its keys. Key handling, the hint bar, the help overlay and the `:` palette all read it, so they can't disagree.
- Lower-case keys act on the selection or open a modal (`a`, `x`, `r`, `t`, `m`, `o`, `f`, `s`); upper-case ones are the stronger or less common variant (`F`, `R`, `S`, `K`/`J`). The category picker is `C`, leaving `c` to the chunk map.
- Status filter and sort open pickers; `S` reverses the order. `esc` in the list clears the filters.
- Every modal closes on `esc` without doing anything; the filter prompt also clears what it typed.
- `o` opens the folder with the system's opener when the engine's API is on a loopback address, and otherwise says where the folder is on the engine's machine.

**Rationale:** Pickers show what each choice leaves (counts per status) and need no memorized order. `K`/`J` follow vim's "shift moves the thing, not the cursor" habit, and `<`/`>` read as "to the start or end". A single table keeps help and hints from drifting from the keys.

---

## D-192: How filters and sorting behave

**Date:** 2026-10-07 · **Status:** Decided

**Context:** D-118 lists filtering by name, tag and status and sorting by ten keys, but not how a query matches or which way each sort runs. The TUI and the web UI should behave alike (D-119).

**Decision:**

- A query is words that must all match, ignoring case: `#word` matches any tag containing it, any other word the name. Matching is live as the query is typed.
- The default order is the queue. Each sort key has a natural direction: name A to Z, status in the glossary's order, time left soonest first, and size, progress, rates, peers, ratio and date added largest or newest first. Reversing flips it.
- Unknown values (a size before the manifest arrives, no time left, no ratio) sort last in either direction. Ties keep queue order.
- "Peers" sorts by open connections, the closest the API comes to "peers serving us".

**Rationale:** Natural directions put what users look for first (the fastest, the biggest, the nearly done). Keeping unknowns last stops reversing from filling the top of the list with dashes.

---

## D-193: The API's view inside a transfer

**Date:** 2026-10-07 · **Status:** Decided

**Context:** The detail views (D-137) need a transfer's files, peers and chunks, which the API didn't expose. Chunks are 16 KiB, so a large bundle has hundreds of thousands of them.

**Options for chunks:** every chunk's state; run-length ranges of have and in flight plus availability; the chunks summarized into a number of cells the client asks for (chosen). **For fetching:** push changes on the event stream; poll routes while a view is open (chosen).

**Decision:**

- Three routes under a transfer: `files`, `peers` and `chunks?cells=N`, described in `specs/api.md`.
- The session summarizes its chunks into cells of near-equal runs. Each cell counts chunks stored, in flight and missing, and its availability is that of its rarest chunk. The answer is capped at 65,536 cells, encoded as arrays to keep it small.
- Peers come only from a running transfer. A stopped transfer's files and map come from its saved checkpoint.
- Clients poll these routes while a view shows them. The event stream stays about transfers as a whole.

**Rationale:** The client knows how many cells it can draw, so it asks for that many and the answer stays bounded. Run-length encoding is small for a nearly done download but not for a swarm's scattered chunks. Pushing peer and chunk changes would flood the stream for every client to show views few have open.

---

## D-194: The TUI's detail, chunk map and rates chart

**Date:** 2026-10-07 · **Status:** Decided

**Context:** The client layout puts a transfer's detail below the list, with the chunk map and rates chart one level further in (D-137), and gives `g` to the rates chart while the list already uses it for the first transfer (D-191). The detail needs keys, and the two full-screen views a scale and a source.

**Options for `g`:** move the rates chart to another key; give `g` to whichever screen has the keys (chosen). **For the rates chart:** a history route in the engine; the TUI's own samples (chosen).

**Decision:**

- Keys depend on the screen: the list, the detail, the chunk map or the rates chart. `enter` opens the detail from the list, and `esc` goes back one level. In the list `g` is the first transfer; in the detail and the chunk map it opens the rates chart, and `home` takes the top.
- In the detail, `tab`/`shift-tab`, `h`/`l` and the arrows switch tabs, and `j`/`k` scroll the tab. Decisions scroll back from the newest. Transfer and queue keys still act on the transfer shown; filtering and sorting stay with the list.
- Files, peers and the chunk map are fetched when their view opens and once a second while it shows, one request of a kind at a time (D-193). A failed fetch shows in its view, not as a notice.
- The chunk map asks for two cells per terminal cell below its title and legend, and draws them as half blocks: stored, downloading, missing dimmer the rarer, and no peer holding it. A transfer that isn't running shows only stored and missing.
- The rates chart draws the TUI's own once-a-second samples, kept for ten minutes, as braille lines for ↓ and ↑. The time axis spans at least a minute, and the rate axis tops out at a round number above the peak.

**Rationale:** One meaning per key per screen keeps the k9s/lazygit habit of keys that follow focus, and the hint bar and help, read from the same table, always show what `g` does where you are. The TUI samples every transfer's rates from the event stream already, so a history route would only matter to a client that just started.

---

## D-195: Clients keep their look and views in the engine's settings

**Date:** 2026-10-07 · **Status:** Decided (revised by D-231)

**Context:** The TUI's theme should survive a restart, and D-118's custom columns and saved views need a home. The TUI and the web UI share these choices by design (D-119).

**Options:** a file of the TUI's own in the user's config folder; the browser's storage for the web UI and a file for the TUI; a `ui` group in the engine's settings (chosen).

**Decision:** The engine's settings gain a `ui` group: the theme's name, the list's optional columns (`null` for the clients' default) and the saved views, each a name with a filter, a status and a sort. Clients change it through `PATCH /v1/settings` like any setting; it applies at once and is kept in the state database. The engine checks only that views have distinct names; names a client doesn't know are ignored there. The TUI takes the theme in once, at the start, so another client's choice never repaints it mid-session, and takes columns and views whenever no change of its own awaits an answer.

**Rationale:** The settings already persist, validate and reach every client, local or remote, with no new file format. One choice follows the user across interfaces and machines that share an engine.

---

## D-196: The TUI's Share, Catalog and Settings areas

**Date:** 2026-10-07 · **Status:** Decided (revised by D-234)

**Context:** D-136 and D-139 make Share, Catalog and Settings areas of their own. They need layouts and keys that fit the TUI's one-table key model (D-191) without letting typing clash with area keys such as `1`-`4` and `q`.

**Decision:**

- **Share** shows a form (path, name, comment, category, tags), the shares, and the selected share's whole link. The form takes the keys only after `a`, until `esc` or `enter`; while the engine builds the bundle the form says so. `y` copies a link with an OSC 52 escape, which works over SSH; terminals without it ignore it. `enter` opens a share's detail in Transfers.
- **Catalog** and **Settings** are sections of rows, a label and a value each, with the selected row's meaning below. `enter` or `space` changes a row (a prompt, a form, a toggle or the next choice), `a` adds (a tracker or relay in Catalog, by the section under the cursor; a category in Settings), and `x` removes an entry or gives a setting changed through the API the config file's value again (`DELETE /v1/settings/{key}`). `•` marks such settings. `tab` jumps between sections. Groups awaiting a restart show as a line above the rows.
- Values are typed as the TUI shows them: speeds as `500K` or `2.5M`, `none` for no limit, durations as the engine reads them. The prompt starts with the exact value, so `enter` alone changes nothing.
- `L` opens a transfer's own speed and sharing limits from the list or detail. Empty keeps the engine's setting and `none` overrides it with no limit; the transfer's other overrides stay.
- Catalog says plainly that browsing what trackers list arrives with M9.
- Help scrolls with `j`/`k` when it is taller than the terminal.
- The settings, which have no events, are fetched when Catalog or Settings opens, after each change and every five seconds while one shows.

**Rationale:** An explicit focus for the share form keeps every area's keys predictable, as the add modal does. Rows with one change key, one add key and one remove key cover every setting kind without a mode per kind, and showing the meaning of the row under the cursor keeps labels short.

---

## D-197: Custom columns and saved views

**Date:** 2026-10-07 · **Status:** Decided

**Context:** D-118 lists custom columns and saved views without saying what they hold; D-138 says they add to the columns' priority order.

**Decision:**

- The user picks which optional columns show from size, ↓, ↑, ETA, peers and ratio (the default set) plus category, tags and the date added (UTC). Name, progress and status always show, and narrow terminals still drop the chosen ones in D-138's order, the new ones last.
- A saved view is a named filter, status and sort, not columns, so applying one never rearranges the list's shape. Saving under an existing name replaces it.
- TUI keys: `|` picks columns, `v` lists the views (`enter` shows one, `x` deletes it) and `V` saves the current one.

**Rationale:** Views answer "show me these, in this order"; columns are a standing preference. Keeping them apart makes each simpler to explain, and both live in `ui` (D-195).

---

## D-198: The UI kit's shape

**Date:** 2026-10-07 · **Status:** Decided

**Context:** D-076 calls for one design-system crate shared by the web UIs, and the architecture lets `ui-kit` depend on nothing of Tempest's. The web UIs are served by the engine and may run with no internet access.

**Options for styling:** CSS written by Rust components; one stylesheet with themes as custom properties (chosen). **For the font:** Google Fonts, as in the M0 prototype; IBM Plex Mono bundled with the kit (chosen). **For data types:** the engine's API types; the kit's own (chosen).

**Decision:**

- One stylesheet holds the four themes' tokens (D-089, D-135), the type scale and every component's classes. Each theme also has six series colors for charts with more lines than ↓ and ↑. Tests check that every theme defines the same tokens, that text colors keep 4.5:1 contrast on the backgrounds and series colors 3:1, and that the fonts it names exist.
- The "clean" toggle the interfaces spec asks for turns off glow, scanlines and animation; reduced motion turns off animation and transitions.
- IBM Plex Mono's Latin subset, in three weights, ships with the kit under the OFL.
- Charts and the chunk map take the kit's own types: series of points with optional bands, bars with intervals, and chunk cells shaped like the engine's (D-193). Their geometry is plain Rust with tests; components only draw it and handle the pointer.

**Rationale:** Custom properties switch themes without re-rendering, and a test is the only thing that keeps four hand-tuned palettes legible. A bundled font works offline and leaks no requests. Owning its types keeps the kit buildable for WebAssembly without the engine's native dependencies, and lets the simulation UI reuse the charts.

---

## D-199: The UI gallery and the live view benchmark

**Date:** 2026-10-07 · **Status:** Decided

**Context:** UI quality is checked by eye in all four themes, and the Leptos guidelines require a CI benchmark of a 1,536-cell live view. The pattern it guards against costs time in the browser's reactive graph and DOM, which a native test doesn't exercise.

**Options:** a native test of the reactive graph alone; a Playwright script against a built page; `wasm-bindgen-test` in headless Chrome (chosen).

**Decision:** A crate, `ui-gallery`, shows every component, chart and the chunk map on made-up live data, with the theme picked by a query parameter for screenshots. Its browser test mounts the chunk map with 1,536 cells, applies updates that each change an eighth of them, and fails when an update averages more than a 60 Hz frame. CI runs it in headless Chrome in a debug build.

**Rationale:** A browser test measures what users feel, needs no JavaScript tooling, and uses the chromedriver GitHub's runners already have. The forbidden pattern takes about 25 times the diffing pattern's time, so a frame's budget separates them with room for slower runners.

---

## D-200: The API's types live in a crate of their own

**Date:** 2026-10-07 · **Status:** Decided

**Context:** The web UI runs in the browser, so it can't depend on `engine`, whose tokio, quinn and SQLite don't build for WebAssembly. It needs the API's types, and should behave like the TUI in filtering, sorting, columns and explanations (D-119), which lived in the TUI.

**Options:** mirror the types in `web` with round-trip tests against the engine's JSON; move them into a serde-only crate the engine, the TUI and the web UI share (chosen). **For the shared client logic:** port it to `web`; move it into the same crate (chosen); a second crate for it.

**Decision:** A crate, `api`, holds the API's data types (transfers, statuses, inspection, requests, events, errors and the `ui` settings) and the client logic that only reads them: following the event stream with rate samples, finding (D-192), which columns fit given each column's width in the client's own unit (D-138), plain-language explanations and saved views. It depends on serde and `protocol` only, and an optional feature lets the engine send its errors through axum. The engine re-exports the types where they were. The engine's config stays in `engine`, since it uses `net`'s types; clients read the settings they need.

**Rationale:** One definition can't drift, where a mirror needs tests to notice. Clients sharing the logic behave the same by construction, and a single crate keeps the dependency graph simple: everything in it builds for the browser and has no I/O.

---

## D-201: Building the web UI and embedding it in the binary

**Date:** 2026-10-07 · **Status:** Decided (revised by D-222)

**Context:** D-079 ships one `tempest` binary, and the engine serves the client web UI. The UI is WebAssembly built by trunk, a separate tool, and a plain `cargo build` must keep working without it.

**Options:** a build script that runs trunk; trunk run by hand or by CI into `crates/web/dist`, which the binary embeds when present (chosen); serving the files from disk at run time. **For a binary built without them:** fail the build; serve a page that says how to build the UI (chosen).

**Decision:**

- `trunk build --release --cargo-profile web` in `crates/web` builds the UI. The `web` profile optimizes for size, with LTO and `panic = "abort"`, and trunk runs `wasm-opt`.
- The `tempest` crate's build script embeds every file in `crates/web/dist` and reruns when it changes. The engine's server takes the UI's routes from the binary: `/` and each file, with content-named files cached for good and the rest revalidated. Every other path stays the API's.
- The UI's files are served without the API's access checks, since they hold no data, and no other page may frame them. With remote access on (D-125), the page has no way yet to present the token, so the UI is for local use until it gains one.
- Without the files, `/` explains how to build them and points to the TUI.
- `tempest web` opens the UI of a daemon that answers, or else runs the engine as the daemon would until Ctrl-C. The daemon and the TUI's own engine serve the UI too.
- CI builds the UI this way and tests the binary serving it.

**Rationale:** Nested cargo builds in a build script are slow, fight over the target directory and fail without trunk, while an optional folder keeps every native build and test working. Embedding keeps the one-binary promise and works offline.

---

## D-202: The web UI follows the engine and the keyboard

**Date:** 2026-10-07 · **Status:** Decided

**Context:** The web UI needs live updates, a clear state when the engine is gone, and the TUI's keys where they make sense in a browser (D-119, D-191). A browser's `EventSource` reconnects on its own schedule, gives up on an error answer, and can't send `Last-Event-ID` on a new stream.

**Options for reconnecting:** the browser's own reconnection; closing a broken stream and opening a new one after a backoff (chosen), which needs another way to resume. **For resuming:** always start over with a snapshot; `GET /v1/events?after=N` beside the header (chosen).

**Decision:**

- The engine accepts `?after=` where `Last-Event-ID` isn't given; the header wins if both are. The page reconnects after 1, 2, 4, 8, then 15 seconds, resuming after the last numbered event, and says plainly while the engine can't be reached, with a button to try now.
- Events are applied in batches a tenth of a second apart. Rows follow D-199's rule: each has its own signal, which one pass updates after each batch, and the cursor is a selector.
- Columns that don't fit are hidden with one class on the table, so a resize redraws no rows. Widths are pixels; the order is the TUI's (D-138).
- Keys follow one table, as in the TUI. `tab` keeps its meaning in a browser, so the detail's tabs are `h`/`l` and the arrows. `f` and `s` focus the status and sort pickers. `esc` closes a dialog, leaves a field, goes back one level of the detail, then clears the filters. Keys are ignored while typing or with ctrl, alt or meta held.
- The theme is read from the `ui` settings once the engine answers and saved when changed (D-195). Share and Catalog say they are in the TUI for now; Settings holds the theme.

**Rationale:** An explicit backoff gives the page one place to know the engine is gone and to say so, and a query parameter is the only way a new `EventSource` can resume. The rest keeps the TUI's habits without fighting the browser's own keys.

---

## D-203: The web UI's Share, Catalog and Settings areas, and its notifications

**Date:** 2026-10-07 · **Status:** Decided (revised by D-234)

**Context:** The web UI needs the TUI's areas (D-196), columns and views (D-197), and notifications of completions and failures (D-118). A browser offers inline controls that a terminal can't, and its notification permission belongs to the browser, not the engine. The engine's settings type doesn't build for WebAssembly.

**Options for editing settings:** a prompt per row, as in the TUI; each row's control in place (chosen). **For the notification switch:** the engine's `ui` settings; the browser's own storage (chosen). **For the web UI's effects:** the browser's storage; the `ui` settings (chosen). **For reading settings:** mirroring them by hand in the web UI; client types in `tempest-api` that skip what they don't name, checked against the engine by a test (chosen).

**Decision:**

- Catalog and Settings show D-196's sections as rows, each with its control in place: a field that saves on `enter`, a switch, a choice, the schedule's days and times, or a button. Changes through the API carry `•` and a reset button. The keyboard's cursor moves through the rows; `enter` uses the row's control, `x` its remove or reset button and `a` the add field of the cursor's section. The Queue section lists the waiting transfers with buttons to reorder them.
- Share is D-196's form; while the engine builds the bundle the page shows an indeterminate bar and the seconds waited, then the link to copy. The shares are listed below it.
- Catalog says plainly that browsing what trackers list arrives with M9, with its search field disabled.
- A transfer's tags, category, folder and own limits, the columns and the saved views are dialogs, opened by the TUI's keys or buttons. `|` and `v` work from the list; `V` opens the views dialog at its save field.
- The `ui` settings gain `scanlines` and `clean`, which the TUI ignores.
- A completion or failure seen in the event stream gives a toast, and a browser notification when the user switched them on in this browser, which asks the browser's permission. Transfers first seen in a batch give none, so opening the page is quiet.
- While a text field has the keys, the hint bar shows only `enter` and `esc`.

**Rationale:** Controls in place are what a browser user expects and need no mode per kind, while one cursor keeps the TUI's keys. A permission granted to one browser shouldn't switch notifications on in another. Effects are part of the look, which D-195 keeps with the engine.


---

## D-204: The simulation results UI and its API

**Date:** 2026-10-07 · **Status:** Decided (revised by D-214 and D-222: `tempest-sim web` serves it)

**Context:** D-074 and D-075 put simulation results in DuckDB, read by a Rust web UI. The UI is WebAssembly and can't open the database, so something must serve it; the comparisons it shows must agree with what `tempest sim run` and `tempest sim baseline` print.

**Options:** **Where the server lives:** a new server crate; the simulator behind a feature; a thin axum layer in the `tempest` binary over query functions in the simulator (chosen). **Shared types:** the simulator's own types, built for the browser without its `results` feature; a small `sim-api` crate (chosen), as `api` is for the engine (D-200). **Database access:** one connection for the server's life; a read-only connection per request (chosen). **Where views live:** separate pages; one page with the view in the address's `#` (chosen).

**Decision:**

- `tempest sim web [--db PATH] [--baselines DIR] [--listen ADDRESS] [--no-open]` serves the results UI and a JSON API under `/v1`: experiments and runs with filters, sorts and pages; a run with its peers, traffic and time series; comparisons of any variants, from one experiment or several; and the golden baselines with experiments checked against them. It listens on `127.0.0.1:7340` by default, and its UI is embedded like the client's (D-201).
- The simulator's `browse` module answers each route with `sim-api`'s types. Filters travel as SQL parameters; sorts come from a fixed table of columns. The query strings are parsed and written by `sim-api`, so the UI and the server agree, and are fuzzed.
- Comparisons use the simulator's `stats` (D-167): medians across seeds with bootstrap intervals, and differences paired by seed with a verdict. Over time, each variant's completed fraction, rarest-chunk copies and peers present are medians across seeds at 80 moments, with 1,000-resample intervals so a page loads quickly; a quantity no run has two moments of is left out.
- An experiment is checked against the baseline whose scenario has its name, on the seeds both have, as `tempest sim baseline` checks: by means for up to three seeds, otherwise seed by seed (D-171). Baselines are read on every request, so updates show without a restart.
- Each request opens the database read-only and closes it, so `tempest sim run` can add experiments while the UI is open; while a run records, requests answer that the database is busy.
- The UI has four areas: Experiments, Runs, Compare and Baselines. Variants are picked into a comparison basket kept in the browser. Every view has a link.

**Rationale:** Query functions in the simulator are tested against real recorded runs beside the code that writes them, and the HTTP layer stays thin. A tiny types crate keeps the simulator out of the browser. Short-lived read-only connections fit DuckDB's one-writer rule without making the UI and the simulator take turns by hand.

---

## D-205: The announce's listing field is a signature

**Date:** 2026-10-07 · **Status:** Decided

**Context:** D-129 adds an optional listing field to announces and attributes a listing to the key that asked. Announces are unsigned, so a bare flag would let anyone list a bundle in another peer's name.

**Options:** a boolean flag, listing under the unverified peer ID; a catalog session token in the announce; the peer's Ed25519 signature over a fixed message naming the bundle (chosen).

**Decision:** `listing` is `{ "sig": … }`, the announcing peer's signature over `tempest listing v1`, a zero byte and the bundle ID (`protocol/trackers.md`, with a test vector). A tracker without a catalog ignores it; one with a catalog refuses an announce whose signature fails. Clients include it in every announce for a bundle they chose to list there.

**Rationale:** The peer key is the account key (D-128), so a signature proves the request without sessions in announces, and its owner needs no sign-in to list. It depends on the bundle only, so it is computed once and a stateless tracker catches up after a restart; a replay repeats only what the owner asked. Withdrawing is the catalog's business: a withdrawn listing stays withdrawn until its publisher edits it.

---

## D-206: Catalog accounts, sessions and storage

**Date:** 2026-10-07 · **Status:** Decided (revised by D-225, D-234)

**Context:** D-128 signs catalog users in with their Tempest key, but a browser doesn't hold the key. M9 also leaves the catalog's storage and API open.

**Options:** **Browser sign-in:** the local engine signs for the page (cross-origin calls to localhost); a code shown in the page and signed by the user's client, like a device-code flow (chosen). **Storage:** SQLite (chosen, as the engine's state, D-082); files; an external database. **Types:** in `tracker`; a serde-only `catalog-api` crate shared with the client and the catalog UI (chosen, as D-200 and D-204).

**Decision:**

- A tracker runs a catalog only when its operator passes `--catalog FILE`; `--invites-required` and `tempest tracker invite` make invites, and `--category` replaces the default categories, which lead with D-031's legitimate uses.
- Sign-in: `POST /catalog/v1/challenges` gives a typeable code and a poll secret. The key signs `tempest catalog sign-in v1`, a zero byte, the tracker's authority as the signer reached it, a zero byte and the code, and posts it to `/catalog/v1/sessions`, which answers with a session; the holder of the poll secret can collect that session once, as an `HttpOnly`, `SameSite=Strict` cookie. Codes last ten minutes and are kept in memory. Sessions last 30 days and are stored as hashes of their tokens.
- Requests authenticate with a bearer token or the cookie. Bodies must be JSON with a JSON content type, so no cross-site form can act with a browser's session.
- Listings are pending until the tracker knows the bundle's content, and only their publisher sees them then. The first key to ask keeps a bundle's listing. Publishers edit title, description, category and tags; signed-in accounts vote up or down and comment, and delete their own comments.

**Rationale:** The key never leaves the client, and the device-code flow works from any browser. Signing the authority stops a tracker from passing on another tracker's code to sign in there as the user. SQLite needs no service to run, and the shared crate keeps the tracker, the client and the UI agreeing on every body and text rule.

---

## D-207: How the tracker fetches manifests to list bundles

**Date:** 2026-10-07 · **Status:** Decided

**Context:** D-129 has the tracker fetch a listed bundle's manifest from peers, but leaves open which peers, how often and within what limits. Every fetch is a QUIC connection the tracker opens to addresses that announces gave it.

**Options:** **Peers:** only the publisher; any peer at random; the publisher first, then complete peers, then incomplete ones (chosen). **Retries:** a fixed interval; exponential backoff (chosen). **Giving up:** never; after a number of attempts; after a time since the first failure (chosen). **State:** in the catalog's database; in memory (chosen).

**Decision:**

- A background task, run only with a catalog, looks at the pending listings every few seconds. For each one due, it asks up to three live peers of the swarm in turn: the publisher, then complete peers, then incomplete ones, each group in random order. It dials at most four addresses per peer, the observed one first, then its relay address.
- It opens the connection as a peer with an empty Have and sends ManifestRequest, from an endpoint with a fresh key. A manifest larger than the limit (4 MiB by default) is refused unread; one that doesn't hash to the bundle ID is discarded.
- At most eight bundles are fetched at once. A failed attempt waits 30 seconds, doubling up to an hour. An empty swarm waits without counting as a failure. Three days after its first failure a bundle is given up on until the tracker restarts, and stays pending.
- `tempest tracker` takes `--fetch-concurrency`, `--fetch-max-manifest` and `--fetch-give-up`.

**Rationale:** The publisher announced the listing and almost surely has the manifest; complete peers certainly do. Few peers and few addresses per attempt bound how many connections an announce can make the tracker open. The manifest limit bounds memory well below the protocol's 64 MiB, since a catalog needs only the name, size and file count. Keeping attempts in memory keeps the store simple; a restart retries everything once, which is cheap.

---

## D-208: Publishing from the client

**Date:** 2026-10-07 · **Status:** Decided (revised by D-234)

**Context:** D-129 lets sharers opt in per tracker or with `tempest share --publish`, and D-128 signs them in with their key. The daemon holds the key, while the CLI and UIs talk to it through its API.

**Options:** **Who signs:** the CLI, loading the key file itself; the engine, through API routes (chosen). **Publish state:** a flag per transfer; the set of trackers per transfer (chosen). **Config:** tracker entries become objects with a `publish` flag; a separate `publish` list of tracker URLs (chosen).

**Decision:**

- The engine stores, per transfer, the trackers it is listed on, and announces there with `listing` (signed once per run). Trackers in the config's `publish` list apply to every new share; a share also announces to them.
- `POST /v1/transfers/{id}/publish` signs in with the engine's key, sets the listing's fields, signs out, then records the tracker; a running transfer starts announcing there with `listing` at once. `POST /v1/catalog/sign-in` signs a browser's code.
- `tempest share --publish <tracker>` with `--title`, `--description`, `--category`, `--tag` and `--invite` publishes through the daemon, or standalone with the share's own key. `tempest catalog publish` publishes a transfer already added, and `tempest catalog sign-in <tracker> --code` signs a browser in.

**Rationale:** The key stays with the engine, so the catalog account is always the key that announces and serves the bundle. A set of trackers survives restarts and config changes, and a plain list in the config keeps the existing `trackers` format unchanged.

---

## D-209: Catalog search, facets and ranking

**Date:** 2026-10-07 · **Status:** Decided (revised by D-234)

**Context:** D-130 chooses Tantivy for catalog search, with typo tolerance and category and tag facets, and ranks by live swarm health, votes and downloads over time. It leaves open where the index lives, how typos and ranking work, how downloads are counted and what the query looks like.

**Options:** **Index:** on disk beside the SQLite file; in memory, rebuilt from SQLite at startup (chosen). **Typos:** Tantivy's query parser with fuzzy fields; a query built per word (chosen). **Ranking with live counts:** store swarm counts in the index; rank the matches in Rust with counts read at query time (chosen). **Downloads:** every `completed` announce; once per peer as it becomes complete (chosen); per IP. **Query string:** serde's URL encoding; a parser in `catalog-api`, as `sim-api`'s (chosen), since tags repeat.

**Decision:**

- The tracker keeps a Tantivy index of listed bundles in memory: title, description, tags, content name and category name as text, category and tags as facets. It is built from the store when the catalog opens and updated whenever a listing is listed, edited or withdrawn.
- Every word of the search must match a field, exactly or within an edit distance of 0 (up to 3 letters), 1 (up to 6) or 2; the last word also matches as a prefix. Exact matches score by BM25 above fuzzy ones; titles weigh most, then tags and content names, then category names and descriptions. Words are lowercased and folded to ASCII.
- `GET /catalog/v1/listings` takes `q`, `category`, `tag` (repeated), `sort`, `offset` and `limit`, parsed by `catalog-api`'s `ListingQuery`. Its page carries facet counts: categories as if no category were picked, tags among the matches.
- Sorts: `relevance` (default), `newest`, `seeders` (complete peers, then all), `votes` (up less down) and `trending`. Relevance is the text score times e^(0.2 × popularity), where popularity adds the logarithms of swarm health (complete peers plus a quarter per incomplete one), net votes (signed) and trending; without text, popularity alone orders. Trending sums the last 30 days' downloads, halving every 3 days. Ties go to the newer listing.
- Catalog store migration 2 counts downloads per bundle and day. A `completed` announce counts if the peer wasn't already a live complete peer, and only for bundles with a listing. Listings show their total downloads.

**Rationale:** Rebuilding is cheap at catalog sizes and keeps the index from drifting from SQLite after a crash, so SQLite stays the one source of truth. Per-word queries give typo tolerance with exact matches still ranking first, and prefix matching makes search-as-you-type work. Swarm counts change with every announce, so they are read live rather than indexed. Logarithms keep one huge swarm or vote count from burying better text matches. Counting a peer's transition to complete stops repeated announces from inflating downloads, though new peer IDs still can; per-IP or per-account limits are still open.

---

## D-210: The catalog's web UI

**Date:** 2026-10-07 · **Status:** Decided (revised by D-234)

**Context:** D-118 and `interfaces.md` ask for a catalog UI in the client web UI's framework and quality bar, served by the tracker, with search, facets, sorting, listing pages, votes, comments, publishing and sign-in with a Tempest key (D-128). How it is served, how it routes, pages and signs in, and how it is tested in a browser were open.

**Options:** **Serving:** a separate static site; embedded in the binary like the other UIs (D-201) and merged into the tracker's router when it runs a catalog (chosen). **Routes:** paths, which need the server to answer each with the page; the address's `#`, as the other UIs (chosen), with the API's own `ListingQuery` string after `#/?`. **More results:** infinite scroll; numbered pages (chosen). **Sign-in:** a page that only shows the code; the code with the exact `tempest catalog sign-in` command, polling for the session (chosen). **Browser test:** `wasm-bindgen-test`, which can't reach a tracker on another origin; Playwright, which brings Node into CI; WebDriver against ChromeDriver from a Rust test (chosen).

**Decision:**

- A crate, `catalog-web`, built by trunk into `crates/catalog-web/dist` and embedded in `tempest`. `tempest tracker --catalog` serves it at `/` beside the API; without the files, `/` says how to build them.
- Views: browsing (`#/?q=…&category=…&tag=…&sort=…&offset=…`), a listing, its edit form, sign-in and the account with its listings. The search runs as it is typed, a moment after the last key, replacing the history entry; an answer that arrives after a newer request was sent is dropped. Facets and the old results stay while a new answer loads, dimmed.
- The sign-in view asks for a challenge, shows the code and the command that signs it with the tracker's origin (and an invite when the catalog requires one), and polls every two seconds until the session's cookie is set or the code expires; then it returns to where the user was.
- Keys come from one table, scoped by view: `/` search, `j`/`k`/`enter` move and open, `[`/`]` pages, `s` sort, `+`/`-` vote, `c` comment, `e` edit, `a` account, `T` theme, `esc` back. The theme defaults to green (D-135) until one is chosen.
- `crates/tempest/tests/catalog_web.rs` drives headless Chrome through ChromeDriver: search with a typo, category and tag facets, the keyboard, and sign-in with `tempest catalog sign-in`. It is ignored unless run explicitly (D-216).

**Rationale:** Embedding keeps the one-binary promise, and hash routes need nothing from the tracker but `/`. Numbered pages give every result a link and work with `[`/`]`. Showing the command, with the origin the signer must use, turns a code into one paste. WebDriver needs no new dependencies or toolchains, and CI's runners already have ChromeDriver.

---

## D-211: Release configuration

**Date:** 2026-10-07 · **Status:** Decided

**Context:** D-133 chooses cargo-dist. Its configuration leaves some details open.

**Options:** **Web UIs in release builds:** build them in a setup step before dist builds the binaries (chosen); commit their output. **Trunk in CI:** `cargo install trunk`, which is slow but needs no pinned download; a prebuilt install action. **Tap:** `gregorybchris/homebrew-tap`. **Updater:** none; the installers rerun instead.

**Decision:**

- `dist-workspace.toml` configures dist and `.github/workflows/release.yml` is generated from it; `dist generate --check` verifies they agree. Only `tempest` is a distributable app; the web crates are `publish = false`.
- Releases build when a version tag is pushed. Pull requests only run `dist plan`, which builds and publishes nothing.
- `.github/build-setup.yml` builds the three web UIs before every platform build, so release binaries embed them (D-201).
- Binaries install into `CARGO_HOME`'s `bin` folder, with no self-updater.

**Rationale:** Committing build output would drift; a setup step keeps one source. Installing trunk with cargo is the least to maintain, and release builds are rare enough to absorb its time.

## D-212: Cross-platform checks in CI

**Date:** 2026-10-07 · **Status:** Decided

**Context:** Releases (D-211) build on Windows and on Linux for arm64. Locally, `cargo check --target x86_64-pc-windows-msvc -p tempest` fails: `ring` compiles C and needs the target's headers, which a Mac lacks. A review of the code that differs by platform found each case gated or given a Windows implementation: file permissions, `O_NOFOLLOW`, positioned reads and writes, symlinks, `SO_REUSEPORT`, signals and the data directory.

**Options:** **Windows check:** cross-compile locally (not possible here); a Windows runner in CI (chosen). **Scope:** `cargo check` only; clippy over all targets plus the tests of the crates holding platform code (chosen). **Dist drift:** rely on the generated workflow's own `dist plan` on pull requests; also run `dist generate --check` in CI (chosen), so a hand edit of the config or workflow fails.

**Decision:**

- CI's `platforms` job runs clippy for `tempest` and the `tempest-net` and `tempest-engine` tests on Windows, Linux arm64 and macOS.
- CI's `dist` job runs `dist generate --check` and `dist plan`.
- `cargo build --profile dist -p tempest` was built on the host and the binary runs.

**Rationale:** Only a native runner compiles the C dependencies and runs the platform code, and it fails before a tag does.

---

## D-213: MIT license only (supersedes D-121)

**Date:** 2026-10-07 · **Status:** Decided

**Decision:**

- Tempest is licensed under MIT only, in a single `LICENSE` file.
- The Apache-2.0 half of the earlier dual license is dropped.
- `deny.toml` still allows Apache-2.0 and the other listed licenses for dependencies.

**Rationale:** One license is simpler than two.

---

## D-214: The simulator ships as its own binary

**Date:** 2026-10-07 · **Status:** Decided

**Context:** The `tempest` binary includes the simulator, DuckDB and the simulation results UI, which most users never need. D-079 put everything in one binary.

**Decision:** The simulator, its results UI and its DuckDB dependency move out of `tempest` into a separate binary. The `tempest` binary keeps the client, the engine, the UIs, the tracker and the relay. This refines D-079.

**Rationale:** The everyday client gets smaller and builds faster. Simulation is a development and research tool.

**Implementation:**

- The binary is `tempest-sim`, with `run`, `baseline` and `web` as its subcommands, so `tempest sim run …` is now `tempest-sim run …` (D-160, D-204). Its package is `tempest-sim-cli` in `crates/sim-cli`, since `tempest-sim` is the library; dist names the app, archive and Homebrew formula after the package.
- Serving embedded UIs and embedding them from build scripts moved to a small `tempest-assets` crate that both binaries use.
- `tempest sim …` is a hidden command that fails with a message pointing to `tempest-sim`, so old muscle memory and scripts get an explanation rather than a parse error.
- Releases ship both apps. DuckDB is bundled only into `tempest-sim`, through the package's own dist `features`. Builds of `tempest` need no DuckDB, `cargo tree -p tempest` has none, and development builds of `tempest-sim` still link the system library (specs/development.md).

---

## D-215: Merging peer records

**Date:** 2026-10-07 · **Status:** Decided

**Context:** A session replaced a known peer's record with any newer one, so a LAN record (one address, no relay) erased addresses and the relay learned from a tracker or peer exchange. `protocol/messages.md` also says a PeerRecord's addresses never hold a relay address, which nothing checked on receipt.

**Decision:**

- A record for a known peer is merged: its addresses first, then the known ones it lacks, without duplicates. Its relay replaces the known one; with none, the known relay is kept.
- A session keeps a bounded number of addresses per known peer (`MAX_PEER_ADDRESSES` in `tempest-core`), dropping the oldest. The protocol sets no bound on a record's addresses.
- An address equal to the record's relay address is dropped rather than the record rejected. It is the only relay address a receiver can recognize, and the rest of the record is still useful.

**Rationale:** Sources each know part of a peer's addresses, and none is more authoritative. Newest first means dials try fresh addresses before stale ones.

---

## D-216: Limits on incoming connections before Hello

**Date:** 2026-10-07 · **Status:** Decided

**Context:** The node spawned a routing task for every incoming connection and waited up to `hello_timeout` for Hello. The session's connection limits apply only once Hello names a bundle, so anyone could hold any number of handshakes and tasks open.

**Options:** accept the handshake and close with Goodbye (`too_many_connections`); refuse the attempt before the handshake.

**Decision:**

- `NodeConfig` bounds the incoming connections still opening (handshake and Hello), in all and per source (`max_opening`, `max_opening_per_source`).
- A source is the IP address, an IPv6 address's /64 prefix, or for a connection through the home relay, the sender's address as the relay reports it.
- Attempts beyond a limit are refused without a handshake (QUIC `CONNECTION_REFUSED`), not closed with Goodbye.

**Rationale:** Refusing costs no TLS work, which is the point of the limit. Before the handshake there is no authenticated channel to carry a Goodbye reason. A /64 is what one host commonly holds. The per-source default leaves room for one peer to open a connection per bundle for many bundles at once.

---

## D-217: Catalog rate limits

**Date:** 2026-10-07 · **Status:** Decided

**Context:** The catalog had no rate limits. Anyone could request sign-in codes until the challenge table was full, which blocked sign-ins for everyone, and post comments, votes and edits, or inflate download counts with fresh peer IDs, as fast as the tracker answered. D-176 limits only announces, per peer.

**Options:** **Key:** per account, which signing in with a fresh key evades; per client IP (chosen). **Algorithm:** fixed windows; a token bucket per action and client, kept as one time each, as in GCRA (chosen). **Over the limit on an announce:** refuse the announce; answer it but skip the catalog's part (chosen).

**Decision:**

- Every catalog route is limited per client IP, with a bucket per action: challenge, sign-in (signing and polling), browse (every GET), edit (other writes), vote and comment. Requests over a limit get 429 `rate_limited` with `retry_after_secs` and `Retry-After`, in the usual error body. Signing out isn't limited.
- Announces count against two more: publish, only when they ask to list a bundle the catalog doesn't know yet, and download, for each counted `completed`. Over either, the announce succeeds but lists or counts nothing; a later announce asks again.
- The client IP is the one announces observe, so `--behind-proxy` applies. IPv6 clients are grouped by /64.
- Each IP holds at most 16 outstanding sign-in codes by default (`--challenges-per-ip`), under the global cap; codes still expire after ten minutes.
- The limiter forgets idle clients when it reaches 100,000, and refuses new ones with `unavailable` while it can't. Limits are set with `--catalog-limit action=BURST/SECONDS` or `action=off`; the defaults live in `crates/tracker/src/limits.rs`.

**Rationale:** Per-IP limits need no account and catch key churn. Defaults are generous enough for a browser polling every two seconds and a sharer publishing a hundred bundles at once. Refusing an announce over catalog work would cost the peer its swarm.

---

## D-218: The catalog browser test is ignored unless run explicitly

**Date:** 2026-10-07 · **Status:** Decided (revised by D-234)

**Context:** The browser test (D-210) passed with a note when `CHROMEDRIVER` was unset or the binary lacked the catalog UI, so a broken CI setup stayed green.

**Options:** fail when an environment variable such as `TEMPEST_REQUIRE_BROWSER_TESTS` is set; mark the test `#[ignore]` and run it with `--ignored` where the environment exists (chosen).

**Decision:** The test is `#[ignore]` with a reason. Run explicitly, it fails without ChromeDriver or the built-in UI. CI's web job runs it with `--ignored`.

**Rationale:** An ignored test shows as ignored in every run, rather than as a pass, and needs no extra variable to remember. Plain `cargo test` still works anywhere.

---

## D-219: Themed form controls in the UI kit

**Date:** 2026-10-07 · **Status:** Decided

**Context:** The web UIs used native selects, checkboxes and radios. Their popups and marks are drawn by the browser and operating system, so they ignored the four themes and looked like default HTML.

**Options:** a component library such as biji-ui; a port of Radix's primitives; the kit's own components, with `leptos-use` for browser behavior (chosen).

**Decision:**

- The select is the kit's own: a button with the `combobox` role and a listbox popup, following the WAI-ARIA select-only combobox. Focus stays on the button; the arrows, `Home`, `End`, the page keys and type-ahead move the highlight; `enter` or `space` chooses; `esc`, a click outside or leaving closes it. The popup is placed against the button in a portal, below it unless above has more room, so dialogs neither clip nor shift it.
- Keys a select uses stop there, so an app's shortcuts and dialogs don't also act on them. The apps count a combobox as a field that takes typed keys, as they did native selects.
- Checkboxes, radios, text, search, number, range and file inputs stay native, restyled with `appearance: none` and the theme tokens, for the browser's accessibility and keyboard behavior. On/off settings are switches: buttons with the `switch` role.
- Every control has hover, focus, invalid and disabled states, from the theme tokens only.
- `leptos-use` 0.19 supplies click-outside and event listeners, with only those features enabled.

**Rationale:** No stylesheet reaches a native select's popup, so a themed one must be drawn. One select written against the ARIA pattern is little code next to a library whose look would still need overriding. Native elements already handle the other controls' accessibility, and restyling them needs no script.

---

## D-220: No limit on active uploads by default

**Date:** 2026-10-07 · **Status:** Decided

**Context:** Under D-180, a complete transfer takes one of five upload slots. A share never stops on its own (D-156), so once five run, each new share waits in the queue and its link doesn't work.

**Options:** keep the limit; exempt shares from it; no upload limit by default (chosen).

**Decision:** `queue.max_active_uploads` defaults to no limit. The download limit stays. This revises D-180.

**Rationale:** A link you created should always work. An idle share costs almost nothing, and the speed limits (D-183) already bound bandwidth.

---

## D-221: Where downloads land

**Date:** 2026-10-07 · **Status:** Decided

**Context:** Every download was written to a folder named after its bundle, so a shared `report.pdf` arrived as `~/Downloads/report.pdf/report.pdf`. Moving a share's storage put its entries straight into the target, while moving a download kept its folder. D-143 left the single-file layout open.

**Decision:**

- A bundle with one file entry is written straight into the save folder. Bundles with several entries get a folder named after the bundle.
- If the destination name exists, the download fails with an error that names it, as now. Nothing is renamed or overwritten.
- Moving a share's storage keeps its folder name, as moving a download does. This revises D-184.

**Rationale:** A single file in its own folder surprises everyone. Failing on a collision keeps names predictable. Shares and downloads should move the same way, and moved shares should never mix their files.

---

## D-222: Signing in to the web UI from another machine

**Date:** 2026-10-07 · **Status:** Decided

**Context:** With remote access on (D-125), every request needs the token in a header. The web UI's event stream (`EventSource`) can't send one, and the UI can't ask for the token, so it works only locally. The token amounts to running commands on the machine, since the API can set `notifications.command`. `tempest sim web --listen` serves without authentication on any address.

**Options:** local only, reached by SSH port forwarding; a login that exchanges the token for a session cookie (chosen); the token in browser storage with a `fetch`-based stream.

**Decision:**

- In remote mode, the web UI shows a sign-in page. The engine exchanges the token for an `HttpOnly`, `Secure`, `SameSite=Strict` session cookie, which every request and the event stream then carry. The CLI and TUI keep using the header.
- `tempest sim web` warns when it listens on an address other than loopback.

**Rationale:** Remote control of a headless daemon is the reason D-125 exists. The catalog already signs in with a cookie (D-206), and a cookie scripts can't read is safer than a stored token.

---

## D-223: Superseeding offers a window of ranges, and by default only while a crowd wants the bundle

**Date:** 2026-10-07 · **Status:** Decided (amended 2026-10-08 after simulation: the default is `crowd`)

**Context:** D-063 made superseeding opt-in. In simulation, a crowd arriving at once was done once the origin had uploaded 2.5 copies under round-robin, against 1.0 with superseeding. As built (D-155), a superseeding origin offers each peer one range and waits for it to announce the whole range before offering the next, which caps a 1:1 send at about one range per round trip. A window of ranges fixed 20 and 100 Mbps sends, but a 1 Gbps send took 4.4 s instead of 1.7 s, so the default stayed off. The owner asked to superseed by default only while two or more peers want the bundle.

**Decision:**

- A superseeding origin keeps a window of ranges offered to each peer. It starts at one range and grows by one each time the peer gains chunks while the origin serves no other peer; while it does, it shrinks by one, to one. `upload.superseed_window` caps it.
- `strategies.superseed` is `off`, `on` or `crowd`, and defaults to `crowd`. It applies to a session that starts with every chunk, so a download never superseeds, and ends as before (D-155) when every chunk has been sent once or another complete peer connects.
- In `crowd` mode the origin offers ranges only while two or more connected peers lack chunks. A connection counts from when it opens, until its peer is known to have every chunk.
- A connection that opens while no other peer lacks chunks is announced every chunk in the opening Have, as without superseeding, so a 1:1 send behaves as round-robin. Chunks announced are never hidden again, so the switch is one-way per connection:
  - When a second peer opens, the first keeps everything it was shown. The newcomers are offered ranges, of chunks nobody has been sent, as in `on` mode. A chunk the first peer requests counts as sent, so the newcomers aren't offered it afterwards.
  - When peers leave or complete and at most one lacks chunks, every remaining peer is announced everything not yet announced to it, and its offered ranges lapse.
  - If a second peer then returns, only new connections are hidden from.
- `on` always hides, as before.

This revises D-063 and D-155.

**Simulation:** paired runs, 3 seeds each, of [`superseed-crowd`](../crates/sim/scenarios/experiments/superseed-crowd.yaml) and [`superseed-direct`](../crates/sim/scenarios/experiments/superseed-direct.yaml). `on` has the default window. Times are medians per run, averaged over the seeds. Round-robin is `off`.

| Scenario | Round-robin | Superseeding on | Crowd mode |
| --- | --- | --- | --- |
| Crowd of 100 at once: median time to complete | 132 s | 61 s | 63 s |
| Same: copies the origin uploaded | 2.51 | 1.04 | 1.08 |
| Crowd of 100 over ~1 min: median time to complete | 106 s | 28 s | 29 s |
| 1:1, 64 MiB, 100 ms apart, 20 Mbps upload | 40.0 s | 40.2 s | 40.0 s |
| Same at 100 Mbps | 6.6 s | 6.5 s | 6.6 s |
| Same at 1 Gbps | 1.7 s | 4.4 s | 1.7 s |

The 1:1 runs in crowd mode are identical to round-robin, since the downloader is the only peer. The crowds keep 97% and 98% of superseeding's time saving. The gradual crowd's origin keeps seeding for six minutes, so its copies uploaded are not comparable.

The cost is a crowd whose origin is not the bottleneck: the golden `crowd` scenario (40 peers, 4 MiB, origin at twice a peer's upload) had a median completion of 9.6 s with `off`, 11.5 s with `on` and 11.3 s with `crowd`, and the rarest chunk had 27, 5 and 10 copies, because peers have fewer chunks to trade while the origin hides the rest. Its baseline and the other golden baselines were regenerated for the new default (`mixed` and `routers` moved within noise, `strategic` improved, and `sessions` completed 26 peers at the median seed against 27).

A core swarm test of 8 MiB over 40 Mbps and a 200 ms round trip took 3.2 s with the window against 2.7 s without superseeding, mostly while the window grows. Raising the cap shortens the gigabit send (64 ranges: 2.3 s) but slows the 100 Mbps one by about 12%, apparently through the downloader's pipelining, whose requests in flight settle lower. Variants that grew the window faster or started it larger let early arrivals in the gradual crowd hold ranges that others could have been sent, and lost much of that crowd's saving.

**Rationale:** Sending each chunk out of the origin about once is the classic result this tool should demonstrate, and a lone downloader gains nothing from hiding chunks. Counting peers that lack chunks needs no new messages and no timing, and never has to un-announce a chunk. The first peer of a crowd is shown everything, which costs little: what it requests counts as sent.

---

## D-224: One set of sharing settings, and a farewell peer list

**Date:** 2026-10-07 · **Status:** Decided

**Context:** Two mechanisms decided when a finished download stops sharing: the core's lingering (`settings.lingering`, ratio 1.0 or ten idle minutes, D-065) and the engine's sharing limits (`sharing.*`, D-184). A downloader that knew the swarm only through peer exchange could be stranded when its sources left.

**Decision:**

- The engine's `sharing.*` settings are the only user-facing ones: ratio, time, idle timeout, and the action at the limit. They drive the core's lingering, whose own settings stop being user-facing.
- A peer leaving a swarm sends its known peers in a final Peers message before Goodbye.
- Standalone `tempest get` still exits when its download completes (D-175).
- The engine gives the session only the idle timeout and counts the ratio and the time across runs itself. When a session stops on its own, the sharing action applies, so `remove` works for the idle timeout too.
- A share starts with no limits, so a link keeps working (D-156, D-220); only limits set on the share apply. Session settings under `lingering.` are refused.
- The farewell list holds up to 50 peers, connected ones first, then other known peers, and is exempt from the once-a-minute limit. A peer that is leaving finishes its control stream and waits, within a bound, for the acknowledgement before it closes, since QUIC drops unsent data on a close. A finished control stream is the start of a Goodbye.
- A download that is already complete when the engine restarts starts its session as complete, so only the ratio and time apply to it.

This revises D-065, D-151, D-156 and D-184. The owner confirmed the last four points after implementation, including the farewell list's overhead: about 920 KB to 1.18 MB of control traffic per run in the `routers` baseline.

**Rationale:** Two ratio settings with different defaults contradict conceptual clarity. The farewell list fixes stranding with one message.

---

## D-225: Catalog operator tools

**Date:** 2026-10-07 · **Status:** Decided

**Context:** A bundle ID doesn't identify an author, so the first key to list a bundle keeps its listing (D-206). Operators are responsible for what their catalog lists (`specs/tracker.md`) but had no tool to remove a listing short of editing the database. D-031 deferred moderation until the catalog existed.

**Options:** keep first come with no tools; operator tools (chosen); one listing per publisher, side by side; author keys in manifests.

**Decision:**

- `tempest tracker` gains commands to hide a listing and to ban a key, which also hides its listings, and to undo either.
- Each listing page has a Report link. Reports are recorded for the operator; nothing happens automatically.
- First come still owns a listing. Revisit if squatting appears.
- The commands are `tempest tracker hide`, `unhide`, `ban`, `unban` and `reports`, each taking `--catalog FILE`. They change the catalog database directly, as `tempest tracker invite` does, so they work with the tracker stopped or running, and need no operator login or route. The running tracker checks the database when it serves, so a change applies at once, search facets included.
- A hidden listing is treated as missing for everyone but its publisher, and so is every listing of a banned key. A ban is a flag on the key's account, so unbanning restores its listings, except those hidden on their own. A banned key can't list, edit, vote, comment or report, and its announces list nothing.
- A report is a signed-in key's reason for a listing. Each key has one report per listing; a later one replaces it. Reports have their own per-IP limit, `report` (D-217).

**Rationale:** Operators carry the risk and need the minimum tools to act. Side-by-side listings and author keys are larger changes to make only if squatting turns out to be a problem.

---

## D-226: Notifications are the command plus a notice in each UI

**Date:** 2026-10-07 · **Status:** Decided

**Context:** D-118 put notifications in scope and "run a command on completion" out. D-184 implemented notifications as exactly that command. The web UI shows notices; the TUI showed none on completion.

**Decision:** The engine's only notification mechanism stays `notifications.command`. The TUI shows a notice when a transfer completes or errors, as the web UI does. This deliberately brings the command into scope, revising D-118.

**Rationale:** One hook covers desktop notifications, chat and scripts without platform code. Both UIs should tell you the same things.

---

## D-227: Network defaults stay on and are explained

**Date:** 2026-10-07 · **Status:** Decided

**Context:** Port mapping (D-188), relays chosen by trackers (D-186) and LAN discovery (D-178) are on by default. Each reaches beyond the machine: the router opens a port, a tracker's relay sees who talks to whom, and LAN peers see which bundles you ask about.

**Decision:** All three stay on by default. Settings shows each as a switch with a plain sentence on what it does and what it reveals, and the first run says the same once.

**Rationale:** These make direct sends work behind home routers, which D-029's "no default relays" depends on. Saying plainly what they share is the honest cost (D-002).

---

## D-228: When commands use the daemon

**Date:** 2026-10-07 · **Status:** Decided

**Context:** `share` and `get` hand their transfer to a running daemon, but options it couldn't take, such as `--tracker` or `--created`, silently ran them standalone instead (D-175, D-182). A TUI or web UI that started its own engine stopped every share on quitting.

**Decision:**

- The daemon's API accepts the per-transfer options `share` and `get` can give, such as trackers and the creation time. Only `--standalone` and options that belong to a separate process (its port, identity, binding, relay and LAN settings) run standalone.
- Quitting a TUI or web UI that runs its own engine while shares are running asks first: hand them to a daemon, or stop them.

This revises D-175, D-182 and D-189.

**Rationale:** Which process runs a transfer should follow from what you asked for, not from unrelated flags. Closing a window shouldn't silently break links you've sent.

---

## D-229: Incentives under the default upload strategy are accepted as they are

**Date:** 2026-10-07 · **Status:** Decided

**Context:** After D-170, free riders finish about 15% later than contributors under the default BitTorrent strategy, and strategic clients about 13% sooner than honest ones.

**Decision:** Keep the default strategy and these results. D-154, D-166 and D-170 stand, including the protocol's allowance to drop pending requests from a peer it stops serving.

**Rationale:** Weak incentives are a known property of BitTorrent, and the simulator can show them. Other strategies remain available to compare.

---

## D-230: Experiments can vary peer-class fields

**Date:** 2026-10-07 · **Status:** Decided

**Context:** `experiment.vary` takes only session settings, which apply to every peer (D-157). Comparing one class's behavior, such as the origin superseeding (D-223) or the share of strategic peers, needed separate scenario files, losing the pairing of runs on the same seeds.

**Decision:**

- `vary` can address a peer class's fields, such as `peers.origin.settings.strategies.superseed` or a class's `count`.
- Simulator-only protocol variants (D-004) wait until an experiment needs one.

**Rationale:** Paired comparisons of one class are what D-223's check and incentive studies need.

---

## D-231: Settings changed through the API win, and the daemon says so

**Date:** 2026-10-07 · **Status:** Decided

**Context:** Settings changed through the API are stored apart from the config file and apply over it (D-181, D-195), so a later edit to the same key in the file is ignored. The UIs mark overridden keys.

**Options:** keep it, with a log; rewrite the file on API changes; let a newer file edit win.

**Decision:** Keep it. At start, the daemon logs each config-file key that a stored setting overrides, and how to reset it.

**Rationale:** The least machinery, and the log turns a silent surprise into a visible fact.

---

## D-232: Release artifacts can be built locally

**Date:** 2026-10-08 · **Status:** Decided

**Context:** GitHub Actions was unavailable (billing), and releases (D-211) were built only there. Dist can drive cross compilers itself.

**Decision:**

- A release can be built on one Mac with `dist build`, using the system linker for both macOS targets, `cargo-zigbuild` for the Linux targets and `cargo-xwin` for Windows. Steps and tools are in `specs/development.md`.
- Windows `tempest` builds this way. The `.msi` installers can't be built, since they need WiX on Windows; they come from CI. (Windows `tempest-sim` didn't cross-compile either, because clang-cl rejects DuckDB's C++, which D-233 makes moot.)
- Linux binaries are cross-built and not run locally.
- A local build publishes nothing; tagging, the GitHub release and the Homebrew tap stay separate steps.

**Rationale:** The workflow file and the dist configuration stay the single source of what a release contains; the local build only supplies the machine.

---

## D-233: Releases ship only `tempest`

**Date:** 2026-10-08 · **Status:** Decided

**Context:** Releasing `tempest-sim` (D-214) meant compiling DuckDB's C++ for every platform, which took most of a local release's hour, and it doesn't cross-compile for Windows (D-232).

**Options:** ship it and cache the C++ compile with sccache; link DuckDB's prebuilt dynamic libraries and ship them beside the binary; don't ship it (chosen).

**Decision:** Releases contain only `tempest`. `tempest-sim` is built from source (`specs/development.md`), where development links the system DuckDB. This refines D-211.

**Rationale:** The simulator is a development and research tool; its users build Tempest anyway. Releases get faster and simpler, with no C++ toolchain per platform.

---

## D-234: The tracker is the service; the catalog is its internals

**Date:** 2026-10-08 · **Status:** Decided

**Context:** "Tracker" and "catalog" named overlapping things. The client had a Catalog area for trackers and a placeholder for browsing, the CLI had `tempest catalog`, the HTTP paths said `/catalog/v1`, and the shared API crate and the tracker's website said catalog. D-025, D-139, D-196, D-203, D-206, D-208 and D-210 introduced those names.

**Options:** keep "catalog" as the name of the listing service beside the tracker; make the client browse catalogs; make the tracker the one service and keep "catalog" for how it stores things (chosen).

**Decision:**

- The tracker is the service. Clients announce to a tracker, publish bundles to it and sign in to it. A tracker may have a website where people browse its listings.
- "Catalog" is internal to a tracker: where it stores its listings, accounts, votes, comments, reports and search index. It appears only in the tracker crate's catalog module and storage, the operator flag `tempest tracker --catalog FILE`, the operator commands' help (`tempest tracker hide|unhide|ban|unban|reports|invite`) and the operator docs (`specs/tracker.md`).
- The line sits at the tracker's public interface. Nothing a client or visitor touches says "catalog": client UI text, the tracker's website, CLI client commands, HTTP paths, the shared API types crate and the client's networking code. There are no compatibility shims or aliases (Tempest has not launched).
- The client, meaning the TUI and the web UI, is only for transfers. It never browses listings; that happens on a tracker's website.

**Changes:**

- The client UIs have three areas: Transfers, Share and Settings. The tracker list (the trackers every transfer announces to) and the relays moved into Settings, after the network settings. A transfer is "Published to" trackers and can be unpublished from them.
- `tempest catalog publish|unpublish|sign-in` became `tempest publish`, `tempest unpublish` and `tempest sign-in`. Publishing to a tracker without listings fails with "the tracker doesn't take listings".
- The tracker's listing API moved under the peer-list service's prefix, `/tempest/v1/`: `session/challenges`, `session/challenges/poll`, `session` (POST signs in, DELETE signs out), `account`, `account/listings`, `info`, `listings`, `listings/{bundle}` with `vote`, `comments` and `reports`, and `comments/{id}`. The session cookie is `tempest_session`, and the sign-in signature's context is `tempest sign-in v1`.
- The engine's `POST /v1/catalog/sign-in` became `POST /v1/trackers/sign-in`.
- `catalog-api` became `tracker-api`, `catalog-web` became `tracker-web`, and `tempest-net`'s `catalog` module became `publish`. The website's header reads "Tracker" and the host.

**Rationale:** People meet a tracker, not a catalog. One name for the service keeps every interface describing the same thing, and "catalog" stays useful for operators, who run a database file.

---

## D-235: One integration-test binary per crate

**Date:** 2026-10-08 · **Status:** Decided

**Context:** Each file in a crate's `tests/` was its own test target, so a crate with eight files linked eight executables, each with the crate and its dependencies. Builds spent much of their time linking and `target/` grew with the stale copies. The owner found development slow because so much gets built.

**Options:** keep a file per target, which gives one filterable binary per topic; merge each crate's files into one target (chosen).

**Decision:** Each crate with several integration test files has a single target, `tests/integration/main.rs`, with a module per former file and the shared helpers as a sibling `common` module. The target name is `integration` in every crate. The former file names remain as module names, so tests read `integration module::test`. This reverses the earlier default of one target per file. The UI kit's single test file and the dev-profile settings are unchanged.

**Consequences:**

- The terminal UI's insta snapshots are named after the module path and were renamed to `integration__screens__*`.
- Plain `cargo test` runs a binary's tests in one process on threads, so tests must not rely on a process of their own (`specs/development.md`). None did: no test sets environment variables, changes directories, or binds a fixed port.
- CI selects browser and web tests with a module filter on `--test integration`.

**Rationale:** Linking once per crate saves link time and disk. Test names stay readable and filterable by module, and nextest still isolates each test in its own process.

---

## D-236: The tracker and the relay are their own binaries

**Date:** 2026-10-08 · **Status:** Decided

**Context:** D-079 put everything but the simulator (D-214) in one `tempest` binary. That binary carried the tracker with its Tantivy index, SQLite catalog and website, and the relay server, which almost nobody who downloads the client runs. The owner decided the tracker and the relay should be their own programs, and that nothing else is split: the client keeps the engine inside, so one command opens the TUI or the web UI.

**Options:** keep everything in `tempest`; split the tracker and relay out (chosen); also split the engine and the UIs from the CLI.

**Decision:** Four binaries, each with a package of its own:

| Binary | Package | For | Contains |
| --- | --- | --- | --- |
| `tempest` | `tempest` | everyone | CLI, engine and daemon, TUI, client web UI |
| `tempest-tracker` | `tempest-tracker-cli` (`crates/tracker-cli`) | tracker operators | peer lists, the catalog and search, the tracker website, operator commands |
| `tempest-relay` | `tempest-relay-cli` (`crates/relay-cli`) | relay operators | the relay server |
| `tempest-sim` | `tempest-sim-cli` | researchers | the simulator (D-214) |

- `tempest-tracker [OPTIONS]` serves; `tempest-tracker invite|hide|unhide|ban|unban|reports --catalog FILE` are the operator commands (D-225, D-234). The options and the help text are those of the former `tempest tracker`, except `--run-relay`. `tempest-relay [OPTIONS]` is the former `tempest relay`.
- A tracker no longer runs a relay (D-185). `--run-relay`, `--relay-port`, `--relay-public-ip` and `--relay-identity` are gone. The tracker's existing `--relay '<key>@host[:port]'` names a relay run separately, which it recommends to clients as before (D-186); `tempest-relay` prints that identity on startup.
- The `tempest` binary depends on neither `tempest-tracker` nor `tempest-relay`, so Tantivy, the catalog's SQLite use and the tracker website are no longer in it. It keeps the clients of both in `tempest-net`, and the shared types in `tracker-api`. `tempest tracker` and `tempest relay` are hidden commands that fail with a message naming the new program, as `tempest sim` does.
- The tracker website is embedded into `tempest-tracker` by its build script, through `tempest-assets`, and the client web UI into `tempest` only.
- The per-user data folder lookup moved from `tempest-engine` to `tempest-net`, so the relay finds its default key file without the engine.
- Tests that start a tracker or relay as a process (publishing, the daemon, transfers, the tracker website in a browser) stay in `crates/tempest`: they drive the client. Cargo builds only a package's own binaries, so a test helper builds `tempest-tracker` or `tempest-relay` on first use with `cargo build -p` and runs it from the same target folder. The operator commands, the website being served and each program's command line are tested in its own crate.
- `ListingStatus::Withdrawn` is now `Unpublished` in the API (D-234's term); the catalog still stores the value as `withdrawn`.

**Releases:** both new packages set `dist = false`, so releases stay `tempest` only, as in D-233. **Open question for the owner:** whether releases should ship `tempest-tracker` and `tempest-relay`, for example for Linux and macOS. Shipping the tracker means building its website and compiling SQLite for each target.

**Rationale:** Operators get a program that matches their job, and everyone else gets a smaller client that builds faster. Keeping the engine in `tempest` means the common case stays one command.

## D-237: The engine opens a transfer's folder for the web UI

**Date:** 2026-10-08 · **Status:** Decided

**Context:** The TUI opens a download's folder with the system's file manager (`o`), since it runs on the engine's machine. A browser can't open local folders, and the web UI showed only the folder's path.

**Options:** the web UI shows the path only; a `file:` link (browsers block it); the engine opens the folder on request (chosen).

**Decision:**

- `POST /v1/transfers/{id}/reveal` makes the engine show the transfer's files in its machine's file manager: for a bundle of one file, that file selected (macOS `open -R`, Windows `explorer /select,`, and on Linux its folder), otherwise the folder. It answers with the path.
- Only a local engine does it (D-125). Remote access answers 403 `local_only`, since the browser's screen is not the engine's, even with a valid token. Nothing saved yet, or a missing folder, answers 409 `no_files`.
- The opener is `tempest_engine::reveal`. The TUI's `o` now calls the route instead of opening the folder itself, so both select a single file. The router takes the opener as a parameter so tests record paths instead of starting a file manager.
- The web UI's folder row has an open button, labelled "Show in Finder" on macOS, "Show in Explorer" on Windows and "Open folder" elsewhere by the browser's platform (a browser that can use the button is on the engine's machine), and a "Copy path" button that works in any mode. In remote mode the open button is disabled with a tooltip that says why. The key is `o`, as in the TUI.

**Rationale:** The engine is the only process that can open a window on its own machine, and the local-only rule keeps a remote client from opening windows on someone else's screen.

---

## D-238: One hardened HTTP server for the tracker and the engine API

**Date:** 2026-10-08 · **Status:** Decided

**Context:** The tracker served its routes through hyper with a connection cap and timeouts on headers and requests. The engine's API used `axum::serve`, which has none of them, and completed TLS handshakes in tasks of its own without a cap. With remote access on (D-125), anyone who can reach the port could hold connections open.

**Options:** copy the tracker's serving code into the engine; move it into `tempest-net`, which both use; a small crate of its own (chosen).

**Decision:** `tempest-http` serves an axum router over HTTP/1.1 with a cap on open connections, which counts connections still in their handshake, a timeout on a connection's handshake and on each request's headers, and a timeout from a request's headers to its response's headers, answered with 408 in the JSON error shape both APIs share. `serve_with` takes a handshake, so the engine runs TLS through it. The engine's API uses it with limits of its own: a request timeout long enough for slow tracker calls, and no limit on how long an event stream lasts, since the timeout ends at the response's headers. Shutdown waits a short grace period for responses in progress instead of every one.

**Rationale:** One implementation of the limits, tested once. `tempest-net` is the client's networking, and a server module would bring hyper's server into everything that uses it.
