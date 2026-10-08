# Glossary

Tempest's terms, as decided. Terms not yet named are listed at the bottom with their working placeholder. See [decisions.md](decisions.md) for the reasoning behind each name.

| Term | Meaning | BitTorrent equivalent |
| --- | --- | --- |
| **announce** | A peer's request to a tracker, saying it takes part in a bundle's swarm; the tracker replies with other peers | announce |
| **bundle** | One or more files (plus optional symlinks and empty directories) offered together | torrent |
| **bundle ID** | The identifier of a bundle, computed from its manifest | info hash |
| **catalog** | A tracker's own store of its listings, accounts, votes, comments, reports and search index. It is internal to a tracker: it appears only in the tracker's code, `tempest tracker --catalog`, the operator commands and the operator docs, never in what a client or visitor sees | (none) |
| **chunk** | A 16 KiB unit of a file; the unit that is requested, sent and verified | block (inside a piece) |
| **chunk tree** | The Merkle tree built from a file's chunks | piece / merkle tree |
| **complete / incomplete** | State of a peer that has / doesn't have the whole bundle | seeder / leecher |
| **entry** | One item in a manifest: a file, symlink or empty directory | file entry |
| **Have / HaveUpdate** | Messages listing every chunk range a peer has / ranges added or removed | bitfield, have / have, lt_donthave |
| **listing** | A tracker's page for a bundle that its sharer published: title, description, category, tags, votes and comments | (none; trackers list torrents) |
| **manifest** | The document describing a bundle: its name, metadata, and every entry with its root | .torrent file / info dictionary |
| **peer** | Another client participating in a swarm | peer |
| **peer ID** | A peer's Ed25519 public key, which identifies it | peer id (20 random bytes) |
| **proof** | The sibling hashes needed to verify a chunk against its root | hash proof / uncle hashes |
| **provider / requester** | Roles in a single exchange: the requester asks for a chunk and the provider sends it. Every peer plays both roles | (none; BitTorrent uses uploader / downloader informally) |
| **publish** | A sharer's choice to list a bundle on a tracker, with `tempest publish` or `tempest share --publish`. **Unpublish** removes the listing | upload (to a tracker's site) |
| **Reject** | The answer to a request a provider won't fulfil, with a reason | reject (BEP 6) |
| **relay** | A public server that forwards encrypted traffic between two peers that can't connect directly, and coordinates hole punching between them. A peer's **home relay** is the one it stays connected to and advertises | (none in BitTorrent; BEP 55 uses peers) |
| **root** | The top hash of a file's chunk tree; identifies the file's contents | pieces root (v2) |
| **Serving** | A peer's yes/no signal: "if you send requests now, I intend to answer them with data" | unchoke / choke |
| **swarm** | The peers exchanging the same bundle | swarm |
| **tracker** | The service clients announce to, publish bundles to and sign in to. It helps peers in a swarm find each other and may keep listings | tracker |
| **tracker website** | A tracker's web pages, where people search and browse its listings, and, signed in, vote and comment. The client never browses listings; this is where that happens | (a torrent site) |
| **Want** | A peer's yes/no signal: "you have chunks I'd like" | interested / not interested |

**The boundary rule.** The tracker is the service; "catalog" names only how a tracker stores things. Nothing a client or visitor touches says "catalog": the UIs, the tracker's website, the client commands, the HTTP paths, `tracker-api` and the client's networking code say tracker, listing, publish and sign in (D-234).

Message names are listed in [decisions.md](decisions.md) (D-052); only those that replace BitTorrent concepts appear here.

## Working placeholders (not yet named)

None right now.
