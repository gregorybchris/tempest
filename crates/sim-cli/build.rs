//! Embeds the simulation results UI in the binary when it has been built: trunk puts it in
//! `crates/sim-web/dist` (D-201).

use tempest_assets::build::embed;

fn main() {
    embed(env!("CARGO_MANIFEST_DIR"), "sim-web", "sim_web_assets.rs");
}
