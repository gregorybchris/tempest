# Leptos prototype

## How to run

```sh
cd prototypes/ui/leptos-app && trunk serve --port 8081   # add --release for an optimized build
```

`Trunk.toml` already sets the port, so plain `trunk serve` works too.

## Versions

- Leptos 0.8.22 (latest stable; `csr` feature). 0.9 is still in beta (`0.9.0-beta2`), and 0.8 worked, so I stayed on stable.
- reactive_graph 0.2.15, tachys 0.2.19, wasm-bindgen 0.2.129, web-sys 0.3.106.
- trunk 0.21.14, rustc 1.98 nightly. Release profile: `opt-level = "z"`, LTO, 1 codegen unit, `panic = "abort"`, plus `wasm-opt -Oz` via trunk.

## Size (release)

| File | Raw | gzip -9 | brotli -q 11 |
| --- | --- | --- | --- |
| `leptos-app_bg.wasm` | 317 KB | 129 KB | 106 KB |
| `leptos-app.js` (wasm-bindgen glue) | 34 KB | 6.6 KB | |

No other JS. The debug WASM is 11.9 MB.

## Build times (M-series Mac; other prototypes were compiling at the same time)

- Clean release build (`cargo clean && trunk build --release`, including wasm-bindgen and wasm-opt): **49 s**.
- Incremental debug rebuild after a one-line text change in a `view!`, from save to trunk's "success": **1.7–2.3 s** (3 runs). The page then reloads and renders in about 0.1 s, so about 2 s from save to refreshed page.

## Lines of app code

735 non-blank, non-comment lines of Rust (873 total) in `src/`, plus a 14-line `index.html`, `Trunk.toml` and `rustfmt.toml`.

## Smoothness

Measured with Playwright in headless Chrome at 1440×1000, ticking at 4 Hz:

- Release and debug builds: 0 frames over 20 ms and no long tasks in 5 s; max frame 16.8 ms. WASM self-time is under 1 ms per tick.
- Hover and drag feel instant. The tooltip moves with the pointer, and the hover memo only changes the DOM when the nearest sample changes.

The chunk map was not smooth at first. Each of the 1,536 cells had its own `Memo` reading the world signal. That cost about 55 ms per tick in release and 200 ms in debug, so every tick was a long task. The CPU profile showed a single WASM function using all the time. The cause is that `reactive_graph` keeps each signal's subscribers in an `IndexSet` and unsubscribes with `shift_remove` (O(n)), and every memo unsubscribes and re-subscribes each time it re-runs. With 1,536 subscribers on one signal, that is O(n²) per tick. The fix was one `Effect` that diffs the selected bundle's cells into a `RwSignal` per cell (`detail.rs`). The world signal now has about 60 subscribers, and each tick touches only the cells that changed.

Other update paths:

- The peers table is a `<For>` keyed by peer id. Each row's memo updates only the text nodes that changed.
- The decision log is keyed by (bundle, time, rank), so a new entry is inserted at the top without re-rendering the rest.
- The chart memo depends only on history, zoom and visibility, so it re-renders at 1 Hz. Series `<path>`s stay in the DOM, and only their `d` changes.

## Fidelity

Screenshots of all four themes match `snapshots/*.png`. The hover tooltip (including `--left`), crosshair, dots, drag selection and zoomed axes render as the CSS intends. The numbers differ only because the snapshots are taken after 40 ticks. DOM differences from `render.rs`:

- **Comment nodes.** Leptos inserts `<!---->` placeholders. One goes inside every element built with dynamic attributes and no children, because empty children render as a placeholder: all 1,536 cells, the SVG paths, lines and circles. Others mark `<For>`, `<Show>` and `Option` positions. They are invisible, and `tempest.css` uses no `:empty`, so rendering is unaffected.
- **Stray `<g>` (worked around).** Leptos renders fully static SVG subtrees from an HTML template and wraps them in a stray `<g>`. This is a bug in `tachys::renderer::dom::create_svg_element_from_html`, which returns the wrapper `<g>`. It hit the wordmark's `<defs>`/`<stop>`s. Writing numeric literals (`x1=0`, `offset=0.55`) makes those elements dynamic and avoids the bug (commented in `topbar.rs`).
- **Attribute order.** On cells, `style` comes before `class`. This has no visual effect.
- **Head.** `tempest.css` is served as `/tempest.css` like the reference (filehash off). Trunk adds an `integrity` attribute and `modulepreload`/`preload` links for the WASM and JS. Chrome logs a harmless warning that `integrity` is ignored on the WASM preload. `trunk serve` also injects its live-reload script.

No extra CSS.

## Developer experience

Easy:

- Putting all state in a `Copy` struct of signals (`state::Ui`), shared through context, keeps closures free of `.clone()` noise.
- `view!` reads like the reference markup. Reactive attributes are just closures, and typed events (`web_sys::PointerEvent`) and `NodeRef<Svg>` made the chart interactions short.
- `window_event_listener` handles the keyboard. `set_interval` drives the ticker.
- It compiled on the first try. Rebuilds take about 2 s.

Painful:

- **Performance cliff.** The O(n²) subscriber cost above is easy to hit: one memo per item is the obvious idiomatic pattern. Release WASM has no function names, so the profile only showed `wasm-function[109]`, and I had to find the cause by reading `reactive_graph`.
- **Two rendering surprises found by diffing the DOM.** These were the stray `<g>` around static SVG and the comment placeholder inside empty elements. Both are invisible in screenshots.
- **Effects run after mount.** A value fed by an `Effect` must be seeded with `untrack(...)`, or the first frame renders defaults.
- **Formatting.** `rustfmt` does not format inside `view!` (that needs the separate `leptosfmt`, which I didn't use). Long attribute lines need manual care.
- **Verbose reads.** Reactive reads are wordy (`move || ui.bundle(|b| ...)`). I used small `fn`-pointer helpers (`b(|b| ...)`) to keep the view readable.

## Verdict

Leptos meets the quality bar. The page matches the reference visually, every behavior works, release WASM is 129 KB gzipped, and after one fix it updates 1,536 cells at 4 Hz with no dropped frames. Writing it is pleasant, and the DOM output is close to hand-written. But the reactive graph has a non-obvious quadratic cost when many subscribers share one signal. Large live views need a deliberate pattern: diff into per-item signals, or use stores.
