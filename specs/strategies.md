# Strategies

The policies the Tempest client implements. Policies are local choices each peer makes on its own; they're not part of the protocol, and other clients may choose differently. In the Tempest client, every strategy:

- is swappable and parameterized, selected through profiles plus per-setting overrides (D-004, D-005);
- emits decision events with human-readable reasons, which the UIs show and the simulator records (D-006);
- can be compared against its alternatives in simulation.

Defaults are starting points. Revisiting them with simulation data is part of the plan.

## Chunk selection

Decides which chunks to request next (D-059).

| Strategy | Behavior | Notes |
| --- | --- | --- |
| **Rarest-first** (default) | Pick the chunks the fewest connected peers have, breaking ties at random. The first few chunks are chosen at random among those quickly available | Keeps every chunk alive and gives newcomers something to trade quickly |
| Random | Pick any needed chunk at random | Baseline |
| Sequential | Pick chunks in order | For previews; poor for the swarm. Mainly an experimental contrast |
| Streaming window | Chunks near a playback position first, rarest-first elsewhere | For playing media while downloading |

Parameters to expose include: how many "random first" chunks, and the streaming window's size.

## Requests and pipelining

Decides how much to ask for at once and how much to keep in flight (D-060).

- **Request size:** a configured number of chunks (e.g. 16 chunks = 256 KiB), capped by the provider's `max_request_chunks`. Requests are smaller only when needed: at file ends and during endgame. Requests are always aligned (see the protocol).
- **In flight:** adaptive per peer, keeping about *measured rate × round-trip time* bytes requested, within the provider's `max_open_requests`.

## Endgame

Finishes the last chunks without waiting on the slowest peer (D-061). Endgame begins once every missing chunk has been requested from at least one peer.

| Strategy | Behavior |
| --- | --- |
| **Limited** (default) | Request each remaining chunk from up to *k* additional peers (default *k* = 2) |
| Classic | Request each remaining chunk from every peer that has it |

In both, when a copy arrives, the duplicate requests are cancelled by resetting their streams.

## Upload

Decides which peers are told Serving (yes), and whose requests get answered with Data (D-051, D-062).

| Strategy | Behavior |
| --- | --- |
| **BitTorrent** (default) | Every ~10 s, serve the ~4 interested peers that uploaded to us fastest over the last ~20 s. Every ~30 s, rotate one extra slot to a random interested peer |
| PropShare | Divide upload among peers in proportion to what each uploaded to us recently |
| FairTorrent | Send each next chunk to the interested peer we owe most (bytes received from them minus bytes sent to them) |
| Cooperative | Serve every interested peer; for trusted swarms |

**When complete** (D-063): nobody uploads to a complete peer, so reciprocity doesn't apply. A complete peer serves interested peers **round-robin** by default.

**Superseeding** (opt-in, D-063): when a bundle is brand new and this peer is its only complete copy, offer each chunk to only one peer until every chunk has been sent once. Each peer is offered a window of ranges that grows while no other peer is being served, so that a lone downloader isn't held to one range per round trip, and shrinks to one range in a crowd (D-223).

## Connections

- At most **50** connections per bundle by default (D-064).
- When over the limit, prefer peers that are exchanging data with us and low-latency peers. Drop idle ones.

## Lingering

How long to keep sharing after finishing (D-065, D-066).

- **Public swarms:** a session that downloaded the bundle keeps sharing until it reaches its ratio, or until nobody has wanted anything from it for its idle timeout, whichever comes first. Each limit can be switched off; with both off it shares until stopped. A session that started with every chunk, such as a bundle's origin, never stops on its own. The defaults are ratio 1.0 and ten minutes (D-065, D-224). The engine's `sharing.*` settings are the user-facing ones (D-184, D-224); the engine hands the idle timeout to the session and counts the ratio and time across runs itself.
- **Direct sends:** stop once the recipient has every chunk.
- **Leaving:** before closing its connections for good, a session sends each peer one last Peers: up to 50 peers, those it is connected to first, then others it knows (D-224). It does so only when peer exchange is on. The protocol exempts that message from the once-a-minute limit.

## Misbehaving peers

- A chunk that fails verification proves the sender misbehaved, because the transport guarantees integrity. Close the connection with Goodbye (`bad_data`) and refuse that peer ID for this bundle (D-067).

## Settings and profiles

Every strategy choice and parameter is a setting with a dotted key, and named profiles set them all (D-150, D-151). The details each strategy settled during implementation are in D-152 to D-156.

## Open questions

- Exact default values for timing parameters, to be tuned with simulation.
