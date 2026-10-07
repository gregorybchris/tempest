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

## Install

Prebuilt binaries for macOS, Linux and Windows are on the [releases page](https://github.com/gregorybchris/tempest/releases). Windows also has an MSI.

```sh
# macOS and Linux
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/gregorybchris/tempest/releases/latest/download/tempest-installer.sh | sh

# Homebrew
brew install gregorybchris/tap/tempest

# Windows (PowerShell)
powershell -ExecutionPolicy Bypass -c "irm https://github.com/gregorybchris/tempest/releases/latest/download/tempest-installer.ps1 | iex"
```

To build from source, run `cargo install --path crates/tempest`.

## Usage

The `tempest` binary does not share or download anything yet. Planned commands (`share`, `get`) arrive with [M1 and M4](specs/roadmap.md); run `tempest --help` for what exists.

## Status

In development. See the [roadmap](specs/roadmap.md).

## Repository

| Path | Contents |
| --- | --- |
| [`crates/`](crates) | The Rust workspace |
| [`protocol/`](protocol/README.md) | The Tempest protocol specification |
| [`specs/`](specs/index.md) | Project goals, decisions, architecture and plans |
