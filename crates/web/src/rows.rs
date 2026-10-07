//! What the Catalog and Settings areas list (D-196): sections of rows, each a label, its value
//! and the control that changes it. This is plain data built from the engine's settings, so it
//! is tested here; the components only draw it.
//!
//! Every row has an ID, unique in its area, which the keyboard's cursor and the page's DOM IDs
//! use.

use std::collections::BTreeMap;

use tempest_api::TransferInfo;
use tempest_api::columns::Column;
use tempest_api::schedule::Schedule;
use tempest_api::settings::{SettingsInfo, ValueKind, keys};
use tempest_api::units;

/// What changes a row.
#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    /// A text field for a value of `kind`, starting with `text`, the exact value.
    Value {
        /// The setting.
        key: &'static str,
        /// What it takes.
        kind: ValueKind,
        /// What the field starts with.
        text: String,
    },
    /// A switch.
    Toggle {
        /// The setting.
        key: &'static str,
        /// Whether it is on.
        on: bool,
    },
    /// One of a few choices.
    Choice {
        /// The setting.
        key: &'static str,
        /// Each choice's value and name.
        options: &'static [(&'static str, &'static str)],
        /// The value chosen.
        chosen: String,
    },
    /// The alternative limits' days and times.
    Schedule(Option<Schedule>),
    /// The palette.
    Theme,
    /// Scanlines over the page.
    Scanlines,
    /// No glow, scanlines or animation.
    Clean,
    /// This browser's notifications.
    Notifications,
    /// The list's optional columns.
    Columns,
    /// A category's save folder; it can be removed.
    Category {
        /// Its name.
        name: String,
        /// Its folder.
        folder: String,
    },
    /// A tracker URL; it can be changed and removed.
    Tracker(String),
    /// A relay identity; it can be changed and removed.
    Relay(String),
    /// A tag: the transfers with it.
    Tag(String),
}

/// A line of a section.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// Unique in its area.
    pub id: String,
    /// What it is.
    pub label: String,
    /// Its value in words.
    pub value: String,
    /// What changes it.
    pub control: Control,
    /// What it means.
    pub help: String,
    /// The setting it shows, when it was changed through the API and `x` can give it the
    /// config file's value again.
    pub reset: Option<&'static str>,
}

impl Row {
    fn new(id: impl Into<String>, label: impl Into<String>, control: Control) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            value: String::new(),
            control,
            help: String::new(),
            reset: None,
        }
    }

    fn value(mut self, value: impl Into<String>) -> Self {
        self.value = value.into();
        self
    }

    fn help(mut self, help: impl Into<String>) -> Self {
        self.help = help.into();
        self
    }
}

/// What a section's add field adds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Adds {
    /// A category with its folder.
    Category,
    /// A tracker URL.
    Tracker,
    /// A relay identity.
    Relay,
}

/// A titled group of rows.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    /// Unique in its area.
    pub id: &'static str,
    /// Its title.
    pub title: &'static str,
    /// A sentence under the title, if it needs one.
    pub note: Option<String>,
    /// Its rows.
    pub rows: Vec<Row>,
    /// What its add field adds, if it has one.
    pub adds: Option<Adds>,
}

impl Section {
    fn new(id: &'static str, title: &'static str, rows: Vec<Row>) -> Self {
        Self {
            id,
            title,
            note: None,
            rows,
            adds: None,
        }
    }

    fn note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }
}

/// What sharing does at its limit.
const ACTIONS: &[(&str, &str)] = &[("stop", "stop it"), ("remove", "remove it, keep files")];

/// A count limit in words.
fn count(limit: Option<u32>) -> String {
    limit.map_or_else(|| "no limit".to_owned(), |n| n.to_string())
}

/// A row for setting `key`, marked for reset if it was changed through the API.
fn setting(info: &SettingsInfo, key: &'static str, label: &str, control: Control) -> Row {
    let mut row = Row::new(key, label, control);
    if info.changed.contains_key(key) {
        row.reset = Some(key);
    }
    row
}

fn value_row(
    info: &SettingsInfo,
    key: &'static str,
    label: &str,
    kind: ValueKind,
    text: String,
) -> Row {
    setting(info, key, label, Control::Value { key, kind, text })
}

