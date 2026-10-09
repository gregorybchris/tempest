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
- **Engine + API (D-079, D-080).** One background engine per machine. The web UI, TUI and CLI are clients of its HTTP + JSON API and its live event stream. The client, with the engine inside, ships as one `tempest` binary with subcommands. Servers and tools are separate binaries: `tempest-tracker` and `tempest-relay` (D-236) and the simulator, `tempest-sim` (D-214). The TUI and CLI can start an embedded engine when none is running.
- **Actors (D-085).** Inside the engine, each long-lived thing (a bundle, a connection, the disk writer) is a tokio task that owns its state and talks through bounded channels. Nothing is shared behind locks.

## Crates (D-083)

| Crate | Contents | May depend on |
| --- | --- | --- |
| `protocol` | Wire types, encoding and decoding, chunk tree, proofs, manifest, links | (minimal: hashing, Unicode normalization, serde for tracker JSON) |
| `core` | Sans-IO peer and bundle logic, strategies, decision events | (none) |
| `net` | QUIC driver (quinn), port mapping, relay client, LAN discovery, tracker client (announces, sign-in, listings), manifest fetching | `core`, `tracker-api`, `http` |
| `engine` | Daemon: bundles, SQLite state, config and profiles, HTTP API and events | `core`, `net`, `api`, `tracker-api` |
| `api` | The engine API's types, and what clients make of them: the event-following model, finding, columns, explanations, the settings they show and how values are typed (D-200); with its `client` feature, the native API client (D-245) | `tracker-api`; `http` for the client |
| `http` | The HTTPS client setup the tracker client and the API client share: TLS, certificate trust, user agent (D-245) | (none) |
| `sim` | Discrete-event simulator, scenarios, DuckDB results and the queries the results UI reads (D-204) | `core`, `sim-api` |
| `sim-api` | The simulation results API's types and query strings, shared by its server and the results UI (D-204) | (none) |
| `ui-kit` | Shared design system for the web UIs (D-076, D-198), and the request layer and app shell they share (D-243) | (none) |
| `ui-gallery` | A gallery of the design system, and the browser benchmark of its live views (D-199) | `ui-kit` |
| `web` | Client web UI | `ui-kit`, `api` |
| `tui` | Terminal UI | `api` (its client) |
| `sim-web` | Simulation results UI, served by `tempest-sim web` | `ui-kit`, `sim-api` |
| `tracker-api` | The types and text rules of a tracker's listing API, shared by the tracker, the client and the tracker website (D-206, D-234) | (none) |
| `tracker-web` | The tracker website (D-210, D-234) | `ui-kit`, `tracker-api` |
| `tracker` | Tracker server: peer-list service, and a catalog (its internal store) with search and ranking, fetching listed bundles' manifests (D-207, D-209) | `tracker-api`, `net` |
| `relay` | Relay library | `net` (D-185) |
| `assets` | Serving embedded web UIs' files, and embedding them from build scripts (D-201); shared by the binaries that embed a UI (D-214, D-236) | (none) |
| `tempest` | The `tempest` binary: CLI subcommands, launches engine, TUI and web UI; embeds the client web UI's built files (D-201). It has no tracker or relay server, only their clients in `net` (D-236) | the client's crates, not `sim`, `tracker` or `relay` |
| `tracker-cli` | The `tempest-tracker` binary (package `tempest-tracker-cli`): runs a tracker, with its catalog and website, which it embeds, and the operator commands (D-236) | `tracker`, `tracker-api`, `net`, `assets` |
| `relay-cli` | The `tempest-relay` binary (package `tempest-relay-cli`): runs a relay (D-236) | `relay`, `net` |
| `sim-cli` | The `tempest-sim` binary (package `tempest-sim-cli`): runs scenarios, checks baselines and serves the results UI, which it embeds (D-214) | `sim`, `sim-api`, `assets` |

The last column lists the workspace crates each may use besides `protocol`, which any crate may use.

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
| Tracker's catalog | SQLite store, Tantivy search index in memory | D-206, D-130, D-209 |
| Simulation results | DuckDB | D-075 |
| Web UIs | Leptos 0.8 (client-side, with server rendering and islands where useful) | D-117 |

## Open questions

- Error handling, logging and metrics conventions.
