# Interfaces

Tempest's user-facing interfaces and the quality bar they share. Decisions referenced as D-### are in [decisions.md](decisions.md).

## The interfaces

| Interface | Built with | Talks to |
| --- | --- | --- |
| Client web UI | Leptos (D-117) | Engine API (HTTP + JSON, event stream) |
| Terminal UI | ratatui (D-090) | Engine API, or an embedded engine |
| CLI | The `tempest` binary's subcommands (D-079) | Engine API, or an embedded engine |
| Tracker catalog | Same framework as the client web UI | Tracker server |
| Simulation results UI | Same framework as the client web UI | DuckDB results database |

The three web UIs share a design-system crate, `ui-kit`, with colors, typography, components and chart styles (D-076).

## Quality bar

Every UI should be beautiful, intuitive and high quality, with interactive charts and easy filtering, sorting and search. The look evokes 1980s hacker culture, but the layout stays modern and readable at a glance: navigation, a main table or list, a detail panel and a status bar. The retro feeling lives in the skin: phosphor colors, terminal or pixel fonts, box-drawing borders and subtle scanlines.

Guardrails from the research:

- Gate flicker, scanline animation and glow effects behind `prefers-reduced-motion`, and offer a "clean" toggle.
- Keep text contrast at least 4.5:1. Glow applies to headings and numbers only, never body text.
- Check font licenses. VT323, IBM Plex Mono, JetBrains Mono and Departure Mono are free (OFL). The Ultimate Oldschool PC Font Pack needs attribution (CC BY-SA). Berkeley Mono is commercial.
- Explain states in plain language. "No one online has this bundle right now" beats "Stalled".

## Web framework

Leptos builds the web UIs (D-117). An all-Rust stack met the quality bar against Dioxus and server-rendered axum + Datastar/htmx. The fallback if it had not was Vite + TypeScript + React (D-088). Palettes are in D-089.

## Client features (v1)

These apply to both the TUI and the client web UI (D-118).

| Area | Features |
| --- | --- |
| Transfer controls | Start/stop, force start (bypasses the queue), remove (keep or delete files), rename, open download folder, recheck data |
| Queue | Limits on active downloads and uploads; reorder queued transfers |
| Limits | Global and per-transfer speed limits; alternative-limits mode (toggle or schedule); sharing limits (ratio or time) |
| Organizing | Tags (many per transfer), categories (one per transfer, each with a default save folder), move storage |
| Finding | Filter by name or tag; filter by status; sort by name, size, progress, status, ↓, ↑, peers serving us, time left, ratio, date added; custom columns; saved views |
| Sharing | Create a bundle from a file or folder and get a `tempest:` link |
| Notifications | On completion and on error |

**Statuses:** downloading, sharing, stalled, queued, stopped, completed, errored, checking, moving.

**Out for now:**

- **Files:** file selection, priority, streaming and renaming.
- **Adding and discovery:** watched folders, feeds and searching trackers.
- **Monitoring:** a trackers tab, global stats and an event log.
- **Peer management:** ban, add, disconnect and peer details.
- **Automation and system integration:** these are out too.

## Leptos guidelines

The web UIs use Leptos (D-117). Code written for them, by people or agents, follows these rules:

- Use Leptos 0.8 (pinned). Don't mix in APIs from 0.6 or older.
- Never create one memo per item that subscribes to a shared signal. Removing a subscriber is O(n) in `reactive_graph`, so this pattern becomes quadratic: 1,536 such memos cost about 55 ms per update in an early prototype. Use one effect that diffs into per-item signals, `Selector`, or keyed stores.
- Keep UI logic in plain Rust modules, and keep components thin.
- A CI benchmark of a 1,536-cell live view guards against performance regressions.

## Client layout

Shared by the TUI and the web UI (D-119, D-131, D-136 to D-139). Statuses and wording come from the feature table above and the glossary; each screen below is a sketch of structure, not exact columns.

**Principles**

- **List first.** The transfers list fills the screen. Each row answers "is it working, how far, how fast, how long" and nothing else.
- **Detail on demand.** Selecting a transfer opens its detail beside or over the list. The summary comes first: plain-language state, then files, peers and the decision log. The chunk map and rates chart are one level further in.
- **Four areas.** Transfers, Share, Catalog (trackers and catalog), Settings (limits, queue, categories, theme). Transfers is home.
- **One focus at a time.** A single pane is active, and the hint bar lists only the keys that work there.
- **Explain, don't decode.** A stalled or errored transfer says why in a sentence and offers the fix.

