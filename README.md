<p align="center"><img src="branding/banner.png" alt="Tempest" width="800"></p>

# Tempest

[![CI](https://github.com/gregorybchris/tempest/actions/workflows/ci.yml/badge.svg)](https://github.com/gregorybchris/tempest/actions/workflows/ci.yml)

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

## Install

Releases are on [GitHub](https://github.com/gregorybchris/tempest/releases/latest).

**Client** (`tempest`), for macOS, Linux and Windows:

```sh
# macOS and Linux
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/gregorybchris/tempest/releases/latest/download/tempest-installer.sh | sh

# Windows (PowerShell)
powershell -ExecutionPolicy Bypass -c "irm https://github.com/gregorybchris/tempest/releases/latest/download/tempest-installer.ps1 | iex"
```

**Tracker** (`tempest-tracker`) and **relay** (`tempest-relay`), for the same platforms. The Windows installers are `tempest-tracker-cli-installer.ps1` and `tempest-relay-cli-installer.ps1`:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/gregorybchris/tempest/releases/latest/download/tempest-tracker-cli-installer.sh | sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/gregorybchris/tempest/releases/latest/download/tempest-relay-cli-installer.sh | sh
```

See [running a tracker](specs/tracker.md#running-one) and [relays](protocol/relays.md).

**From source:** build the web UIs below (optional), then `cargo install --path crates/tempest`. The other programs install the same way. The simulator is only built from source:

```sh
cargo install --path crates/tracker-cli   # tempest-tracker, for tracker operators (build crates/tracker-web first)
cargo install --path crates/relay-cli     # tempest-relay, for relay operators
cargo install --path crates/sim-cli --features bundled-duckdb   # tempest-sim, a development tool
```

The simulator can link an installed DuckDB instead of the feature ([development](specs/development.md)).

## Usage

```sh
(cd crates/web && trunk build)         # the web UI, to embed (optional)
(cd crates/sim-web && trunk build)     # the results UI, likewise
(cd crates/tracker-web && trunk build) # the tracker website, likewise
# `trunk serve` iterates on a UI, and releases use `--release --cargo-profile web`: see specs/development.md

tempest share ./photos   # prints a tempest: link, then shares until Ctrl-C
tempest get '<link>'     # downloads the bundle into ./<its name>

tempest-tracker                                   # runs a tracker
tempest share --tracker http://host:4777 ./photos # announces the bundle to it
tempest-tracker --catalog catalog.db              # with listings and a website at its address
tempest share --publish http://host:4777 ./data   # lists the bundle on that tracker
tempest-relay                                     # runs a relay for peers behind routers, and prints its <key>@host:port
tempest share --relay '<key>@host:4775' ./photos  # reachable through that relay too
tempest-tracker --relay '<key>@host:4775'         # a tracker recommends a relay to its peers

tempest daemon           # runs the engine, which keeps transfers going and serves its API
tempest list             # shows its transfers; start, stop and remove control them
tempest tui              # the terminal UI, on the daemon or an engine of its own
tempest web              # the web UI in your browser, likewise

tempest-sim run crates/sim/scenarios/flash-crowd.yaml   # simulates a swarm, into tempest-sim.duckdb
tempest-sim web                                         # browses and compares the results
```

Building the web UIs needs [trunk](https://trunk-rs.github.io/trunk/); a binary built without them serves a page that says so. In the TUI and the web UI, `1`-`3` switch between Transfers, Share and Settings, `:` finds any command (TUI), and `?` lists the keys of the screen you are on.

While a daemon runs, `share` and `get` hand their transfers to it. Peers on the same local network also find each other without a tracker, and each peer asks its router to forward its port with PCP, NAT-PMP or UPnP-IGD; `share` and `get` turn these off with `--no-lan` and `--no-port-mapping`. `tempest <command> --help`, `tempest-tracker --help` and `tempest-relay --help` list each command's options. The engine's API is described in [`specs/api.md`](specs/api.md). Running a public tracker: see [`specs/tracker.md`](specs/tracker.md).

## Repository

| Path | Contents |
| --- | --- |
| [`protocol/`](protocol/README.md) | The Tempest protocol specification |
| [`specs/`](specs/index.md) | Project goals, decisions, architecture and plans |