/// What the Settings area shows besides the engine's settings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Extras {
    /// Each category's folder, by name, once known.
    pub categories: Option<BTreeMap<String, String>>,
    /// Each tag and how many transfers have it.
    pub tags: BTreeMap<String, usize>,
    /// The optional columns shown.
    pub columns: Vec<Column>,
}

/// Each tag of `transfers` and how many have it.
pub fn tag_counts(transfers: &[TransferInfo]) -> BTreeMap<String, usize> {
    let mut tags = BTreeMap::new();
    for tag in transfers.iter().flat_map(|t| &t.tags) {
        *tags.entry(tag.clone()).or_default() += 1;
    }
    tags
}

/// The Settings area's sections.
pub fn settings_sections(info: &SettingsInfo, extras: &Extras) -> Vec<Section> {
    let settings = &info.settings;
    let limits = &settings.limits;
    let alternative = &limits.alternative;
    let sharing = &settings.sharing;
    let limit_row = |key, label, limit: Option<u64>, help: &str| {
        value_row(
            info,
            key,
            label,
            ValueKind::Limit,
            units::limit_input(limit),
        )
        .value(units::limit(limit))
        .help(help)
    };
    let mut sections = vec![
        Section::new(
            "speed",
            "Speed limits",
            vec![
                limit_row(
                    keys::DOWNLOAD,
                    "Download",
                    limits.download,
                    "The most every transfer together receives per second.",
                ),
                limit_row(
                    keys::UPLOAD,
                    "Upload",
                    limits.upload,
                    "The most every transfer together sends per second.",
                ),
            ],
        ),
        Section::new(
            "alternative",
            "Alternative limits",
            vec![
                setting(
                    info,
                    keys::ALT_ENABLED,
                    "Switched on",
                    Control::Toggle {
                        key: keys::ALT_ENABLED,
                        on: alternative.enabled,
                    },
                )
                .value(if alternative.enabled { "on" } else { "off" })
                .help("On, they apply whatever the schedule says."),
                limit_row(
                    keys::ALT_DOWNLOAD,
                    "Download",
                    alternative.download,
                    "The download limit while the alternative limits apply.",
                ),
                limit_row(
                    keys::ALT_UPLOAD,
                    "Upload",
                    alternative.upload,
                    "The upload limit while the alternative limits apply.",
                ),
                setting(
                    info,
                    keys::ALT_SCHEDULE,
                    "Schedule",
                    Control::Schedule(alternative.schedule.clone()),
                )
                .value(
                    alternative
                        .schedule
                        .as_ref()
                        .map_or_else(|| "none".to_owned(), Schedule::describe),
                )
                .help("When they apply on their own, in the engine's local time."),
            ],
        )
        .note(match info.alternative_limits_active {
            true => "They apply now, in place of the speed limits.",
            false => "They replace the speed limits while on, or while scheduled.",
        }),
        Section::new(
            "queue",
            "Queue",
            vec![
                value_row(
                    info,
                    keys::MAX_DOWNLOADS,
                    "Active downloads",
                    ValueKind::Count,
                    count_input(settings.queue.max_active_downloads),
                )
                .value(count(settings.queue.max_active_downloads))
                .help("How many downloads run at once; the rest wait as queued."),
                value_row(
                    info,
                    keys::MAX_UPLOADS,
                    "Active shares",
                    ValueKind::Count,
                    count_input(settings.queue.max_active_uploads),
                )
                .value(count(settings.queue.max_active_uploads))
                .help("How many complete transfers share at once; the rest wait as queued."),
            ],
        ),
        Section::new(
            "sharing",
            "Sharing limits",
            vec![
                value_row(
                    info,
                    keys::RATIO,
                    "Ratio",
                    ValueKind::Ratio,
                    sharing.ratio.map(|r| r.to_string()).unwrap_or_default(),
                )
                .value(sharing.ratio.map_or("none".to_owned(), |r| r.to_string()))
                .help("Stop sharing once this many bytes are sent per byte received."),
                value_row(
                    info,
                    keys::TIME,
                    "Time",
                    ValueKind::Duration,
                    sharing.time.clone().unwrap_or_default(),
                )
                .value(sharing.time.clone().unwrap_or_else(|| "none".to_owned()))
                .help("Stop sharing after this long complete."),
                setting(
                    info,
                    keys::ACTION,
                    "At the limit",
                    Control::Choice {
                        key: keys::ACTION,
                        options: ACTIONS,
                        chosen: sharing.action.clone(),
                    },
                )
                .value(
                    ACTIONS
                        .iter()
                        .find(|(value, _)| *value == sharing.action)
                        .map_or(sharing.action.as_str(), |(_, name)| name),
                )
                .help("What happens to a complete transfer that reaches a limit."),
            ],
        )
        .note("When a complete transfer stops sharing. L sets a transfer's own."),
    ];

    let mut categories = vec![
        value_row(
            info,
            keys::SAVE_FOLDER,
            "Default folder",
            ValueKind::Folder,
            settings.save_folder.clone().unwrap_or_default(),
        )
        .value(
            settings
                .save_folder
                .clone()
                .unwrap_or_else(|| "Downloads, in the home folder".to_owned()),
        )
        .help("Where downloads go unless their category or the add says otherwise."),
    ];
    if let Some(all) = &extras.categories {
        categories.extend(all.iter().map(|(name, folder)| {
            Row::new(
                format!("category:{name}"),
                name.clone(),
                Control::Category {
                    name: name.clone(),
                    folder: folder.clone(),
                },
            )
            .value(folder.clone())
            .help("Downloads added in this category go to its folder.")
        }));
    }
    sections.push(Section {
        adds: Some(Adds::Category),
        ..Section::new("categories", "Categories", categories)
    });

    let tags = extras
        .tags
        .iter()
        .map(|(tag, n)| {
            Row::new(
                format!("tag:{tag}"),
                format!("#{tag}"),
                Control::Tag(tag.clone()),
            )
            .value(tempest_api::words::count(*n, "transfer"))
            .help("Shows its transfers.")
        })
        .collect();
    let mut tags = Section::new("tags", "Tags", tags);
    if extras.tags.is_empty() {
        tags = tags.note("No tags yet. Press t on a transfer to tag it.");
    }
    sections.push(tags);

    let ui = &settings.ui;
    let on = |on: bool| if on { "on" } else { "off" };
    let columns: Vec<&str> = extras.columns.iter().map(|c| c.title()).collect();
    sections.push(Section::new(
        "appearance",
        "Appearance",
        vec![
            Row::new("theme", "Theme", Control::Theme)
                .value(ui.theme.clone())
                .help("The palette, shared with the TUI. T changes it anywhere."),
            Row::new("scanlines", "Scanlines", Control::Scanlines)
                .value(on(ui.scanlines))
                .help("Faint lines over the page, like a CRT."),
            Row::new("clean", "Clean", Control::Clean)
                .value(on(ui.clean))
                .help("No glow, scanlines or animation."),
            Row::new("columns", "Columns", Control::Columns)
                .value(match columns.is_empty() {
                    true => "only name, progress and status".to_owned(),
                    false => columns.join(", "),
                })
                .help(
                    "The transfers list's optional columns, shared with the TUI. | from the list.",
                ),
            Row::new("notifications", "Notifications", Control::Notifications).help(
                "A browser notification when a transfer completes or fails. This browser only.",
            ),
        ],
    ));
    sections
}

