# Tempest

Tempest is a peer-to-peer file sharing protocol and client, written in Rust. It is built on BitTorrent's ideas but is not compatible with BitTorrent.

It aims to be easy to understand first, configurable and measurable second, fast third, and pleasant to use throughout.

## How it works

- You share a **bundle**: one or more files.
- Each file is split into small **chunks**, which are hashed into a Merkle tree. A file's root hash identifies its contents, and each chunk can be verified the moment it arrives.
- A **manifest** lists the bundle's files and their roots. Its hash is the **bundle ID**, which appears in `tempest:` links.
- Peers exchanging the same bundle form a **swarm**. They find each other through links, trackers, the local network and peer exchange, and connect over encrypted QUIC.
- Relays help peers behind home routers reach each other.

The full protocol is in [`protocol/`](protocol/README.md).

## Status

In development. See the [roadmap](specs/roadmap.md).

## Usage

```sh
cargo install --path crates/tempest

tempest share ./photos   # prints a tempest: link, then shares until Ctrl-C
tempest get '<link>'     # downloads the bundle into ./<its name>

tempest tracker                                   # runs a tracker
tempest share --tracker http://host:4777 ./photos # announces the bundle to it

tempest daemon           # runs the engine, which keeps transfers going and serves its API
tempest list             # shows its transfers; start, stop and remove control them
```

While a daemon runs, `share` and `get` hand their transfers to it. Peers on the same local network also find each other without a tracker; `--no-lan` turns that off. `tempest <command> --help` lists each command's options. The engine's API is described in [`specs/api.md`](specs/api.md).

## Repository

| Path | Contents |
| --- | --- |
| [`protocol/`](protocol/README.md) | The Tempest protocol specification |
| [`specs/`](specs/index.md) | Project goals, decisions, architecture and plans |
