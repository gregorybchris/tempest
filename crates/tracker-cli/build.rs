//! Embeds the tracker website in the binary when it has been built: trunk puts it in
//! `crates/tracker-web/dist` (D-201).

use tempest_assets::build::embed;

fn main() {
    embed(
        env!("CARGO_MANIFEST_DIR"),
        "tracker-web",
        "tracker_web_assets.rs",
    );
}
