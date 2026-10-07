# Tempest

Tempest is a peer-to-peer file sharing protocol and client, written in Rust. It is built on BitTorrent's ideas but is not compatible with BitTorrent.

It aims to be easy to understand first, configurable and measurable second, fast third, and pleasant to use throughout.

## How it works

- You share a **bundle**: one or more files.
- Each file is split into 16 KiB **chunks**, which are hashed into a Merkle tree. A file's root hash identifies its contents, and each chunk can be verified the moment it arrives.
- A **manifest** lists the bundle's files and their roots. Its hash is the **bundle ID**, which appears in `tempest:` links.
- Peers exchanging the same bundle form a **swarm**. They find each other through links, trackers, the local network and peer exchange, and connect over encrypted QUIC.
- Relays help peers behind home routers reach each other.

The full protocol is in [`protocol/`](protocol/README.md).

## Status

In design. The protocol spec is drafted, and implementation starts with milestone M1 ([roadmap](specs/roadmap.md)). Nothing is installable yet.

## Try the prototypes

The UI prototypes run on simulated data and need a Rust toolchain.

```sh
# Terminal UI
cd prototypes/tui && cargo run --release

# Web UI (Leptos), needs trunk and the wasm32-unknown-unknown target
cd prototypes/ui/leptos-app && trunk serve --port 8081
```

## Repository

| Path | Contents |
| --- | --- |
| [`protocol/`](protocol/README.md) | The Tempest protocol specification |
| [`specs/`](specs/index.md) | Project goals, decisions, architecture and plans |
| [`prototypes/`](prototypes/) | Throwaway UI prototypes |
