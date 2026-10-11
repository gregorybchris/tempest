//! The network defaults that reach beyond the machine, and what each one shares (D-227). Both
//! UIs show these words, in Settings and on the first run.

use crate::settings::keys;

/// A network setting that is on by default, and a plain account of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Feature {
    /// The setting, by dotted key.
    pub key: &'static str,
    /// Its name.
    pub label: &'static str,
    /// What it does and what it reveals.
    pub says: &'static str,
}

/// Port mapping, relays from trackers and LAN discovery, in the order Settings shows them.
pub const FEATURES: [Feature; 3] = [
    Feature {
        key: keys::PORT_MAPPING,
        label: "Port mapping",
        says: "Asks your router to open a port so peers can connect to you directly. The router \
               sees which port you use.",
    },
    Feature {
        key: keys::RELAYS_FROM_TRACKERS,
        label: "Relays from trackers",
        says: "Uses relays that trackers recommend, so peers can reach you behind a router. \
               The relay sees which peers talk to each other.",
    },
    Feature {
        key: keys::LAN,
        label: "LAN discovery",
        says: "Finds peers on your local network and lets them find you. Others on the \
               network see which bundles you request.",
    },
];

/// What the first run says before the features.
pub const INTRO: &str = "These are on by default so transfers work behind home routers. Turn each off \
                         in Settings.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_feature_ends_with_a_period() {
        for feature in FEATURES {
            assert!(feature.says.ends_with('.'), "{}", feature.label);
        }
    }
}
