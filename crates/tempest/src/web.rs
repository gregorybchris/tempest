//! The client web UI: its files, embedded at build time when they were built (D-201) and served
//! beside the engine's API, and `tempest web`, which opens it.

use anyhow::{Context, Result};
use axum::Router;
use serde_json::Value;

use crate::assets::{self, show};
use crate::daemon;
use crate::engine::{EngineArgs, Target};

/// The UI's files, by their path in the page, as `crates/web/dist` held them at build time.
static ASSETS: &[(&str, &[u8])] = include!(concat!(env!("OUT_DIR"), "/web_assets.rs"));

/// What `/` shows when the binary was built without the UI.
const NOT_BUILT: &str = r#"<!DOCTYPE html>
<html lang="en">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Tempest</title>
<style>
  body { background: #020b05; color: #8af2a4; font: 15px/1.6 ui-monospace, monospace; padding: 8vh 6vw; }
  code { color: #d2ffe0; }
</style>
<h1>Tempest</h1>
<p>The engine is running, but this <code>tempest</code> binary was built without its web UI.</p>
<p>Build the UI with <code>trunk build --release --cargo-profile web</code> in <code>crates/web</code>,
then build <code>tempest</code> again. Meanwhile, <code>tempest tui</code> shows the same transfers,
and the engine's API answers under <code>/v1</code>.</p>
</html>
"#;

/// The routes serving the UI: `/` and each of its files.
pub fn router() -> Router {
    assets::router(ASSETS, NOT_BUILT)
}

/// Whether the binary has the UI's files.
pub fn built() -> bool {
    !ASSETS.is_empty()
}

#[derive(Debug, clap::Args)]
pub struct WebArgs {
    #[command(flatten)]
    engine: EngineArgs,
    /// Print the web UI's address rather than opening it in a browser.
    #[arg(long)]
    no_open: bool,
    /// Override a setting for an engine run here, as `tempest daemon --set` does.
    #[arg(long, value_name = "KEY=VALUE", value_parser = daemon::parse_setting)]
    set: Vec<(String, Value)>,
}

/// `tempest web`: opens the web UI of the daemon if one answers, or else runs the engine here,
/// as the daemon would, until Ctrl-C.
pub fn web(args: WebArgs) -> Result<()> {
    let mut config = args.engine.load()?;
    for (key, value) in args.set {
        config
            .set(&key, value)
            .with_context(|| format!("--set {key}"))?;
    }
    let open = !args.no_open;
    if !built() {
        eprintln!("note: this binary was built without the web UI; the page says how to add it");
    }
    let runtime = tokio::runtime::Runtime::new().context("cannot start the async runtime")?;
    runtime.block_on(async {
        if let Some(client) = Target::daemon(&config).await? {
            show(client.url(), open);
            return Ok(());
        }
        daemon::run(config, move |url| {
            eprintln!("no daemon is running, so the engine runs here until Ctrl-C");
            show(url, open);
        })
        .await
    })
}