/// A count limit as its field starts.
fn count_input(limit: Option<u32>) -> String {
    limit.map(|n| n.to_string()).unwrap_or_default()
}

/// The Catalog area's sections. What trackers list is browsed in a later release (roadmap
/// M9); the area says so.
pub fn catalog_sections(info: &SettingsInfo) -> Vec<Section> {
    let settings = &info.settings;
    let trackers = settings
        .trackers
        .iter()
        .map(|url| {
            Row::new(
                format!("tracker:{url}"),
                url.clone(),
                Control::Tracker(url.clone()),
            )
            .help("Every transfer announces to it.")
        })
        .collect();
    let mut relays = vec![
        setting(
            info,
            keys::RELAYS_FROM_TRACKERS,
            "Use relays trackers suggest",
            Control::Toggle {
                key: keys::RELAYS_FROM_TRACKERS,
                on: settings.relays.from_trackers,
            },
        )
        .value(if settings.relays.from_trackers {
            "on"
        } else {
            "off"
        })
        .help("Also pick a home relay from those the trackers recommend."),
    ];
    relays.extend(settings.relays.known.iter().map(|relay| {
        Row::new(
            format!("relay:{relay}"),
            relay.clone(),
            Control::Relay(relay.clone()),
        )
        .help("A relay to pick the home relay from.")
    }));
    vec![
        Section {
            adds: Some(Adds::Tracker),
            ..Section::new("trackers", "Trackers", trackers).note(
                "Every transfer announces to these, as well as to the trackers its link names.",
            )
        },
        Section {
            adds: Some(Adds::Relay),
            ..Section::new("relays", "Relays", relays).note(
                "The fastest becomes the home relay, which lends this peer an address when \
                 others can't reach it directly.",
            )
        },
    ]
}

