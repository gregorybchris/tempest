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

- **Message-level (first):** each link has bandwidth, latency and loss. A message arrives after `size ÷ available bandwidth + latency`, and flows share capacity max-min fairly (D-245). It scales to about 10,000 peers on one machine (D-069, D-071). How loss and ordering work is in D-158.
- **Packet-level (later):** real QUIC (`quinn-proto`, itself sans-IO) runs through the simulated network, for small validation scenarios.

## What the first version models (D-070)

- Per-peer upload and download bandwidth, latency (including geographic models) and loss
- Churn and arrivals: session lengths, flash crowds, steady arrivals
- NAT reachability, hole-punch success and relayed links
- Disk write speed and hashing CPU cost
- Misbehaving peers: free riders, peers sending bad data, strategic clients (BitTyrant-style)

How routers, relays, disks and sessions are modeled is in D-165; strategic clients, in D-166.

## Fidelity ladder

1. **The simulator:** most experiments.
2. **Real binaries under emulation:** occasional checks that results hold for the real client, using Shadow or Linux network namespaces with traffic shaping.
3. **A few real machines on the internet:** a final sanity check.

## Scenario files (D-072)

One YAML file describes a **scenario** (the world) and the **experiment** run in it (the variants to compare). The fields are provisional (D-157); the simulator's `scenario` module defines them, and [`crates/sim/scenarios/`](../crates/sim/scenarios/) has examples. In outline:

```yaml
name: release-day
seeds: 20
bundle: { files: [{ size: 4GiB }] }
peers:
  - { class: origin, starts: complete, bandwidth: { up: 50Mbps, down: 1Gbps } }
  - class: home
    count: 800
    bandwidth: { up: 20Mbps, down: 200Mbps }
    arrivals: { pattern: flash-crowd, decay: 15m }
experiment:
  vary:
    strategies.chunk_selection: [rarest-first, random]
    strategies.upload: [bittorrent, propshare, fairtorrent]
```

This produces 2 × 3 = 6 variants × 20 seeds = 120 runs, which `tempest-sim run <scenario.yaml>` runs and summarizes (D-160). Every run records its seed, the resolved scenario, a hash of the scenario file, and the git commit.

A varied key is either a setting for every peer, as above, or a field of one class, written `peers.<class>.<field>`: its `count`, `bandwidth.up`, or a setting for that class alone under `settings`, such as `peers.origin.settings.strategies.superseed` (D-230). Variants stay paired on each seed: every class draws its randomness for as many peers as it has in any variant, so changing one class's count leaves the other classes' arrivals and sessions as they were.

## Results (D-073, D-074, D-075)

- **Storage:** one local **DuckDB** database holds experiments, runs, summary metrics and full event logs (every message, decision event and state change). `tempest-sim run` adds each experiment to it. The simulator's `results` module defines the schema (D-161):
  - **experiments:** the scenario as parsed, the scenario file's path and hash, and the git commit.
  - **variants:** each combination of varied settings, with the scenario as that variant runs it.
  - **runs:** one per variant and seed, with the seed, the event log's digest and the run's headline metrics.
  - **peers:** what each peer did in a run: when it arrived, connected, got its first byte, completed and left, and what it sent and received.
  - **traffic:** messages and bytes sent per message type, with the overhead in each.
  - **events:** every log entry as a row, in order, with a readable description for decision events.
- **Viewing:** `tempest-sim web` serves the results UI and the API it reads over the database (D-204). It browses experiments and runs, charts each run over time, compares any variants, from one experiment or several, with the statistics below, and checks experiments of golden scenarios against their baselines. It has no authentication, so it warns when it listens on an address other than loopback (D-222). It shares a design-system crate with the client and tracker UIs (D-076).
- **Statistics:** many seeds per variant, paired seeds across variants, and medians with confidence intervals. Reports say whether differences are meaningful. `tempest-sim run` prints this comparison against a reference variant; the method and thresholds are in D-167, and the simulator's `stats` module implements them.

