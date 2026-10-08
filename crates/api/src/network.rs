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
    /// One sentence on what it does and what it reveals.
    pub says: &'static str,
}

/// Port mapping, relays from trackers and LAN discovery, in the order Settings shows them.
pub const FEATURES: [Feature; 3] = [
    Feature {
        key: keys::PORT_MAPPING,
        label: "Port mapping",
        says: "Asks your router to open this peer's port so others can connect directly; \
               the router learns which port you use.",
    },
    Feature {
        key: keys::RELAYS_FROM_TRACKERS,
        label: "Relays from trackers",
        says: "Takes a relay a tracker recommends, which lends you an address behind a \
               router; that relay sees who talks to whom.",
    },
    Feature {
        key: keys::LAN,
        label: "LAN discovery",
        says: "Finds peers on your local network and lets them find you; people on it see \
               which bundles you ask about.",
    },
];

/// What the first run says before the features.
pub const INTRO: &str = "These are on so that sending directly works behind home routers. \
                         Each shares something, and Settings switches each off.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_feature_says_one_sentence() {
        for feature in FEATURES {
            assert_eq!(feature.says.matches(". ").count(), 0, "{}", feature.label);
            assert!(feature.says.ends_with('.'), "{}", feature.label);
        }
    }
}
