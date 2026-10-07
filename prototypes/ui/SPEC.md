# M0: Rust UI quality prototype

Goal: decide whether a Rust-built web UI can meet Tempest's quality bar, and which approach is best (`specs/interfaces.md`, D-087 to D-089). Three apps render the **same page**, so differences come from the framework, not the design.

| App | Approach | Folder | Dev port |
| --- | --- | --- | --- |
| Leptos | Rust → WASM, client-side rendering, built with `trunk` | `leptos-app/` | 8081 |
| Dioxus | Rust → WASM, web renderer, built with `trunk` | `dioxus-app/` | 8082 |
| Server-rendered | axum + maud HTML + Datastar (vendored JS) | `server-app/` | 8083 |

## Shared pieces (do not modify)

- `proto-core/`: the simulated client (`World`), formatting (`format`), and chart geometry (`chart::rates`, `chart::hover`, `chart::zoom`, `chart::sparkline`). All numbers, labels and SVG path data come from here.
- `shared/tempest.css`: the only stylesheet. Link it unchanged. If an app truly needs extra CSS, put it in a small app-local file and list it in that app's `NOTES.md`.
- `server-app/src/render.rs`: **the reference DOM**. Every app reproduces this markup: elements, class names, ids, attributes and nesting. `snapshots/<theme>.html` are static renders of it, and `snapshots/*.png` are screenshots.

## Page behavior

All apps must implement everything below.

**Live data.** Create `World::new(42)`. Every 250 ms of real time, call `world.tick(250)` and update the page: top-bar meters and sparklines, the transfers list, and the selected bundle's summary, chunk map, chart, peers and decision log (newest 60).

**Theme and scanlines.** These live on the `<html>` element as `data-theme` (`green`, `amber`, `vector`, `beige`) and `data-scanlines` (`on`, `off`). Theme buttons set the theme and update `aria-pressed`. The scanlines button toggles. The default is `green`, scanlines off.

**Transfers.** Clicking a transfer selects it (`aria-current="true"`) and shows it in the detail panel. Selecting another bundle resets chart zoom.

**Chunk map hover.** Hovering a cell sets the `#readout` text, for example:

- `chunks 41,216–41,470 · have · 3 of 14 peers have them`
- `· downloading ·` or `· missing ·` for those states
- for a cell with availability 0: `· no connected peer has them`

Use `Bundle::cell_chunks` and `format::count`; the end is inclusive (`end - 1`). On leaving the map, restore `hover a cell for details`.

**Chart.** The SVG uses viewBox `0 0 960 240` (`CHART_WIDTH`, `CHART_HEIGHT`) with `preserveAspectRatio="none"`. Convert pointer positions into viewBox x: `(clientX - rect.left) / rect.width * 960`.

- **Hover:** call `chart::hover`. Append inside the SVG, after the lines:
  - `<line class="chart__crosshair" x1=x x2=x y1=plot_top y2=plot_bottom>`
  - one `<circle class="chart__dot chart__dot--{key}" cx cy r="3.5">` per point

  Inside `.chart`, after the SVG, add `<div class="chart__tooltip" style="left:{x/960*100}%">` with one `<div>` per `lines` entry. Add the class `chart__tooltip--left` when x > 672 (70%). Remove all of these on pointer leave.
- **Drag to zoom:** while dragging, draw `<rect class="chart__selection" x y=plot_top width height=plot_bottom-plot_top>`. On release, call `chart::zoom`. If it returns a window, zoom to it. While zoomed, the window stays fixed as new data arrives.
- **Reset:** double-click, the `#chart-reset` button (shown only when zoomed), or the `0` key resets the zoom.
- **Series toggles:** the two `.series-toggle` buttons flip `Visible.down` / `Visible.up` and their `aria-pressed`.

**Keyboard** (ignored while focus is in a text input):

| Key | Action |
| --- | --- |
| `j` / `k` | Next / previous transfer |
| `t` | Next theme |
| `s` | Scanlines |
| `d` / `u` | Toggle a series |
| `0` | Reset zoom |

**No other JavaScript.** The WASM apps use only the framework's generated glue. The server app uses only the vendored Datastar file. Fonts come from the Google Fonts `<link>` in the reference `<head>`.

## Each app's NOTES.md

Measure honestly and keep it short:

1. **How to run** (one command).
2. **Versions** of the framework and key crates.
3. **Size:** release WASM size and gzip size, plus JS. For the server app, the JS and the HTML/SSE payload per update.
4. **Build times:** clean release build. Incremental rebuild after a one-line view change, debug, from save to page refresh.
5. **Lines of app code**, not counting `proto-core`.
6. **Smoothness:** does the 1,536-cell chunk map update without jank? Do hover and drag feel instant?
7. **Fidelity:** what differs from the reference, and why.
8. **Developer experience:** what was easy, what was painful, and any workarounds. Be specific.
9. **Verdict** in two or three sentences.

Verify with screenshots, against `snapshots/<theme>.png`. Use headless Chrome, or Playwright from a scratch directory for hover and drag states. Don't commit screenshots or `node_modules`.
