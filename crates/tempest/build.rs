//! Embeds the client web UI in the binary when it has been built: trunk puts it in
//! `crates/web/dist` (D-201). Without it, the binary serves a page that says how to build it.

use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let web = Path::new(env!("CARGO_MANIFEST_DIR")).join("../web");
    let dist = web.join("dist");
    // Watching a folder that doesn't exist would rerun this on every build; the web crate's
    // folder exists, and gains `dist` when the UI is built.
    let watched = if dist.is_dir() { &dist } else { &web };
    println!("cargo:rerun-if-changed={}", watched.display());

    let mut files = Vec::new();
    if dist.is_dir() {
        collect(&dist, &dist, &mut files);
    }
    files.sort();
    let mut code = String::from("&[\n");
    for (published, source) in files {
        code.push_str(&format!(
            "    ({published:?}, include_bytes!({:?})),\n",
            source.display().to_string()
        ));
    }
    code.push_str("]\n");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    fs::write(out.join("web_assets.rs"), code).expect("OUT_DIR is writable");
}

/// Every file under `dir`, as its path from `root` with `/` between segments, and its path.
fn collect(root: &Path, dir: &Path, files: &mut Vec<(String, PathBuf)>) {
    let entries = fs::read_dir(dir).expect("the web UI's files can be listed");
    for entry in entries {
        let path = entry.expect("the web UI's files can be listed").path();
        if path.is_dir() {
            collect(root, &path, files);
        } else {
            let relative = path.strip_prefix(root).expect("under the root");
            let published: Vec<_> = relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            let source = path.canonicalize().expect("the file exists");
            files.push((published.join("/"), source));
        }
    }
}
