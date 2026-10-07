# Roadmap

Concrete milestones for building Tempest (D-092). Each milestone ends with something runnable. The protocol spec in `protocol/` is completed before implementation begins (D-093).

Once the owner is happy with M0, the remaining milestones are implemented with a Claude workflow.

| # | Milestone | Done when you can… |
| --- | --- | --- |
| **M0** | Rust UI quality prototype | Compare Leptos, Dioxus and server-rendered axum + Datastar/htmx in four palettes (green phosphor, amber phosphor, neon vector, beige-era), including a rich interactive chart, and decide whether Rust meets the bar. If not, fall back to Vite + TypeScript + React ([interfaces.md](interfaces.md)) |
| **M1** | Protocol crate | Run `tempest make ./folder` to produce a manifest and bundle ID. All test vectors pass, and every message encodes and decodes (with property tests and fuzzing) |
| **M2** | Core + simulator skeleton | Watch two simulated peers exchange a bundle, then a 1,000-peer swarm. Runs replay deterministically. Results land in DuckDB |
| **M3** | Simulator validation | Reproduce the classic rarest-first and choking results (Bharambe et al., Legout et al.), then run the first experiments from [simulation.md](simulation.md) |
| **M4** | First real transfer | Run `tempest send ./folder` on one machine and `tempest get <link>` on another, on a LAN, using a link with a direct address over QUIC |
| **M5** | Engine + public swarms | Run the daemon with its HTTP API, event stream and SQLite state. Swarms grow through the tracker's peer-list service, peer exchange and LAN discovery |
| **M6** | Getting through routers | Port mapping, relays and hole punching, so direct sends work between two homes |
| **M7** | Terminal UI | Use the k9s-style TUI against the engine |
| **M8** | Web UIs | Use the client web UI and the simulation results UI, built on the shared `ui-kit` |
| **M9** | Tracker catalog | Search, browse, rank and comment, with listings live from the network |
| **M10** | Learning guide | Decide the guide's design (D-091), then build it |

## How this maps to the phases in index.md

| Phase | Milestones |
| --- | --- |
| 3. Protocol specs | Finished before M1 |
| 4. Simulation | M2, M3 |
| 5. Client implementation | M1, M4, M5, M6 |
| 6. Iterate with simulation | Ongoing from M3 |
| 7. Web interface | M0 (prototype), M8 |
| 8. Terminal interface | M7 |
| 9. Learning guide | M10 |
| 10. Tracker | Peer-list service in M5, catalog in M9 |
