# Releases

Releases are built by [cargo-dist](https://github.com/axodotdev/cargo-dist) (`dist`). The configuration is in `dist-workspace.toml`, and the generated workflow is `.github/workflows/release.yml`. Never edit the workflow by hand: change the configuration and run `dist generate`. CI runs `dist generate --check`, so a stale workflow fails the build. See D-133 and D-140.

## Triggers

- A pushed version tag builds and publishes a release.
- Pull requests run `dist plan` only. Nothing is built or published.

Library crates are not packaged. dist releases only crates that have binaries, so a new library crate needs no configuration.

## One-time setup

1. Create the `gregorybchris/homebrew-tap` repository on GitHub.
2. Create a token with write access to that repository and add it to this repository as the `HOMEBREW_TAP_TOKEN` secret.

Without the secret the Homebrew publish job fails. The GitHub release itself is created before that job runs.

## Cutting a release

1. Set the workspace `version` in `Cargo.toml` and commit it on `main`.
2. Preview the release locally with `dist plan --tag vX.Y.Z`. To render the installers and formula without building binaries, run `dist build --tag vX.Y.Z --artifacts=global`.
3. Push the tag: `git tag vX.Y.Z && git push origin vX.Y.Z`.

## Checks not yet done

The release workflow has not run on GitHub, and the Windows MSI has not been built. The first tag is the real test, so use a prerelease tag such as `v0.1.0-rc.1` first.
