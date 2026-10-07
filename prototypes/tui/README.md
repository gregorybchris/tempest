# Tempest TUI prototype

A dummy terminal UI for feedback, driven by the simulated client in `../ui/proto-core`. Nothing is downloaded.

```sh
cargo run --release
```

`cargo run -- snapshot [width height keys]` prints one frame as plain text.

Keys: `j`/`k` move · `space` start/stop · `F` force start · `/` filter by name or tag · `s`/`S` sort column / reverse · `0`–`7` or `[`/`]` filter by status · `tab` detail tabs · `r` rename · `t` tags · `o` open folder · `x` remove · `a` add · `T` theme · `:` command palette · `?` help · `q` quit.
