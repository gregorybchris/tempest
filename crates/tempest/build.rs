//! Embeds the web UI in the binary when it has been built: trunk puts it in `crates/web/dist`
//! (D-201).

use tempest_assets::build::embed;

fn main() {
    let dir = env!("CARGO_MANIFEST_DIR");
    embed(dir, "web", "web_assets.rs");
}
