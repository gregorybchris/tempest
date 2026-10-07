//! Reads scenario files from arbitrary text: parsing and checking must fail cleanly, never
//! panic, and whatever passes must give every class a config in every variant.

#![no_main]

use libfuzzer_sys::fuzz_target;
use tempest_sim::Scenario;

fuzz_target!(|text: &str| {
    if let Ok(scenario) = Scenario::from_yaml(text) {
        for variant in scenario.variants() {
            for class in 0..scenario.peers.len() {
                scenario.config(class, &variant).unwrap();
            }
        }
    }
});
