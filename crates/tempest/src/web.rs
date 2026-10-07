//! The client web UI: its files, embedded at build time when they were built (D-201) and served
//! beside the engine's API, and `tempest web`, which opens it.

use anyhow::{Context, Result};
use axum::Router;
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde_json::Value;

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
    if ASSETS.is_empty() {
        return Router::new().route(
            "/",
            get(|| async { asset("index.html", NOT_BUILT.as_bytes()) }),
        );
    }
    ASSETS
        .iter()
        .fold(Router::new(), |router, &(path, contents)| {
            let route = match path {
                "index.html" => "/".to_owned(),
                path => format!("/{path}"),
            };
            router.route(&route, get(move || async move { asset(path, contents) }))
        })
}

fn asset(path: &str, contents: &'static [u8]) -> Response {
    let cache = match hashed(path) {
        true => "public, max-age=31536000, immutable",
        false => "no-cache",
    };
    let headers = [
        (
            header::CONTENT_TYPE,
            HeaderValue::from_static(media_type(path)),
        ),
        (header::CACHE_CONTROL, HeaderValue::from_static(cache)),
        (
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ),
        // The UI controls the engine, so no other page may frame it.
        (header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY")),
    ];
    (headers, contents).into_response()
}

/// A file's media type, from its extension.
fn media_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, extension)| extension) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css; charset=utf-8",
        Some("woff2") => "font/woff2",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Whether trunk named the file after its contents, as in `web-5883c789b3a06503.css`, so that
/// it can be cached for good.
fn hashed(path: &str) -> bool {
    let stem = path.rsplit_once('.').map_or(path, |(stem, _)| stem);
    let stem = stem.strip_suffix("_bg").unwrap_or(stem);
    stem.rsplit_once('-')
        .is_some_and(|(_, hash)| hash.len() == 16 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
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

/// Prints `url`, and opens it in the default browser if asked.
fn show(url: &str, open: bool) {
    println!("{url}");
    if open && let Err(error) = open_browser(url) {
        eprintln!("cannot open a browser ({error}); open {url} yourself");
    }
}

fn open_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", "start", ""]);
        command
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = std::process::Command::new("xdg-open");
    let status = command
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    match status.success() {
        true => Ok(()),
        false => Err(std::io::Error::other(format!(
            "the opener exited with {status}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_named_files_are_cached_for_good() {
        assert!(hashed("web-5883c789b3a06503.css"));
        assert!(hashed("tempest-web-cf79489a2eb1c420_bg.wasm"));
        assert!(hashed("tempest-web-cf79489a2eb1c420.js"));
        assert!(!hashed("index.html"));
        assert!(!hashed("fonts/ibm-plex-mono-latin-400.woff2"));
        assert_eq!(media_type("a_bg.wasm"), "application/wasm");
        assert_eq!(media_type("fonts/OFL.txt"), "text/plain; charset=utf-8");
    }
}
