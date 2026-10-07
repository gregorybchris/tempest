# Dioxus prototype

## How to run

```sh
cd prototypes/ui/dioxus-app && trunk serve        # http://127.0.0.1:8082/
trunk build --release                             # optimized build in dist/
```

## Versions

- `dioxus` 0.7.10 (latest stable; 0.8 is alpha), `default-features = false`, features `web`, `minimal`, `mounted`.
- `wasm-bindgen` 0.2.129, `web-sys` 0.3.106, `gloo-timers` 0.3.0 (tick loop), `gloo-events` 0.2.0 (window keydown).
- trunk 0.21.14, wasm-opt version_123 (downloaded by trunk), rustc 1.98.0-nightly.
- Release profile: `opt-level = "z"`, `lto = true`, `codegen-units = 1`, `panic = "abort"`, `data-wasm-opt="z"`.

The Dioxus CLI (`dx`) was not installed or used. trunk works with Dioxus 0.7 web without changes: mount with `LaunchBuilder::web().with_cfg(dioxus::web::Config::new().rootelement(body))`, link CSS from `index.html`, skip `asset!`.

## Size (release)

| File | Raw | gzip -9 |
| --- | --- | --- |
| `.wasm` | 432 KB | 181 KB |
| wasm-bindgen glue `.js` | 59 KB | 9.7 KB |
| Dioxus interpreter snippets (5 files) | 24 KB | 5.8 KB |
| **All JS** | 84 KB | 14 KB |

Debug `.wasm` is 5.8 MB.

## Build times (Apple M2, 16 GB)

- Clean release build (`cargo clean && trunk build --release`, all deps, wasm-opt included): **34 s** (two runs: 34.0 s, 33.8 s).
- Incremental debug rebuild after changing one string in a view, from file save to the refreshed page showing it (`trunk serve` autoreload, timed with Playwright): **1.6–1.9 s** (4 runs: 1.92, 1.57, 1.68, 1.55 s).

Dioxus hot-patching (`dx serve --hotpatch`, subsecond) was not evaluated, since it needs the CLI.

## Lines of app code

743 non-blank, non-comment lines of Rust in `src/` (821 total), plus a 15-line `index.html` and a 6-line `Trunk.toml`. No app-local CSS.

## Smoothness

Measured on the release build in headless Chromium with CDP `Performance.getMetrics`:

- Per 250 ms tick: about 1.2 ms script, 1.7 ms layout, 0.2 ms style recalc (4.3 ms task time in total). No long tasks. Over 5 s of ticking, rAF frame intervals were p50 16.7 ms, p99 16.8 ms, max 16.8 ms.
- The chunk map's 1,536 cells re-diff every tick. Dioxus only writes changed attributes, so the `cell--flight` CSS animation is not restarted (its `currentTime` keeps advancing across ticks).
- Hover and drag feel instant: 400 pointer moves over the chunk map and chart cost 85 ms of script in total (about 0.2 ms each).

How the updates are kept cheap:

- Hover state lives in signals that only small child components read (`Readout`, `Marks`, `Tooltip`), so hovering does not re-render the chunk map or the chart lines.
- The chart's history and geometry are `use_memo`s. History only changes once per second, so the chart re-renders once per second, not on every tick.
- The chunk map has one delegated `onmouseover` listener that reads `data-cell` from the event target, instead of 1,536 per-cell closures.

## Fidelity

Verified against `snapshots/{green,amber,vector,beige}.png` with screenshots, and by comparing the DOM tree (tags, classes, ids, attribute names) to `snapshots/green.html`. Hover tooltip, crosshair, dots, drag selection, zoom, all three resets, series toggles, themes, scanlines, selection and every key were checked with Playwright. Differences:

- Dioxus adds `data-dioxus-id` to `<body>` (the mount root) and to every element with an event listener (14 elements). Its event delegation needs them, so this can't be avoided.
- Attribute order differs on a few elements (e.g. `role` before `style` on `.chunkmap`). This doesn't affect rendering.
- The page is blank until the WASM loads. There's no server-rendered first paint. `<html>` has `data-theme="green" data-scanlines="off"` in `index.html`, so the background is correct from the start.
- `trunk serve` injects its autoreload `<script>` into `<body>` in dev builds only.
- The numbers differ from the snapshots because the app ticks live. The layout matches.

## Developer experience

Easy:

- `rsx!` maps almost one-to-one to the maud reference. The first full draft compiled after two small fixes (a prop rename, quoted SVG aria attributes).
- Signals are `Copy`, so one `#[derive(Clone, Copy)] struct Ui` of signals goes into context, and handlers capture it without any cloning ceremony.
- Splitting reads into small components gives fine-grained updates without extra machinery. `use_memo` with `PartialEq` output skips re-renders when a derived value is unchanged.
- trunk worked without any workarounds.

Painful:

- **Escape hatches to `web-sys`.** You need these for anything outside the component tree:
  - `<html>` attributes are set in a `use_effect` through `web_sys`.
  - Global keyboard shortcuts use a `gloo-events` listener on `window`. Its closure calls a `use_callback` `Callback`, which enters the Dioxus runtime so it can write signals. Writing signals from a raw web-sys closure isn't documented as safe.
  - The chart's bounding rect comes from `onmounted` → `as_web_event()` (a `web_sys::Element`). `MountedData::get_client_rect()` is async, which is awkward in a pointer-move handler.
  - Reading `data-cell` from the event target means downcasting the event to `web_sys::MouseEvent`.
- **Attribute name gaps.** SVG elements don't accept `aria_label` or `aria_hidden`, so these need quoted names (`"aria-label"`). `stop_color` and `vector_effect` exist both as SVG attributes and as style properties, so quoted names are safer there too.
- **No prop named `key`.** A prop can't be called `key` (it's reserved for list keys). `rustfmt` doesn't format inside `rsx!`, so long lines in event handlers need to be wrapped by hand.
- **No interval helper.** Ticking is a `use_future` loop around `gloo_timers::TimeoutFuture`.
- **Text nodes.** `{expr}` is needed for non-literal text nodes (`"{x}"` makes clippy flag `useless_format` for a bare `String`).
- **WASM size.** 432 KB / 181 KB gzip is fine for a desktop-class app, but not small.

No hand-written JS and no `document::eval`.

## Verdict

Dioxus meets the quality bar: the page matches the reference DOM and screenshots, all interactions work, and a full live update of 1,536 cells plus tables costs about 4 ms per tick with steady 60 fps frames. Writing it was pleasant. `rsx!` and `Copy` signals make the view code read close to the maud reference. The rough edges are the `web-sys` escape hatches for global concerns (window keys, `<html>` attributes, element geometry) and a 181 KB gzip WASM payload.
