# Server-rendered prototype: axum + maud + Datastar

## How to run

```sh
cargo run --release        # http://127.0.0.1:8083
```

`cargo run -- snapshot <theme> <out.html>` still writes the reference page, byte-identical to `snapshots/*.html`. `TEMPEST_STATS=1` logs render time and bytes per SSE update.

## Versions

Datastar 1.0.4 (vendored as `assets/datastar-1.0.4.js` and served via `include_str!`), `datastar` crate 0.4.1 (the official Rust SDK; used for its `PatchElements` / `PatchSignals` builders and axum `Event` conversion), axum 0.8.9, tokio 1.53, maud 0.27, tower-http 0.7.1 (compression). Rust 1.98 nightly.

## Architecture

- The server owns the `World` (ticked every 250 ms by a tokio task) and a per-session `Ui` (cookie `tempest_session`), in `state.rs`.
- The page loads with `data-init="@get('/updates')"`. That SSE stream wakes on every tick and every session change, re-renders each fragment with maud, and sends only the fragments whose HTML changed since this stream last sent them (`patch.rs`).
- Every interaction is a `@post` to a small endpoint that mutates the session and returns `204`. The visible result comes back over the SSE stream. All logic (key mapping, selection wrap-around, zoom, hover) runs in Rust.
- Large fragments are patched finer:
  - Chunk map: changed cells only, one event per cell (`selector #chunks [data-cell="N"]`). About 4.5 cells change per tick.
  - Log: new entries are prepended and the overflow removed.
  - Chart overlay: on pointer moves, the crosshair, dots, selection and tooltip are removed and re-appended. The 30 KB of paths are not re-sent.
  - Meters: values and sparklines are patched separately.
- Theme and scanlines are Datastar signals, bound to `<html>` with `data-attr`. The chunk map's `aria-label` is a signal too, so a progress change doesn't re-send all 1,536 cells.
- The SSE stream is gzip/brotli-compressed. tower-http skips `text/event-stream` by default, so the predicate is overridden. Events still flush per tick.

## Size

| | raw | gzip | brotli |
| --- | --- | --- | --- |
| `datastar.js` (the only JS) | 33,553 B | 13,377 B | n/a |
| Initial HTML page | 166.8 KB | 17.9 KB | 15.6 KB |
| SSE, steady state (default view) | 76 KB/s (~19 KB per tick) | 10.4 KB/s | 6.0 KB/s |
| SSE on connect (all fragments) | 162 KB | | |
| SSE per pointer move (chart) | ~1.05 KB, 4–5 events | | |
| SSE per chunk hover | ~150 B (readout only) | | |

Chrome sends brotli only over HTTPS, so over `http://127.0.0.1` the browser gets gzip.

Steady-state raw bytes by fragment, over 30 s:

| Fragment | Events/s | Bytes/event | KB/s |
| --- | --- | --- | --- |
| `#rates` (whole chart, once per second when a sample lands) | 1 | 30,500 | 30.8 |
| `#peers` (every rate changes every tick) | 4 | 6,450 | 25.4 |
| `.transfer-list` | 4 | 1,840 | 7.2 |
| sparklines | 2 × 1 | 2,410 | 4.9 |
| `#summary` | 4 | 660 | 2.6 |
| chunk cells | ~4.5 | 153 | 0.7 |
| log prepend + trim | ~1.4 | 380 + 96 | 0.7 |
| meter values, signals | | ~110 | 0.9 |

The first version morphed whole fragments: `#chunks` alone was 106 KB per tick, and the total was 445 KB/s. Cell-level patches brought the total down 6×. Diffing rendered strings is crude but effective. It does mean every tick renders everything (3.3 ms per tick in release, 10 ms in debug, mostly the 1,536 cell strings and the chart rendered twice). A pointer-only update renders just the readout and overlay, from a cached chart: 51 µs.

## Build times

- Clean release build: 16.0 s (all dependencies). Binary: 3.5 MB.
- Incremental debug rebuild after a one-line view change in `render.rs`: 0.5 s.
- Debug server restart: 0.5 s to listening (most of it is `World::new` pre-filling history).
- Datastar's SSE retry reconnects an open tab by itself. Measured from restart to the first patch in the open page: 0.8 s. A fresh stream re-sends every fragment, so fragment-level view changes show without reloading. Changes to `page()` itself need a reload.
- Save to visible change: about 1.3 s with no manual refresh (build, restart, reconnect). No file watcher is set up; `cargo watch -x run` would make it hands-free.

## Lines of app code

1,094 lines of Rust: `render.rs` 563 (the reference was 384), `main.rs` 208, `patch.rs` 171, `state.rs` 152. No JS files. The live page carries 24 Datastar attributes (25 when zoomed), written as 19 attribute sites in `render.rs`. All expressions are inline there.

The logic in expressions:

- Pointer x in viewBox units, repeated in three chart handlers: `(evt.clientX - el.getBoundingClientRect().left) / el.getBoundingClientRect().width * 960`.
- `el.setPointerCapture(evt.pointerId)` on pointer down.
- Chunk-map event delegation: `evt.target.dataset.cell && @post(...)`.
- The keyboard filter. It is the longest at about 200 characters: key length, a key whitelist, modifier keys and a text-input check, then `@post('/key/' + evt.key)`.
- Everything else is a bare `@post('/…')`.

## Smoothness

Measured with Playwright (headless Chromium, M-series Mac, server on localhost):

- The chunk map updates without jank. Idle with live updates, the main thread is 6% busy (script 1.3%, layout 1.6%). No frame went over 20 ms in 10 s, even with 4× CPU throttling.
- **Chunk hover:** 1.3–2.4 ms median from `pointerover` to the `#readout` DOM change, p95 under 5 ms, over a 90-cell sweep at 60 Hz.
- **Chart hover:** 2.2–3.3 ms median from `pointermove` to the overlay DOM change, p95 under 6 ms. All 120 moves of a 2 s sweep were applied, and the crosshair ends exactly at the last pointer position. No dropped frames during the sweep.
- **Drag to zoom:** the selection rect follows the pointer with the same latency. Release zooms, the window stays fixed while data arrives, and double-click, the button and `0` all reset.
- **On a real network, lag equals one round trip.** A delaying TCP proxy measured:

  | RTT | Crosshair trails the pointer by | Updates applied during the sweep |
  | --- | --- | --- |
  | 20 ms | ~20 ms | 119/120 |
  | 50 ms | ~50 ms | 119/120 |
  | 100 ms | ~100 ms | 116/120 |

  Locally it feels instant, indistinguishable from client-side code. At 50 ms or more the crosshair visibly trails the pointer.
- Datastar's default `requestCancellation: 'auto'` aborts the previous in-flight POST to the same URL on every move. Every POST is reported "aborted" by the browser but still reached the server. The effect comes back over the SSE stream, not the POST response, so the aborts do no harm.
- Each aborted request closes its HTTP/1.1 connection, so a hover sweep opens a new TCP connection per move. That is fine on localhost and wasteful elsewhere.

## Fidelity

Screenshots match `snapshots/*.png`. The static markup is byte-identical, and the live DOM differs only by these additions:

- `data-*` Datastar attributes on `<html>`, `<body>`, the theme, scanlines, transfer, series and reset buttons, `.chunkmap`, and `#chart`.
- A `<script type="module" src="/datastar.js">` in `<head>`.

Behavior and DOM gaps:

- **Tooltip `left` precision.** The style is formatted as `left:{:.3}%`.
- **Readout state for partial cells.** A cell with `0 < have < 1` that isn't in flight reads "missing". In the sim, partial cells are always in flight.
- **Hover state is server-side.** The crosshair, tooltip and readout re-render with live data under a still pointer. This is arguably better than the spec. The cost is that a reload or a second tab shares the same session's hover and zoom state.
- **POST ordering.** POSTs travel on separate connections, so their order isn't guaranteed. A pointer-up overtaking its pointer-down could leave a stuck selection. Click durations make this unlikely on a LAN, but a real version needs sequence numbers or one ordered channel (WebSocket).

## Developer experience

Easy:

- maud views are ordinary Rust functions, type-checked, with 0.5 s rebuilds.
- Optional attributes (`attr=[Option]`) keep one renderer for both snapshot and live output.
- maud accepts Datastar's `data-on:click` directly. Names with `.` (e.g. `__throttle.16ms`) must be quoted: `"data-on:…"=[…]`.
- Keeping all state and logic in Rust is pleasant. Each handler is one line.
- The official SDK covers every event option needed: `selector`, `mode`, `namespace`.

Painful:

- **Fine-grained patching is hand-written.** Datastar morphs whatever you send, so efficiency is your job: string diffing, per-cell selectors, prepend/remove for the log, and a remove-then-append dance for the chart overlay. `patch.rs` has no counterpart in a signal-based framework.
- **Fragments needed stable targets.** Several reference fragments had no id, so they are targeted by CSS selector (`.meters`, `.transfer-list`, `.meter--down .spark`).
- **Datastar 1.0.4 bug.** A multi-element patch with `namespace: svg` drops every other element: it iterates `childNodes` while moving them. The workaround is one event per SVG element.
- **A changing attribute on a big element forces a big patch.** Re-sending 1,536 cells for an `aria-label` change is too much, hence the signal.
- **tower-http compression** silently skips SSE unless you change the predicate.
- **Debugging spans the server and the browser DOM.** No component state exists to inspect, and expression errors only show in the browser console.

## Verdict

It works, and on localhost it is indistinguishable from a client-side app: 2–3 ms pointer-to-DOM latency, no jank, 13 KB of JS and about 10 KB/s on the wire with gzip. Two costs come with it. Every pointer interaction costs a network round trip, which is fine for a desktop client talking to its local daemon but shows at 50 ms+ RTT. And efficient patching of a dense, fast-changing UI is manual work that a reactive framework does for you. For Tempest's local web UI it is a viable and very simple stack. For remote use, pointer-speed interactions would want a client-side path.