**Headline metrics** (defined in the simulator's `summary` module, D-162):

- completion-time distribution (median, 90th and 99th percentile)
- time to first peer and to first byte
- upload utilization
- availability of the rarest chunk over time (D-168)
- fairness (Jain's index of upload/download ratios)
- overhead bytes by message type
- duplicate bytes during endgame

## Quality checks

- **Validation milestone (D-078):** reproduce the classic results of Bharambe et al. and Legout et al. before trusting new conclusions. The scenarios are in [`crates/sim/scenarios/validation/`](../crates/sim/scenarios/validation/), and `crates/sim/tests/integration/validation.rs` checks each result with the statistics above; the outcome is under [Validation](#validation).
- **Regression scenarios (D-077):** a small set of fast golden scenarios, one per model feature, in [`crates/sim/scenarios/golden/`](../crates/sim/scenarios/golden/), each checked against its saved baseline, which names the version of the scenario file it came from (D-246). A quick check of a few seeds runs with every change; `tempest-sim baseline --long` runs every saved seed and compares seed by seed, and `--update` rewrites the baselines after a deliberate change (D-163, D-171).
- **Determinism check:** same seed, same event log. CI runs the 1,000-peer swarm, an example of the router model and every golden scenario twice in separate processes and compares the digests of their logs (D-163).

## Validation

The simulator reproduces the classic results qualitatively. Numbers are medians over 10 seeds.

- **Rarest-first keeps every chunk alive** (Legout; Bharambe's last-block problem). When the origin leaves after uploading about 4.5 copies' worth, every peer completes with rarest-first; with random selection a chunk is lost in 3 seeds of 10 and nobody completes in those, and with sequential selection nobody ever completes. The rarest chunk averages about 21 copies with rarest-first and 7 with random.
- **Near-optimal distribution** (Bharambe). 100 peers arriving at once finish within about 1.2× the bound set by total upload capacity, using over 80% of it. Random selection is about 14% slower at the 90th percentile.
- **The BitTorrent strategy resists free riders, mildly** (Legout). Free riders take about 15% longer than contributors under the BitTorrent strategy and no longer under the cooperative one. Finished peers and the origin serve everyone, which limits the penalty. Fast peers upload about twice what they download under either strategy (Bharambe: tit-for-tat doesn't equalize heterogeneous peers).
- **First chunks, not last ones** (Legout). A newcomer's first 5% takes about 3.7× as long as a 5% in the middle; its last 5% about 1.8×.

Validation exposed three client weaknesses, since fixed: a crowd arriving together locked half its peers out for a minute (D-169), and requests and freed slots leaked upload to peers the strategy had stopped serving (D-170).

Two effects can hide chunk selection. In [`flash-crowd`](../crates/sim/scenarios/flash-crowd.yaml), peers arrive over minutes, the origin stays and every peer sees most of the swarm, so random selection corrects itself (as Massoulié and Vojnović predict) and matches rarest-first. And an origin whose upload is the bottleneck sends what newcomers ask for first, which is chosen at random before rarity is known; it answers every peer in turn (D-154), so it sends several copies of some chunks before one of others, whatever the peers' selection.

## Experiment backlog

From the owner's planning notes and the research (see the research atlas):

- Rarest-first vs random vs sequential chunk selection: see [Validation](#validation).
- Merkle vs flat hashes (simulator-only flat variant)
- Discovery speed: tracker vs peer exchange vs LAN vs links
- Endgame with one very slow peer: limited vs classic. *First run* ([`endgame-slow-peer`](../crates/sim/scenarios/experiments/endgame-slow-peer.yaml)): without endgame, peers wait on the slow peer and the 90th percentile takes 4.3× as long; limited and classic are within 10% of each other. Neither received duplicate bytes, since cancels arrive before the redundant transfers finish.
- Does withholding upload from non-reciprocating peers help? Which upload strategy should be the default?
- Cross-swarm helping: the owner's idea of helping slow peers in other swarms (prior art: Tribler's 2Fast)
- Request size and in-flight amount sweeps. *First run* ([`request-size`](../crates/sim/scenarios/experiments/request-size.yaml), 100 ms links): the defaults are best or tied. One-chunk requests are 16% slower and send 15× the overhead, and 70% slower when only the bandwidth-delay product is kept in flight; 64-chunk requests are 15% slower.
- How the origin serves a crowd when its upload is the bottleneck. *First run* ([`origin-policy`](../crates/sim/scenarios/experiments/origin-policy.yaml)): answering every peer in turn, the default, takes 2.7× the time the origin needs to send one copy; serving a few peers at a time, as BitTorrent does, 1.5×; superseeding, 1.2×. Superseeding is on by default only while two or more peers lack chunks (D-063, D-223).
- Lingering policies and swarm stability
- Superseeding: when it helps and when it hurts. *First run* ([`superseed-crowd`](../crates/sim/scenarios/experiments/superseed-crowd.yaml), [`superseed-direct`](../crates/sim/scenarios/experiments/superseed-direct.yaml)): with a window of offered ranges, crowds keep the whole saving, and 1:1 sends keep up unless superseeding is always on at 1 Gbps; superseeding only while a crowd wants the bundle (`crowd`) keeps both, but slows a small crowd whose origin is not the bottleneck. Numbers in D-223.
- Free-rider and strategic-client robustness

## Open questions

- The latency and bandwidth datasets used for realistic presets.
- How decision events are structured, so they serve both the UIs and analysis.