/// `list` with `old` replaced by `new`, or without it when `new` is `None`, or with `new` added
/// when `old` is `None`. Entries stay unique.
pub fn edited(list: &[String], old: Option<&str>, new: Option<&str>) -> Vec<String> {
    let mut list: Vec<String> = list.to_vec();
    match (old, new) {
        (Some(old), new) => {
            if let Some(i) = list.iter().position(|e| e == old) {
                match new {
                    Some(new) => list[i] = new.to_owned(),
                    None => {
                        list.remove(i);
                    }
                }
            }
        }
        (None, Some(new)) => list.push(new.to_owned()),
        (None, None) => {}
    }
    let mut seen = std::collections::HashSet::new();
    list.retain(|e| seen.insert(e.clone()));
    list
}

/// Why `url` can't be a tracker, if it can't.
pub fn tracker_problem(url: &str) -> Option<&'static str> {
    let url = url.trim();
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    match rest {
        Some(host) if !host.is_empty() && !host.starts_with('/') => None,
        _ => Some("A tracker is an http:// or https:// URL."),
    }
}

/// Why `relay` can't be a relay identity, if it can't: `<key>@host:port`, the key 64 hex
/// characters.
pub fn relay_problem(relay: &str) -> Option<&'static str> {
    let problem = "A relay is its key and address: <64 hex characters>@host:port.";
    let Some((key, address)) = relay.trim().split_once('@') else {
        return Some(problem);
    };
    let key_ok = key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit());
    let address_ok = address
        .rsplit_once(':')
        .is_some_and(|(host, port)| !host.is_empty() && port.parse::<u16>().is_ok());
    (!key_ok || !address_ok).then_some(problem)
}

