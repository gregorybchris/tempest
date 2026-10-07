# Glossary

Tempest's terms, as decided. Terms not yet named are listed at the bottom with their working placeholder. See [decisions.md](decisions.md) for the reasoning behind each name.

| Term | Meaning | BitTorrent equivalent |
| --- | --- | --- |
| **bundle** | One or more files (plus optional symlinks and empty directories) offered together | torrent |
| **bundle ID** | The identifier of a bundle, computed from its manifest | info hash |
| **chunk** | A 16 KiB unit of a file; the unit that is requested, sent and verified | block (inside a piece) |
| **chunk tree** | The Merkle tree built from a file's chunks | piece / merkle tree |
| **complete / incomplete** | State of a peer that has / doesn't have the whole bundle | seeder / leecher |
| **entry** | One item in a manifest: a file, symlink or empty directory | file entry |
| **Have / HaveUpdate** | Messages listing every chunk range a peer has / ranges added or removed | bitfield, have / have, lt_donthave |
| **manifest** | The document describing a bundle: its name, metadata, and every entry with its root | .torrent file / info dictionary |
| **peer** | Another client participating in a swarm | peer |
| **peer ID** | A peer's Ed25519 public key, which identifies it | peer id (20 random bytes) |
| **proof** | The sibling hashes needed to verify a chunk against its root | hash proof / uncle hashes |
| **provider / requester** | Roles in a single exchange: the requester asks for a chunk and the provider sends it. Every peer plays both roles | (none; BitTorrent uses uploader / downloader informally) |
| **Reject** | The answer to a request a provider won't fulfil, with a reason | reject (BEP 6) |
| **relay** | A public server that forwards encrypted traffic between two peers that can't connect directly, and coordinates hole punching between them. A peer's **home relay** is the one it stays connected to and advertises | (none in BitTorrent; BEP 55 uses peers) |
| **root** | The top hash of a file's chunk tree; identifies the file's contents | pieces root (v2) |
| **Serving** | A peer's yes/no signal: "if you send requests now, I intend to answer them with data" | unchoke / choke |
| **swarm** | The peers exchanging the same bundle | swarm |
| **Want** | A peer's yes/no signal: "you have chunks I'd like" | interested / not interested |

Message names are listed in [decisions.md](decisions.md) (D-052); only those that replace BitTorrent concepts appear here.

## Working placeholders (not yet named)

None right now.
