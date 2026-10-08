# Development

How to work on Tempest efficiently, for people and coding agents. These guidelines come from measuring where builds and tests spend their time and disk.

## Setup

The simulator (`tempest-sim`, D-214) stores results in DuckDB. Its development builds link the system DuckDB library instead of compiling DuckDB's C++ source, which took minutes on every cold build:

- **macOS:** `brew install duckdb`. `.cargo/config.toml` points the build at Homebrew's copy.
- **Hooks:** `git config core.hooksPath .githooks` enables the worktree hook (see Parallel work).
- **Elsewhere:** install DuckDB's library and set `DUCKDB_LIB_DIR`, or build with `--features tempest-sim/bundled-duckdb` to compile it from source, as CI does. The `tempest` binary doesn't use DuckDB.

## Iterating

- **Test the crate you're changing.** Use `cargo nextest run -p <crate>` while iterating. Run the whole workspace once, before committing.
- **Don't wait on CI.** Commit, push and keep working. Check the result once at the end of a task, rather than polling a run until it finishes.
- **Keep local fuzzing short.** 10–20 seconds per target is enough to catch regressions while iterating. CI and occasional dedicated runs go longer.
- **Iterate on small simulations.** Use few seeds and small scenarios while developing. Run the golden baselines once per milestone, or when a change could affect speed.
- **Re-run flaky tests a few times, not dozens.** If a test fails intermittently, reproduce it a handful of times, then fix the cause or record it as a known issue.
- **Build the web UIs in development mode**, with `trunk build` or `trunk serve` (see Web UIs). Only releases and size checks use `--release --cargo-profile web`, which optimizes for size with wasm-opt; trunk skips wasm-opt in development builds.
- **Avoid release builds** unless a test needs one or you're measuring performance.
- **Don't run `cargo clean` mid-task.** It forces a full rebuild, which takes minutes. Prune stale artifacts instead (see below).

## Tests

Each crate's integration tests are one test binary, `tests/integration/main.rs`, with a module per topic (D-235). Cargo links every file directly in `tests/` into its own executable, which costs link time and disk; this links once. Shared helpers are a sibling `common` module.

- **Names.** Tests read as `module::test` under the target `integration`. Select a topic with `cargo nextest run -p <crate> -E 'test(/^topic::/)'` or `cargo test -p <crate> --test integration topic::`.
- **Snapshots.** insta names a snapshot after the module path, so the terminal UI's are `integration__screens__<name>.snap`.
- **One process.** Plain `cargo test` runs a binary's tests on threads of one process, while nextest runs each test in its own. Tests must not change the process's environment or working directory, or share a fixed port or file; use temporary directories and port 0.
- **Other programs.** Cargo builds only a package's own binaries for its tests. Tests of `tempest` that need a tracker or a relay (`crates/tempest/tests/integration/common.rs`) build `tempest-tracker` or `tempest-relay` on first use with `cargo build -p` and run it from the same target folder; tests of what a server binary does alone live in its own crate (D-236).
- **Browser tests** (`#[ignore]`, D-218) run with `--ignored` and a module filter, as CI's web job does. The gallery's wasm-bindgen tests are one binary too, configured once in its `main.rs`.

## Web UIs

Build a UI with `trunk build` in its crate (`crates/web`, `crates/tracker-web`, `crates/sim-web`). A development build is much faster than a release one. The binaries embed whatever `dist/` folders exist, whichever way they were built, so rebuild the binary after a UI build to see it embedded.

To iterate on a UI, run `trunk serve` in its crate. It rebuilds on every change and serves the page on a port of its own (`Trunk.toml`), forwarding API requests to a running backend:

- **Client web UI** (`crates/web`): `tempest daemon` is the backend, on its default port 4776. `trunk serve` forwards `/v1/`, including the event stream, to it, and the page works as the engine's own would.
- **Tracker website** (`crates/tracker-web`): `tempest-tracker --catalog catalog.db` is the backend, on its default port 4777.
- **Results UI** (`crates/sim-web`): `tempest-sim web` is the backend.

To point a UI at a backend on another port, change the `[[proxy]]` backend in its `Trunk.toml` or pass `--proxy-backend`. Pass `--port` to serve on a different port than the one in `Trunk.toml`.

## Disk

The `target/` directory is a build cache, and cargo never deletes stale artifacts from it. It grows to many times the size of a full build, from:
- old compiled copies of crates and test binaries left behind by every change;
- incremental caches;
- separate builds for tests, release, WebAssembly and fuzzing.

- **Prune regularly.** Stale test binaries add up quickly: each links the crate and all its dependencies. Delete artifacts that haven't been used for a few hours, either with `cargo sweep --time <hours>` or by removing old files from `target/debug/deps`. Do this when no build is running.

## Parallel work

- Independent tasks (for example, two separate web UIs) can run in parallel, each in its own git worktree. That cuts wall-clock time considerably compared with running them one after another.
- Parallel builds in one working tree contend for the same `target/` lock, so give each its own worktree rather than sharing a target directory.
- A new worktree starts with copy-on-write clones of the main checkout's debug builds and web UI builds, made by the `post-checkout` hook in `.githooks/`. Dependencies are then already built, and only the workspace's own crates rebuild. Clones take no disk until they diverge, but each worktree's own rebuilds do, so check free space before starting several and delete a worktree's `target/` when it's done.

## Releasing locally

CI builds releases when a version tag is pushed (D-211). Releases contain only `tempest`; the servers and the simulator are built from source (D-233, D-236). The same archives can also be built on one Mac with dist (D-232):

- **Tools:** trunk and the `wasm32-unknown-unknown` target for the web UIs; rustup targets for each platform; `zig` and `cargo-zigbuild` for Linux; `llvm` and `cargo-xwin` for Windows. Dist picks the cross compiler itself.
- **Web UI first.** Build the client web UI with `trunk build --release --cargo-profile web`, as `.github/build-setup.yml` does. Binaries embed whatever `dist/` folders exist.
- **Per target:** `dist build --artifacts=local --target <triple>` writes archives and checksums to `target/distrib`. `--artifacts=all` on one target also writes the installers, the Homebrew formulas and `sha256.sum`.
- **Smoke test** each binary that can run here: `--version`, `--help` and `tempest web --set api.port=<spare port>`. The Intel macOS binaries run under Rosetta.
- **Publishing** is separate: a GitHub release with these files, and the formulas pushed to the tap.
- **Disk:** each target's `dist` profile build takes several GB. Delete a target's `target/<triple>/dist` once it is packaged.

What a Mac can't build: the `.msi` installers, since WiX runs only on Windows. They come from CI, or from a Windows machine.
