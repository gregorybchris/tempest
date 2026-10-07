//! Serving a web UI's files, embedded at build time when they were built (D-201), and opening
//! it in a browser.

use axum::Router;
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

/// A web UI's files, by their path in the page, as its `dist` folder held them at build time.
pub type Assets = &'static [(&'static str, &'static [u8])];

/// The routes serving a web UI: `/` and each of its files. Without files, `/` shows
/// `not_built`, which says how to build them.
pub fn router(assets: Assets, not_built: &'static str) -> Router {
    if assets.is_empty() {
        return Router::new().route(
            "/",
            get(move || async move { asset("index.html", not_built.as_bytes()) }),
        );
    }
    assets
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
        // No other page may frame a UI.
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
/// it can be cached for good. Trunk writes the 64-bit hash without leading zeros.
fn hashed(path: &str) -> bool {
    let stem = path.rsplit_once('.').map_or(path, |(stem, _)| stem);
    let stem = stem.strip_suffix("_bg").unwrap_or(stem);
    stem.rsplit_once('-').is_some_and(|(_, hash)| {
        (8..=16).contains(&hash.len()) && hash.bytes().all(|b| b.is_ascii_hexdigit())
    })
}

/// Prints `url`, and opens it in the default browser if asked.
pub fn show(url: &str, open: bool) {
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
        assert!(hashed("tempest-web-e9a8078f88ecbd2_bg.wasm"));
        assert!(!hashed("index.html"));
        assert!(!hashed("fonts/ibm-plex-mono-latin-400.woff2"));
        assert_eq!(media_type("a_bg.wasm"), "application/wasm");
        assert_eq!(media_type("fonts/OFL.txt"), "text/plain; charset=utf-8");
    }
}