/// The ID of the row after or before `current` among `ids`, as the list's cursor moves.
pub fn step(ids: &[String], current: Option<&str>, to: crate::list::Move) -> Option<String> {
    let at = current.and_then(|c| ids.iter().position(|id| id == c));
    to.from(at, ids.len()).map(|i| ids[i].clone())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use serde_json::json;

    use super::*;
    use crate::list::Move;

    fn info() -> SettingsInfo {
        serde_json::from_value(json!({
            "settings": {
                "limits": {
                    "download": 2_500_000,
                    "alternative": {
                        "upload": 100_000,
                        "schedule": { "days": ["mon", "tue", "wed", "thu", "fri"], "from": "09:00", "to": "17:00" }
                    }
                },
                "queue": { "max_active_downloads": 3, "max_active_uploads": null },
                "sharing": { "ratio": 2.0, "time": "12h", "action": "stop" },
                "trackers": ["https://tracker.example.org/announce"],
                "relays": { "known": [], "from_trackers": true },
                "ui": { "theme": "amber", "scanlines": true }
            },
            "changed": { "limits.download": 2_500_000 },
            "alternative_limits_active": true
        }))
        .unwrap()
    }

    fn extras() -> Extras {
        Extras {
            categories: Some(BTreeMap::from([(
                "films".to_owned(),
                "/srv/films".to_owned(),
            )])),
            tags: BTreeMap::from([("live".to_owned(), 2)]),
            columns: vec![Column::Size, Column::Tags],
        }
    }

    fn row<'a>(sections: &'a [Section], id: &str) -> &'a Row {
        sections
            .iter()
            .flat_map(|s| &s.rows)
            .find(|r| r.id == id)
            .unwrap_or_else(|| panic!("no row {id}"))
    }

    #[test]
    fn settings_show_values_in_words_and_fields_start_exact() {
        let sections = settings_sections(&info(), &extras());
        let download = row(&sections, keys::DOWNLOAD);
        assert_eq!(download.value, "2.5M/s");
        assert_eq!(download.reset, Some(keys::DOWNLOAD));
        let Control::Value { text, .. } = &download.control else {
            panic!("{download:?}");
        };
        assert_eq!(text, "2500K");
        assert_eq!(row(&sections, keys::UPLOAD).value, "none");
        assert_eq!(row(&sections, keys::UPLOAD).reset, None);
        assert_eq!(
            row(&sections, keys::ALT_SCHEDULE).value,
            "Mon–Fri 09:00–17:00"
        );
        assert_eq!(row(&sections, keys::MAX_UPLOADS).value, "no limit");
        assert_eq!(row(&sections, keys::ACTION).value, "stop it");
        assert_eq!(row(&sections, "category:films").value, "/srv/films");
        assert_eq!(row(&sections, "tag:live").value, "2 transfers");
        assert_eq!(row(&sections, "columns").value, "Size, Tags");
        assert_eq!(row(&sections, "scanlines").value, "on");
        assert_eq!(
            sections[1].note.as_deref(),
            Some("They apply now, in place of the speed limits.")
        );
    }

    #[test]
    fn catalog_lists_trackers_and_relays() {
        let sections = catalog_sections(&info());
        assert_eq!(sections.len(), 2);
        assert_eq!(
            sections[0].rows[0].label,
            "https://tracker.example.org/announce"
        );
        assert_eq!(sections[0].adds, Some(Adds::Tracker));
        assert_eq!(row(&sections, keys::RELAYS_FROM_TRACKERS).value, "on");
    }

    #[test]
    fn row_ids_are_unique_in_each_area() {
        for sections in [
            settings_sections(&info(), &extras()),
            catalog_sections(&info()),
        ] {
            let mut seen = HashSet::new();
            for row in sections.iter().flat_map(|s| &s.rows) {
                assert!(seen.insert(row.id.clone()), "{} twice", row.id);
            }
        }
    }

    #[test]
    fn lists_are_edited_without_duplicates() {
        let list = vec!["a".to_owned(), "b".to_owned()];
        assert_eq!(edited(&list, None, Some("c")), ["a", "b", "c"]);
        assert_eq!(edited(&list, None, Some("a")), ["a", "b"]);
        assert_eq!(edited(&list, Some("a"), Some("z")), ["z", "b"]);
        assert_eq!(edited(&list, Some("a"), Some("b")), ["b"]);
        assert_eq!(edited(&list, Some("b"), None), ["a"]);
        assert_eq!(edited(&list, Some("q"), None), ["a", "b"]);
    }

    #[test]
    fn entries_are_checked_before_sending() {
        assert_eq!(tracker_problem("https://t.example.org/announce"), None);
        assert_eq!(tracker_problem("http://10.0.0.2:4777"), None);
        assert!(tracker_problem("udp://t.example.org").is_some());
        assert!(tracker_problem("https://").is_some());
        let key = "ab".repeat(32);
        assert_eq!(
            relay_problem(&format!("{key}@relay.example.org:4775")),
            None
        );
        assert!(relay_problem(&format!("{key}@relay.example.org")).is_some());
        assert!(relay_problem("abc@relay.example.org:4775").is_some());
        assert!(relay_problem("relay.example.org:4775").is_some());
    }

    #[test]
    fn the_cursor_steps_through_rows() {
        let ids: Vec<String> = ["a", "b", "c"].map(str::to_owned).to_vec();
        assert_eq!(step(&ids, None, Move::Down).as_deref(), Some("a"));
        assert_eq!(step(&ids, Some("a"), Move::Down).as_deref(), Some("b"));
        assert_eq!(step(&ids, Some("c"), Move::Down).as_deref(), Some("c"));
        assert_eq!(step(&ids, Some("gone"), Move::Up).as_deref(), Some("c"));
        assert_eq!(step(&[], None, Move::Down), None);
    }
}
