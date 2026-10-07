# Architecture

How the Tempest codebase is put together. Decisions referenced as D-### are in [decisions.md](decisions.md).

## Big picture

```text
            ┌──────────┐  ┌──────────┐  ┌──────────┐
            │  web UI  │  │   TUI    │  │   CLI    │      interfaces
            └────┬─────┘  └────┬─────┘  └────┬─────┘
                 └──── HTTP + JSON API, event stream ────┐
                                                         ▼
 ┌────────────────────────────── engine (daemon) ─────────────────────────────┐
 │  config & profiles (YAML) · state (SQLite) · API server                    │
 │                                                                            │
 │   bundle actor ── drives ──► core: sans-IO peer & bundle logic, strategies │
 │        │                           │                                       │
 │   connection actors (QUIC)    disk actor (positioned I/O, thread pool)     │
 │   NAT / relays / LAN / trackers                                            │
 └────────────────────────────────────────────────────────────────────────────┘

 simulator ── drives the same core ──► simulated network · DuckDB results ──► results web UI
```

- **One brain, two drivers (D-068).** All protocol and strategy logic lives in `core` as sans-IO state machines. The engine drives them with real QUIC and real time. The simulator drives them with a simulated network and simulated time.
- **Engine + API (D-079, D-080).** One background engine per machine. The web UI, TUI and CLI are clients of its HTTP + JSON API and its live event stream. Everything ships as one `tempest` binary with subcommands. The TUI and CLI can start an embedded engine when none is running.
- **Actors (D-085).** Inside the engine, each long-lived thing (a bundle, a connection, the disk writer) is a tokio task that owns its state and talks through bounded channels. Nothing is shared behind locks.

## Crates (D-083)

| Crate | Contents | May depend on |
| --- | --- | --- |
| `protocol` | Wire types, encoding and decoding, chunk tree, proofs, manifest, links | (minimal: hashing, Unicode normalization, serde for tracker JSON) |
| `core` | Sans-IO peer and bundle logic, strategies, decision events | `protocol` |
| `net` | QUIC driver (quinn), port mapping, relay client, LAN discovery, tracker client | `protocol`, `core` |
| `engine` | Daemon: bundles, SQLite state, config and profiles, HTTP API and events | `core`, `net` |
| `sim` | Discrete-event simulator, scenarios, DuckDB results | `core` |
| `ui-kit` | Shared design system for the web UIs (D-076) | (none) |
| `web` | Client web UI | `ui-kit` |
| `tui` | Terminal UI | (engine API) |
| `sim-web` | Simulation results UI | `ui-kit` |
| `tracker` | Tracker server: peer-list service, catalog | `protocol`, `ui-kit` |
| `relay` | Relay library and standalone binary | `protocol` |
| `tempest` | The `tempest` binary: CLI subcommands, launches engine, TUI, web UI | most of the above |

`protocol` and `core` must never depend on async runtimes, sockets, the clock or global randomness. `protocol` is the code other implementers should be able to read alongside the spec.

## Technology choices

| Concern | Choice | Decision |
| --- | --- | --- |
| Async runtime | tokio (multi-threaded) | D-084 |
| Transport | QUIC via quinn, TLS 1.3 raw public keys via rustls | D-018, D-020 |
| Hashing | SHA-256 | D-008 |
| Disk I/O | Positioned reads/writes on a thread pool, batched syncs | D-086 |
| Client state | SQLite (rusqlite, bundled) | D-082 |
| Client config | YAML, with profiles + overrides | D-005, D-081 |
| Engine API | HTTP + JSON, server-sent events with resumable IDs | D-080 |
| Tracker API | HTTPS + JSON | D-043 |
| Simulation results | DuckDB | D-075 |
| Web UIs | Leptos 0.8 (client-side, with server rendering and islands where useful) | D-117 |

## Open questions

- Release tooling. (Testing and CI: D-123, D-132, D-141.)
- Error handling, logging and metrics conventions.
