//! What the Settings area lists (D-196): sections of rows, each a label, its value
//! and the control that changes it. This is plain data built from the engine's settings, so it
//! is tested here; the components only draw it.
//!
//! Every row has an ID, unique in its area, which the keyboard's cursor and the page's DOM IDs
//! use.

use std::collections::BTreeMap;

use tempest_api::TransferInfo;
use tempest_api::columns::Column;
use tempest_api::network;
use tempest_api::schedule::Schedule;
use tempest_api::settings::text::{self, Field};
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
    fn new(section: text::Section, rows: Vec<Row>) -> Self {
        Self {
            id: section.id,
            title: section.title,
            note: section.note.map(str::to_owned),
            rows,
            adds: None,
        }
    }

    /// The section with `note` in place of its own when `when` holds.
    fn note_if(mut self, when: bool, note: &str) -> Self {
        if when {
            self.note = Some(note.to_owned());
        }
        self
    }
}

/// A count limit in words.
fn count(limit: Option<u32>) -> String {
    limit.map_or_else(|| "no limit".to_owned(), |n| n.to_string())
}

/// A row for setting `field`, marked for reset if it was changed through the API.
fn setting(info: &SettingsInfo, field: Field, control: Control) -> Row {
    let mut row = Row::new(field.key, field.label, control).help(field.help);
    if info.changed.contains_key(field.key) {
        row.reset = Some(field.key);
    }
    row
}

