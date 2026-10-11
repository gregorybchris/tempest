---
name: release
description: Cut a Tempest release locally (GitHub Actions is out of quota): bump the version, tag, build all 15 executables with dist, and publish the GitHub release. Use when asked to release or ship a new version.
---

# Release Tempest locally

Releases are built on this Mac, not in CI (D-232). Background: "Releasing locally" in `specs/development.md`. Argument: the version, e.g. `0.4.0`.

1. **Commit and push pending work to `main`** first. The tree must be clean.
2. **Bump the version.** Set `version` in `[workspace.package]` of `Cargo.toml`, run `cargo update --workspace --offline`, and commit `Release <version>` (only `Cargo.toml` and `Cargo.lock` change). Tag `v<version>` and `git push origin main v<version>`. The CI runs it triggers fail on billing; ignore them.
3. **Free disk.** A target's dist build takes several GB. If under ~20 GB is free, `rm -rf target/debug`.
4. **Build the web UIs** with `trunk build --release --cargo-profile web` in `crates/web` and in `crates/tracker-web`. Binaries embed `dist/`.
5. **Build each target** in the background (it takes tens of minutes), one at a time, deleting `target/<triple>/dist` after each: `dist build --artifacts=local --target <triple>` for `aarch64-apple-darwin`, `x86_64-apple-darwin`, `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-gnu` and `x86_64-pc-windows-msvc`. Write the loop as a script in the scratchpad that logs `== <triple>` before each build, `FAILED <triple>` and exits on a failure, and prints `ALL-DONE` at the end, and run it in the background with its output in a log. Touch a marker file just before starting it. Then start a Monitor (at most 30 minutes; re-arm it on expiry) on `.claude/skills/release/progress.sh <marker> <log> 300`, which shows every 5 minutes a grid of the 15 executables: ✅ built, 🔨 building, ⏳ waiting, ❌ failed. Relay each grid to the user as it arrives. Don't poll compile output. `target/distrib` still holds the last release's archives, so only archives newer than the marker count as built.
6. **Installers and checksums.** `dist build --artifacts=global` writes the `.sh`/`.ps1` installers and overwrites `sha256.sum` with only the installers. Then, in `target/distrib`, regenerate it over everything: `shasum -a 256 source.tar.gz tempest*.tar.xz tempest*.zip tempest*.sh tempest*.ps1 | sed 's/  / */' > sha256.sum`, and check it with `shasum -a 256 -c`.
7. **Smoke test** the macOS ARM binaries: `--version` on `tempest`, `tempest-tracker` and `tempest-relay`. `tempest web` fails if another daemon holds port 4776.
8. **Publish:** `gh release create v<version> --title "Tempest <version>" --notes "<what changed>..."` with `source.tar.gz`, `tempest*.tar.xz`, `tempest*.zip`, `tempest*.sh`, `tempest*.ps1` and `sha256.sum` from `target/distrib`. The notes end with the platform list and "Built and tested locally. `sha256.sum` lists every file's checksum." (see `gh release view v0.3.0`).
9. Report the release URL. To put it on caliban, use the `deploy` skill in the caliban repo.
