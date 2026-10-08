# Development

How to work on Tempest efficiently, for people and coding agents. These guidelines come from measuring where the M1–M9 implementation runs spent their time and disk.

## Setup

The simulator (`tempest-sim`, D-214) stores results in DuckDB. Its development builds link the system DuckDB library instead of compiling DuckDB's C++ source, which took minutes on every cold build:

- **macOS:** `brew install duckdb`. `.cargo/config.toml` points the build at Homebrew's copy.
- **Elsewhere:** install DuckDB's library and set `DUCKDB_LIB_DIR`, or build with `--features tempest-sim/bundled-duckdb` to compile it from source. CI and release builds use that feature. The `tempest` binary doesn't use DuckDB.

## Iterating

- **Test the crate you're changing.** Use `cargo nextest run -p <crate>` while iterating. Run the whole workspace once, before committing.
- **Don't wait on CI.** Commit, push and keep working. Check the result once at the end of a task, rather than polling a run until it finishes.
- **Keep local fuzzing short.** 10–20 seconds per target is enough to catch regressions while iterating. CI and occasional dedicated runs go longer.
- **Iterate on small simulations.** Use few seeds and small scenarios while developing. Run the golden baselines once per milestone, or when a change could affect speed.
- **Re-run flaky tests a few times, not dozens.** If a test fails intermittently, reproduce it a handful of times, then fix the cause or record it as a known issue.
- **Build the web UIs in development mode while iterating.** Save release builds (`--cargo-profile web`, which runs wasm-opt) for checking size and before committing.
- **Avoid release builds** unless a test needs one or you're measuring performance.
- **Don't run `cargo clean` mid-task.** It forces a full rebuild, which takes minutes. Prune stale artifacts instead (see below).

## Disk

The `target/` directory is a build cache, and cargo never deletes stale artifacts from it. A full build of the workspace is about 4 GB, but during a day of agent work it grew past 30 GB, from:
- old compiled copies of crates and test binaries left behind by every change;
- incremental caches;
- separate builds for tests, release, WebAssembly and fuzzing.

- **Prune regularly.** Each file in a crate's `tests/` links into its own large executable, so stale test binaries add up quickly. Delete artifacts that haven't been used for a few hours, either with `cargo sweep --time <hours>` or by removing old files from `target/debug/deps`. Do this when no build is running.

## Parallel work

- Independent tasks (for example, two separate web UIs) can run in parallel, each in its own git worktree. That cuts wall-clock time considerably compared with running them one after another.
- Parallel builds in one working tree contend for the same `target/` lock and the same disk. Separate worktrees each need their own `target/`, so check free disk space before starting several.