/// A row for `field`, with a text field that starts with `text`.
fn value_row(info: &SettingsInfo, field: Field, kind: ValueKind, text: String) -> Row {
    let key = field.key;
    setting(info, field, Control::Value { key, kind, text })
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

/// The Settings area's sections, in the order of [`text::SECTIONS`].
pub fn settings_sections(info: &SettingsInfo, extras: &Extras) -> Vec<Section> {
    let settings = &info.settings;
    let limits = &settings.limits;
    let alternative = &limits.alternative;
    let sharing = &settings.sharing;
    let limit_row = |field: Field, limit: Option<u64>| {
        value_row(info, field, ValueKind::Limit, units::limit_input(limit))
            .value(units::limit(limit))
    };
    let none = |value: Option<String>| value.unwrap_or_else(|| "none".to_owned());
    let mut sections = vec![
        Section::new(
            text::SPEED,
            vec![
                limit_row(text::DOWNLOAD, limits.download),
                limit_row(text::UPLOAD, limits.upload),
            ],
        ),
        Section::new(
            text::ALTERNATIVE,
            vec![
                setting(
                    info,
                    text::ALT_ENABLED,
                    Control::Toggle {
                        key: keys::ALT_ENABLED,
                        on: alternative.enabled,
                    },
                )
                .value(if alternative.enabled { "on" } else { "off" }),
                limit_row(text::ALT_DOWNLOAD, alternative.download),
                limit_row(text::ALT_UPLOAD, alternative.upload),
                setting(
                    info,
                    text::ALT_SCHEDULE,
                    Control::Schedule(alternative.schedule.clone()),
                )
                .value(none(alternative.schedule.as_ref().map(Schedule::describe))),
            ],
        )
        .note_if(info.alternative_limits_active, text::ALTERNATIVE_ACTIVE),
        Section::new(
            text::QUEUE,
            vec![
                value_row(
                    info,
                    text::MAX_DOWNLOADS,
                    ValueKind::Count,
                    count_input(settings.queue.max_active_downloads),
                )
                .value(count(settings.queue.max_active_downloads)),
                value_row(
                    info,
                    text::MAX_UPLOADS,
                    ValueKind::Count,
                    count_input(settings.queue.max_active_uploads),
                )
                .value(count(settings.queue.max_active_uploads)),
            ],
        ),
        Section::new(
            text::SHARING,
            vec![
                value_row(
                    info,
                    text::RATIO,
                    ValueKind::Ratio,
                    sharing.ratio.map(|r| r.to_string()).unwrap_or_default(),
                )
                .value(none(sharing.ratio.map(|r| r.to_string()))),
                value_row(
                    info,
                    text::TIME,
                    ValueKind::Duration,
                    sharing.time.clone().unwrap_or_default(),
                )
                .value(none(sharing.time.clone())),
                value_row(
                    info,
                    text::IDLE_TIMEOUT,
                    ValueKind::Duration,
                    sharing.idle_timeout.clone().unwrap_or_default(),
                )
                .value(none(sharing.idle_timeout.clone())),
                setting(
                    info,
                    text::ACTION,
                    Control::Choice {
                        key: keys::ACTION,
                        options: &text::ACTIONS,
                        chosen: sharing.action.clone(),
                    },
                )
                .value(text::action(&sharing.action)),
            ],
        ),
        network_section(info),
    ];
    sections.extend(peer_finding_sections(info));

    let mut categories = vec![
        value_row(
            info,
            text::SAVE_FOLDER,
            ValueKind::Folder,
            settings.save_folder.clone().unwrap_or_default(),
        )
        .value(
            settings
                .save_folder
                .clone()
                .unwrap_or_else(|| text::DEFAULT_FOLDER.to_owned()),
        ),
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
            .help(text::CATEGORY)
        }));
    }
    sections.push(Section {
        adds: Some(Adds::Category),
        ..Section::new(text::CATEGORIES, categories)
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
    sections.push(Section::new(text::TAGS, tags).note_if(extras.tags.is_empty(), text::NO_TAGS));

    let ui = &settings.ui;
    let on = |on: bool| if on { "on" } else { "off" };
    let columns: Vec<&str> = extras.columns.iter().map(|c| c.title()).collect();
    sections.push(Section::new(
        text::APPEARANCE,
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
                    true => text::NO_COLUMNS.to_owned(),
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

/// The network defaults, each a switch with a sentence on what it shares (D-227).
fn network_section(info: &SettingsInfo) -> Section {
    let settings = &info.settings;
    let rows = network::FEATURES
        .iter()
        .map(|feature| {
            let on = match feature.key {
                keys::PORT_MAPPING => settings.port_mapping.enabled,
                keys::LAN => settings.lan.enabled,
                _ => settings.relays.from_trackers,
            };
            let field = Field {
                key: feature.key,
                label: feature.label,
                help: feature.says,
            };
            let control = Control::Toggle {
                key: feature.key,
                on,
            };
            setting(info, field, control).value(if on { "on" } else { "off" })
        })
        .collect();
    Section::new(text::NETWORK, rows)
}

/// The trackers every transfer announces to and the relays, as sections of Settings.
fn peer_finding_sections(info: &SettingsInfo) -> Vec<Section> {
    let settings = &info.settings;
    let trackers = settings
        .trackers
        .iter()
        .map(|url| {
            Row::new(
                format!("tracker:{url}"),
                "Tracker",
                Control::Tracker(url.clone()),
            )
            .help("Every transfer announces to it.")
        })
        .collect();
    let mut relays: Vec<Row> = Vec::new();
    relays.extend(settings.relays.known.iter().map(|relay| {
        Row::new(
            format!("relay:{relay}"),
            "Relay",
            Control::Relay(relay.clone()),
        )
        .help("A relay to pick the home relay from.")
    }));
    vec![
        Section {
            adds: Some(Adds::Tracker),
            ..Section::new(text::TRACKERS, trackers)
        },
        Section {
            adds: Some(Adds::Relay),
            ..Section::new(text::RELAYS, relays)
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
                "port_mapping": { "enabled": true },
                "lan": { "enabled": false },
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
    fn settings_list_trackers_and_relays_after_the_network_defaults() {
        let sections = settings_sections(&info(), &extras());
        let at = |id| sections.iter().position(|s| s.id == id).unwrap();
        assert_eq!(at("trackers"), at("network") + 1);
        assert_eq!(at("relays"), at("trackers") + 1);
        let trackers = &sections[at("trackers")];
        assert_eq!(
            trackers.rows[0].control,
            Control::Tracker("https://tracker.example.org/announce".to_owned())
        );
        assert_eq!(trackers.adds, Some(Adds::Tracker));
        assert_eq!(sections[at("relays")].adds, Some(Adds::Relay));
    }

    #[test]
    fn the_network_defaults_are_switches_with_a_sentence_each() {
        let sections = settings_sections(&info(), &extras());
        let on = |key| match &row(&sections, key).control {
            Control::Toggle { on, .. } => *on,
            other => panic!("{other:?}"),
        };
        assert!(on(keys::PORT_MAPPING));
        assert!(on(keys::RELAYS_FROM_TRACKERS));
        assert!(!on(keys::LAN));
        assert_eq!(row(&sections, keys::LAN).value, "off");
        for feature in network::FEATURES {
            assert_eq!(row(&sections, feature.key).help, feature.says);
        }
    }

    #[test]
    fn row_ids_are_unique_in_each_area() {
        let sections = settings_sections(&info(), &extras());
        let mut seen = HashSet::new();
        for row in sections.iter().flat_map(|s| &s.rows) {
            assert!(seen.insert(row.id.clone()), "{} twice", row.id);
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
    fn the_cursor_steps_through_rows() {
        let ids: Vec<String> = ["a", "b", "c"].map(str::to_owned).to_vec();
        assert_eq!(step(&ids, None, Move::Down).as_deref(), Some("a"));
        assert_eq!(step(&ids, Some("a"), Move::Down).as_deref(), Some("b"));
        assert_eq!(step(&ids, Some("c"), Move::Down).as_deref(), Some("c"));
        assert_eq!(step(&ids, Some("gone"), Move::Up).as_deref(), Some("c"));
        assert_eq!(step(&[], None, Move::Down), None);
    }
}
