# Simulation

The design of Tempest's simulation system. The goal: change a strategy, a parameter or a protocol component, then *measure* the effect across many peers and network conditions, reproducibly and with statistical care.

## Architecture: one brain, two drivers

Tempest's protocol and strategy logic is written **sans-IO** (D-068). It's a set of plain state machines that never touch sockets, clocks or global randomness. A peer's logic is told what happened and when, and answers with what it wants to do:

```rust
peer.handle_message(now, from, msg);   // "this arrived at time `now`"
peer.handle_timer(now);                // "it's now `now`"
let actions = peer.poll_actions();     // → [Send(to, msg), …]
let wake_at = peer.next_timer();       // "call me again at this time"
```

The same logic runs under two drivers:

| Driver | Feeds the logic with | Used for |
| --- | --- | --- |
| **Production** | Real QUIC connections (tokio + quinn) and the real clock | The actual client |
| **Simulation** | A discrete-event scheduler, a simulated network, and seeded randomness | Experiments, regression tests, debugging |

The simulation driver keeps a queue of future events and jumps straight from one to the next, so an hour of swarm time can run in seconds. Every random choice comes from a seeded generator, so the same seed replays exactly the same run.

**Discipline:** the core must never read the real clock or real randomness. A CI check runs the same scenario twice with the same seed and verifies that the event logs are identical.

**Bonus:** the same logic can compile to WebAssembly for interactive widgets in the learning guide.

Protocol-level alternatives, such as a flat-hash variant for comparison with Merkle trees, can exist in code behind common interfaces for simulation only (D-004).

## Network model

- **Message-level (first):** each link has bandwidth, latency and loss. A message arrives after `size ÷ available bandwidth + latency`, and links share capacity fairly. It scales to about 10,000 peers on one machine (D-069, D-071).
- **Packet-level (later):** real QUIC (`quinn-proto`, itself sans-IO) runs through the simulated network, for small validation scenarios.

## What the first version models (D-070)

- Per-peer upload and download bandwidth, latency (including geographic models) and loss
- Churn and arrivals: session lengths, flash crowds, steady arrivals
- NAT reachability, hole-punch success and relayed links
- Disk write speed and hashing CPU cost
- Misbehaving peers: free riders, peers sending bad data, strategic clients (BitTyrant-style)

## Fidelity ladder

1. **The simulator:** most experiments.
2. **Real binaries under emulation:** occasional checks that results hold for the real client, using Shadow or Linux network namespaces with traffic shaping.
3. **A few real machines on the internet:** a final sanity check.

## Scenario files (D-072)

One YAML file describes a **scenario** (the world) and the **experiment** run in it (the variants to compare). Illustrative only, since field names are not decided:

```yaml
name: flash-crowd-small-seed
duration: 2h
seeds: 20

network:
  latency: { model: geographic, regions: [na, eu, asia] }
  loss: 0.5%

peers:
  - class: home-cable
    count: 800
    bandwidth: { down: 200Mbps, up: 20Mbps }
    nat: { punchable: 70% }
    churn: { session: weibull(shape=0.6, mean=40m) }
  - class: free-rider
    count: 50
    behavior: never-upload
  - class: origin
    count: 1
    bandwidth: { down: 1Gbps, up: 50Mbps }
    starts: complete

bundle:
  files: [{ size: 4GiB }]
arrivals: { pattern: flash-crowd, peak: 0s, decay: 15m }

experiment:
  vary:
    strategies.chunk_selection: [rarest-first, random]
    strategies.upload: [bittorrent, propshare, fairtorrent]
```

This produces 2 × 3 = 6 variants × 20 seeds = 120 runs. Every run records its seed, the resolved scenario, a hash of the scenario file, and the git commit.

## Results (D-073, D-074, D-075)

- **Storage:** one local **DuckDB** database holds experiments, runs, summary metrics and full event logs (every message, decision event and state change).
- **Viewing:** a **Rust web UI** reads the database and shows results with beautiful, high-quality interactive charts. It shares a design-system crate with the client and tracker UIs (D-076).
- **Statistics:** many seeds per variant, paired seeds across variants, and medians with confidence intervals. Reports say whether differences are meaningful.

**Headline metrics:**

- completion-time distribution (median, 90th and 99th percentile)
- time to first peer and to first byte
- upload utilization
- availability of the rarest chunk over time
- fairness (Jain's index of upload/download ratios)
- overhead bytes by message type
- duplicate bytes during endgame

## Quality checks

- **Validation milestone (D-078):** reproduce the classic results of Bharambe et al. and Legout et al. before trusting new conclusions.
- **Regression scenarios (D-077):** a small set of fast golden scenarios runs on every change, against saved baselines.
- **Determinism check:** same seed, same event log.

## Experiment backlog

From the owner's planning notes and the research (see the research atlas):

- Rarest-first vs random vs sequential piece selection
- Merkle vs flat hashes (simulator-only flat variant)
- Discovery speed: tracker vs peer exchange vs LAN vs links
- Endgame with one very slow peer: limited vs classic
- Does withholding upload from non-reciprocating peers help? Which upload strategy should be the default?
- Cross-swarm helping: the owner's idea of helping slow peers in other swarms (prior art: Tribler's 2Fast)
- Request size and in-flight amount sweeps
- Lingering policies and swarm stability
- Superseeding: when it helps and when it hurts
- Free-rider and strategic-client robustness

## Open questions

- Exact YAML schema and field names.
- The latency and bandwidth datasets used for realistic presets.
- How decision events are structured, so they serve both the UIs and analysis.
