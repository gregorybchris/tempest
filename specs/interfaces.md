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

## The quality prototype (done)

M0 is complete: Leptos, Dioxus and server-rendered prototypes are in `prototypes/ui/`. All three met the bar, and Leptos was chosen (D-117). The original plan follows.

Before heavy implementation, build a small test UI to decide whether Rust can meet the bar:

- **Approaches compared (D-087):** Leptos, Dioxus, and server-rendered axum + Datastar/htmx.
- **Palettes explored (D-089):** green phosphor, amber phosphor, neon vector, and beige-era computing (light).
- **Must include:** a transfer list, a chunk map, live speed sparklines and one rich interactive chart, so chart quality is tested, not assumed.
- **Fallback (D-088):** if Rust can't meet the bar, use a Vite + TypeScript + React frontend. All-Rust is strongly preferred.

## Terminal UI

ratatui, with a k9s/lazygit-style keyboard model (D-090):

- panes
- vim keys plus arrow keys
- `/` to filter, `:` for commands, `?` for help
- a persistent hint bar showing available keys

The TUI is minimal but powerful, giving quick access to every essential feature.

## Visualization ideas

These come from the research, to try in the prototype:

- **Chunk map:** every chunk of a bundle as a cell (have, in flight, missing), shaded by rarity.
- **Swarm availability strip:** how many peers have each chunk.
- **Sparklines** for upload and download rates.
- **Peer table** with each peer's state (Want, Serving), rates and how it's connected (direct or relayed).
- **Decision log:** the live stream of decision events, explaining why the client is doing what it does (D-006).

## Open questions

- Which palette (or palettes, as themes) ships, and a rethought client layout for M8.
- CLI command names and output style.
- Onboarding flow for first-time users (e.g. one big "drop or paste anything" target).
