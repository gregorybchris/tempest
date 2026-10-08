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
| `net` | QUIC driver (quinn), port mapping, relay client, LAN discovery, tracker and catalog client, manifest fetching | `protocol`, `core`, `catalog-api` |
| `engine` | Daemon: bundles, SQLite state, config and profiles, HTTP API and events | `core`, `net`, `api`, `catalog-api` |
| `api` | The engine API's types, and what clients make of them: the event-following model, finding, columns, explanations, the settings they show and how values are typed (D-200) | `protocol`, `catalog-api` |
| `sim` | Discrete-event simulator, scenarios, DuckDB results and the queries the results UI reads (D-204) | `core`, `sim-api` |
| `sim-api` | The simulation results API's types and query strings, shared by its server and the results UI (D-204) | (none) |
| `ui-kit` | Shared design system for the web UIs (D-076, D-198) | (none) |
| `ui-gallery` | A catalog of the design system, and the browser benchmark of its live views (D-199) | `ui-kit` |
| `web` | Client web UI | `ui-kit`, `api` |
| `tui` | Terminal UI | `api`, `engine` (its API client) |
| `sim-web` | Simulation results UI | `ui-kit`, `sim-api` |
| `catalog-api` | The tracker catalog API's types and text rules, shared by the tracker, the client and the catalog UI (D-206) | `protocol` |
| `tracker` | Tracker server: peer-list service, catalog with search and ranking, fetching listed bundles' manifests (D-207, D-209) | `protocol`, `catalog-api`, `net`, `ui-kit` |
| `relay` | Relay library (the binary is `tempest relay`) | `protocol`, `net` (D-185) |
| `tempest` | The `tempest` binary: CLI subcommands, launches engine, TUI, web UI and the results UI; embeds the web UIs' built files (D-201) | most of the above |

`protocol` and `core` must never depend on async runtimes, sockets, the clock or global randomness. `protocol` is the code other implementers should be able to read alongside the spec.

## Technology choices

| Concern | Choice | Decision |
| --- | --- | --- |
| Async runtime | tokio (multi-threaded) | D-084 |
| Transport | QUIC via quinn, TLS 1.3 raw public keys via rustls | D-018, D-020 |
| Port mapping | PCP and NAT-PMP by hand, UPnP-IGD via igd-next | D-188 |
| Hashing | SHA-256 | D-008 |
| Disk I/O | Positioned reads/writes on a thread pool, batched syncs | D-086 |
| Client state | SQLite (rusqlite, bundled) | D-082 |
| Client config | YAML, with profiles + overrides | D-005, D-081 |
| Engine API | HTTP + JSON, server-sent events with resumable IDs | D-080 |
| Tracker API | HTTPS + JSON | D-043 |
| Tracker catalog | SQLite store, Tantivy search index in memory | D-206, D-130, D-209 |
| Simulation results | DuckDB | D-075 |
| Web UIs | Leptos 0.8 (client-side, with server rendering and islands where useful) | D-117 |

## Open questions

- Release tooling. (Testing and CI: D-123, D-132, D-141.)
- Error handling, logging and metrics conventions.