**Web UI (wide)**

```text
 tempest   Transfers  Share  Catalog  Settings          ↓ 12.4 MB/s  ↑ 3.1 MB/s   ◐
 ──────────────────────────────────────────────────────────────────────────────────
  Filter: all ▾   tag ▾   search…                                       + Add
 ──────────────────────────────────────────────────────────────────────────────────
  Name                          Size     Progress          ↓        ↑      ETA  Status
 ▌ensemble-2026.tar            4.2 GB   ████████░░  81%  9.8 MB/s  1.2 MB/s  3m  Downloading
  field-recordings/            812 MB   ██████████ 100%      –    0.4 MB/s   –   Sharing
  corpus-v3.bundle             1.9 GB   ███░░░░░░░  31%      –        –      –   Stalled
 ──────────────────────────────────────────────────────────────────────────────────
  ensemble-2026.tar                                             Summary  Files  Peers
  Downloading from 7 of 12 peers, 2 through relays. About 3 minutes left.  Decisions  Chunks
                                                                              Rates
```

The detail panel opens as a right-hand drawer on wide windows and as a full-screen sheet on narrow ones. The nav collapses to a bottom tab bar on phones.

**TUI (80x24 and up)**

```text
 Transfers  Share  Catalog  Settings                      ↓12.4M ↑3.1M  [green]
 ─────────────────────────────────────────────────────────────────────────────
  Name                       Size   Progress      ↓      ↑     ETA  Status
 ▌ensemble-2026.tar         4.2G   ████████░░ 81%  9.8M  1.2M   3m  Downloading
  field-recordings/         812M   ██████████ 100%    –  0.4M    –  Sharing
  corpus-v3.bundle          1.9G   ███░░░░░░░ 31%     –     –    –  Stalled
 ─────────────────────────────────────────────────────────────────────────────
  Downloading from 7 of 12 peers, 2 through relays. About 3 minutes left.
 ─────────────────────────────────────────────────────────────────────────────
  j/k move  enter detail  space start/stop  / filter  a add  : command  ? help
```

Pressing `enter` replaces the bottom summary with a split detail pane (tabs: Summary, Files, Peers, Decisions). `c` opens the chunk map and `g` the rates chart, each full-screen, and `esc` goes back one level. Narrow terminals drop optional columns in D-138's order before anything wraps.

**Navigation**

- **TUI:** `1` to `4` or `[` and `]` (with `tab` for panes) switch areas. `?` shows keys for the current screen.
- **Web:** the top bar, with the same order. Keyboard shortcuts match the TUI where they exist.

**Share** is a drop or paste target: choose a file or folder, see the bundle being built, then the `tempest:` link with copy. **Catalog** lists trackers and what they offer, with search and add. **Settings** groups limits (including alternative limits), queue, categories and tags, and theme.

## Terminal UI and web UI together

The TUI and the client web UI are designed together, not as copies. They share:
- the feature set (D-118)
- the vocabulary and statuses
- the plain-language explanations of state
- the information hierarchy, so the same things matter at a glance in both

Layout decisions (see Client layout) apply to both where they make sense. Each interface keeps its own affordances. The TUI is keyboard-first and dense; the web UI can use pointer interactions, richer charts and wider layouts. Full parity is not a goal.

## Terminal UI

ratatui, with a k9s/lazygit-style keyboard model (D-090):

- panes
- vim keys plus arrow keys
- `/` to filter, `:` for commands, `?` for help
- a persistent hint bar showing available keys

Keys, hints, help and the palette come from one binding table (D-191). Filters and sorting behave as D-192 describes.

The TUI is minimal but powerful, giving quick access to every essential feature.

## Visualization ideas

These come from the research, to try in the prototype:

- **Chunk map:** every chunk of a bundle as a cell (have, in flight, missing), shaded by rarity.
- **Swarm availability strip:** how many peers have each chunk.
- **Sparklines** for upload and download rates.
- **Peer table** with each peer's state (Want, Serving), rates and how it's connected (direct or relayed).
- **Decision log:** the live stream of decision events, explaining why the client is doing what it does (D-006).

## Open questions

- CLI command names and output style.
- Onboarding flow for first-time users (e.g. one big "drop or paste anything" target).
