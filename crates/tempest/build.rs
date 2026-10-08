//! Embeds the web UIs in the binary when they have been built: trunk puts the client's in
//! `crates/web/dist` and the tracker website's in `crates/tracker-web/dist` (D-201).

use tempest_assets::build::embed;

fn main() {
    let dir = env!("CARGO_MANIFEST_DIR");
    embed(dir, "web", "web_assets.rs");
    embed(dir, "tracker-web", "tracker_web_assets.rs");
}
