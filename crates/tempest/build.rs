//! Embeds the web UIs in the binary when they have been built: trunk puts the client's in
//! `crates/web/dist` and the tracker catalog's in `crates/catalog-web/dist` (D-201).

use tempest_assets::build::embed;

fn main() {
    let dir = env!("CARGO_MANIFEST_DIR");
    embed(dir, "web", "web_assets.rs");
    embed(dir, "catalog-web", "catalog_web_assets.rs");
}
