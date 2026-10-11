//! What the Settings area says in both clients (D-196, D-203): each setting's label and a
//! sentence on it, and each section's title and note. The clients choose the controls and the
//! order of the rows within a section; the sections come in the order of [`SECTIONS`].

use super::keys;

/// A setting as Settings names and explains it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Field {
    /// The setting, by dotted key.
    pub key: &'static str,
    /// Its name.
    pub label: &'static str,
    /// A sentence on what it means.
    pub help: &'static str,
}

/// A titled group of rows, and the sentence under its title, if it has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Section {
    /// Unique among the sections, for the web UI's element IDs.
    pub id: &'static str,
    /// Its title.
    pub title: &'static str,
    /// The sentence under the title.
    pub note: Option<&'static str>,
}

const fn section(id: &'static str, title: &'static str, note: Option<&'static str>) -> Section {
    Section { id, title, note }
}

/// The overall speed limits.
pub const SPEED: Section = section("speed", "Speed limits", None);
/// The alternative speed limits; [`ALTERNATIVE_ACTIVE`] is its note while they apply.
pub const ALTERNATIVE: Section = section(
    "alternative",
    "Alternative limits",
    Some("While switched on or scheduled, these replace the speed limits."),
);
/// The alternative limits' note while they apply.
pub const ALTERNATIVE_ACTIVE: &str = "Active now, in place of the speed limits.";
/// How many transfers run at once.
pub const QUEUE: Section = section("queue", "Queue", None);
/// When a complete transfer stops sharing.
pub const SHARING: Section = section(
    "sharing",
    "Sharing limits",
    Some("When a complete transfer stops sharing. A transfer can have its own limits."),
);
/// The network defaults, each a switch with a sentence on what it shares (D-227).
pub const NETWORK: Section = section(
    "network",
    "Network",
    Some("On by default so peers can reach you behind a home router. Each can be turned off."),
);
/// The trackers every transfer announces to.
pub const TRACKERS: Section = section(
    "trackers",
    "Trackers",
    Some(
        "Transfers announce themselves to these trackers, plus the ones named in their link, \
         so peers can find them.",
    ),
);
/// The relays to pick a home relay from.
pub const RELAYS: Section = section(
    "relays",
    "Relays",
    Some(
        "The fastest becomes your home relay. Peers that can't reach you directly connect \
         through it.",
    ),
);
/// The default folder and the categories, each with its folder.
pub const CATEGORIES: Section = section("categories", "Categories", None);
/// The tags transfers have.
pub const TAGS: Section = section("tags", "Tags", None);
/// The look and the list's columns.
pub const APPEARANCE: Section = section("appearance", "Appearance", None);

/// Every section, in the order both clients show them.
pub const SECTIONS: [Section; 10] = [
    SPEED,
    ALTERNATIVE,
    QUEUE,
    SHARING,
    NETWORK,
    TRACKERS,
    RELAYS,
    CATEGORIES,
    TAGS,
    APPEARANCE,
];

/// The download limit.
pub const DOWNLOAD: Field = Field {
    key: keys::DOWNLOAD,
    label: "Download",
    help: "Maximum combined download speed of all transfers, in bytes per second.",
};
/// The upload limit.
pub const UPLOAD: Field = Field {
    key: keys::UPLOAD,
    label: "Upload",
    help: "Maximum combined upload speed of all transfers, in bytes per second.",
};
/// Whether the alternative limits are switched on.
pub const ALT_ENABLED: Field = Field {
    key: keys::ALT_ENABLED,
    label: "Switched on",
    help: "On: the alternative limits always apply, whatever the schedule says.",
};
/// The alternative download limit.
pub const ALT_DOWNLOAD: Field = Field {
    key: keys::ALT_DOWNLOAD,
    label: "Download",
    help: "The download limit while the alternative limits apply.",
};
/// The alternative upload limit.
pub const ALT_UPLOAD: Field = Field {
    key: keys::ALT_UPLOAD,
    label: "Upload",
    help: "The upload limit while the alternative limits apply.",
};
/// The alternative limits' schedule.
pub const ALT_SCHEDULE: Field = Field {
    key: keys::ALT_SCHEDULE,
    label: "Schedule",
    help: "When the alternative limits switch on automatically, in the daemon's local time.",
};
/// How many downloads run at once.
pub const MAX_DOWNLOADS: Field = Field {
    key: keys::MAX_DOWNLOADS,
    label: "Active downloads",
    help: "How many downloads run at once. The rest wait in the queue.",
};
/// How many complete transfers share at once.
pub const MAX_UPLOADS: Field = Field {
    key: keys::MAX_UPLOADS,
    label: "Active uploads",
    help: "How many complete transfers share at once. The rest wait in the queue.",
};
/// The sharing ratio limit.
pub const RATIO: Field = Field {
    key: keys::RATIO,
    label: "Ratio",
    help: "Stop sharing after uploading this many times the size downloaded (2 is twice).",
};
/// The sharing time limit.
pub const TIME: Field = Field {
    key: keys::TIME,
    label: "Time",
    help: "Stop sharing this long after the download completes.",
};
/// The sharing idle timeout.
pub const IDLE_TIMEOUT: Field = Field {
    key: keys::IDLE_TIMEOUT,
    label: "Idle timeout",
    help: "Stop sharing when no peer has requested data for this long.",
};
/// What happens at a sharing limit; [`ACTIONS`] names its values.
pub const ACTION: Field = Field {
    key: keys::ACTION,
    label: "When a limit is reached",
    help: "What happens to a complete transfer that reaches a limit.",
};
/// The default download folder; [`DEFAULT_FOLDER`] is its value when unset.
pub const SAVE_FOLDER: Field = Field {
    key: keys::SAVE_FOLDER,
    label: "Default folder",
    help: "Where downloads are saved, unless their category or the Add dialog sets another folder.",
};

/// What the sharing action's values do, in its order.
pub const ACTIONS: [(&str, &str); 2] = [
    ("stop", "stop sharing"),
    ("remove", "remove from list, keep files"),
];
/// Where downloads go when no default folder is set.
pub const DEFAULT_FOLDER: &str = "Downloads folder in the home folder";
/// What a category row means.
pub const CATEGORY: &str = "New downloads in this category are saved to its folder.";
/// What the Tags section says while no transfer has a tag.
pub const NO_TAGS: &str = "No tags yet. Tag a transfer from its detail.";
/// The Columns row's value when no optional column shows.
pub const NO_COLUMNS: &str = "only name, progress and status";

/// The name of a sharing action's value, or the value itself if it is unknown.
pub fn action(value: &str) -> &str {
    ACTIONS
        .iter()
        .find(|(known, _)| *known == value)
        .map_or(value, |(_, name)| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_and_help_read_as_sentences() {
        let ids: std::collections::BTreeSet<_> = SECTIONS.iter().map(|s| s.id).collect();
        assert_eq!(ids.len(), SECTIONS.len(), "section IDs are unique");
        let fields = [
            DOWNLOAD,
            UPLOAD,
            ALT_ENABLED,
            ALT_DOWNLOAD,
            ALT_UPLOAD,
            ALT_SCHEDULE,
            MAX_DOWNLOADS,
            MAX_UPLOADS,
            RATIO,
            TIME,
            IDLE_TIMEOUT,
            ACTION,
            SAVE_FOLDER,
        ];
        for field in fields {
            assert!(field.help.ends_with('.'), "{}", field.key);
        }
        assert_eq!(action("remove"), "remove from list, keep files");
        assert_eq!(action("later"), "later");
    }
}
