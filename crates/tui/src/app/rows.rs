//! The Settings area's rows: the engine's settings as sections of rows (D-196).

use super::App;
use crate::form::{Field, Form};
use crate::format;
use crate::prefs;
use serde_json::Value;
use std::collections::BTreeMap;
use tempest_api::network;
use tempest_api::schedule::{Schedule, TimeOfDay, Weekday};
use tempest_api::settings::SettingsInfo;
pub use tempest_api::settings::ValueKind;
use tempest_api::settings::{keys, text};

/// What changing a row does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    /// Nothing: it only shows something.
    None,
    /// Asks for a new value of its setting.
    Value(ValueKind),
    /// Switches its setting, which is on or off now, to the other.
    Toggle(bool),
    /// Steps its setting through these values.
    Cycle(&'static [&'static str]),
    /// Opens the alternative limits' schedule.
    Schedule,
    /// Steps through the themes.
    Theme,
    /// Opens the column picker.
    Columns,
    /// Asks for the category's save folder; `x` removes it.
    Category(String),
    /// Asks for a new category.
    NewCategory,
    /// Asks for a new URL; `x` removes it.
    Tracker(String),
    /// Asks for a tracker's URL.
    NewTracker,
    /// Asks for a new identity; `x` removes it.
    Relay(String),
    /// Asks for a relay's identity.
    NewRelay,
    /// Shows the transfers with the tag.
    Tag(String),
}

/// A line of a section: a label, a value, and what changing it does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// What it is.
    pub label: String,
    /// Its value as shown.
    pub value: String,
    /// The setting it shows, by dotted key, if it shows one.
    pub key: Option<&'static str>,
    /// What changing it does.
    pub edit: Edit,
    /// A sentence on what it means, shown while the cursor is on it.
    pub help: String,
}

impl Row {
    fn new(label: impl Into<String>, value: impl Into<String>, edit: Edit) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            key: None,
            edit,
            help: String::new(),
        }
    }

    fn setting(
        label: &str,
        key: &'static str,
        value: impl Into<String>,
        edit: Edit,
        help: &str,
    ) -> Self {
        Self {
            key: Some(key),
            help: help.to_owned(),
            ..Self::new(label, value, edit)
        }
    }

    /// A row for the setting `field` names and explains.
    fn field(field: text::Field, value: impl Into<String>, edit: Edit) -> Self {
        Self::setting(field.label, field.key, value, edit, field.help)
    }

    fn help(mut self, help: impl Into<String>) -> Self {
        self.help = help.into();
        self
    }
}

/// A titled group of rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// Its title.
    pub title: &'static str,
    /// A sentence under the title, if it needs one.
    pub note: Option<String>,
    /// Its rows.
    pub rows: Vec<Row>,
}

/// What sharing does at its limit, as the setting's values.
const ACTIONS: [&str; 2] = [text::ACTIONS[0].0, text::ACTIONS[1].0];

impl Section {
    fn new(section: text::Section, rows: Vec<Row>) -> Self {
        Self {
            title: section.title,
            note: section.note.map(str::to_owned),
            rows,
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

/// A schedule's days and times: "Mon–Fri 09:00–17:00".
pub fn describe_schedule(schedule: &Schedule) -> String {
    schedule.describe()
}

/// A switch as a row's value.
fn switch(on: bool) -> &'static str {
    if on { "[ ●] on" } else { "[● ] off" }
}

/// `text` with its first letter in upper case.
pub(super) fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// A count limit as shown: `3`, or `no limit`.
fn count(limit: Option<u32>) -> String {
    limit.map_or_else(|| "no limit".to_owned(), |n| n.to_string())
}

/// The Settings area's sections, in the order of [`text::SECTIONS`].
pub fn settings_sections(app: &App, view: &SettingsInfo) -> Vec<Section> {
    let settings = &view.settings;
    let limits = &settings.limits;
    let alternative = &limits.alternative;
    let sharing = serde_json::to_value(&settings.sharing).expect("settings serialize");
    let shown = |value: &Value| match value {
        Value::Null => "none".to_owned(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    let limit =
        |field, limit| Row::field(field, format::limit(limit), Edit::Value(ValueKind::Limit));
    let mut sections = vec![
        Section::new(
            text::SPEED,
            vec![
                limit(text::DOWNLOAD, limits.download),
                limit(text::UPLOAD, limits.upload),
            ],
        ),
        Section::new(
            text::ALTERNATIVE,
            vec![
                Row::field(
                    text::ALT_ENABLED,
                    switch(alternative.enabled),
                    Edit::Toggle(alternative.enabled),
                ),
                limit(text::ALT_DOWNLOAD, alternative.download),
                limit(text::ALT_UPLOAD, alternative.upload),
                Row::field(
                    text::ALT_SCHEDULE,
                    alternative
                        .schedule
                        .as_ref()
                        .map_or_else(|| "none".to_owned(), describe_schedule),
                    Edit::Schedule,
                ),
            ],
        )
        .note_if(view.alternative_limits_active, text::ALTERNATIVE_ACTIVE),
        Section::new(
            text::QUEUE,
            vec![
                Row::field(
                    text::MAX_DOWNLOADS,
                    count(settings.queue.max_active_downloads),
                    Edit::Value(ValueKind::Count),
                ),
                Row::field(
                    text::MAX_UPLOADS,
                    count(settings.queue.max_active_uploads),
                    Edit::Value(ValueKind::Count),
                ),
            ],
        ),
        Section::new(
            text::SHARING,
            vec![
                Row::field(
                    text::RATIO,
                    shown(&sharing["ratio"]),
                    Edit::Value(ValueKind::Ratio),
                ),
                Row::field(
                    text::TIME,
                    shown(&sharing["time"]),
                    Edit::Value(ValueKind::Duration),
                ),
                Row::field(
                    text::IDLE_TIMEOUT,
                    shown(&sharing["idle_timeout"]),
                    Edit::Value(ValueKind::Duration),
                ),
                Row::field(
                    text::ACTION,
                    text::action(sharing["action"].as_str().unwrap_or_default()),
                    Edit::Cycle(&ACTIONS),
                ),
            ],
        ),
    ];

    sections.push(network_section(settings));
    sections.extend(peer_finding_sections(view));

    let mut categories = vec![Row::field(
        text::SAVE_FOLDER,
        settings
            .save_folder
            .as_ref()
            .map_or_else(|| text::DEFAULT_FOLDER.to_owned(), |folder| folder.clone()),
        Edit::Value(ValueKind::Folder),
    )];
    match &app.categories {
        None => categories.push(Row::new("Loading…", "", Edit::None)),
        Some(all) => categories.extend(all.iter().map(|(name, folder)| {
            Row::new(
                name.clone(),
                folder.display().to_string(),
                Edit::Category(name.clone()),
            )
            .help(format!("{} x removes it.", text::CATEGORY))
        })),
    }
    categories.push(
        Row::new("+ Add a category", "", Edit::NewCategory)
            .help("A name and the folder its downloads go to."),
    );
    sections.push(Section::new(text::CATEGORIES, categories));

    let mut tags: BTreeMap<&str, usize> = BTreeMap::new();
    for transfer in app.model.transfers() {
        for tag in &transfer.tags {
            *tags.entry(tag.as_str()).or_default() += 1;
        }
    }
    let tag_rows = match tags.is_empty() {
        true => vec![Row::new(text::NO_TAGS, "", Edit::None)],
        false => tags
            .into_iter()
            .map(|(tag, count)| {
                Row::new(
                    format!("#{tag}"),
                    format::count(count, "transfer"),
                    Edit::Tag(tag.to_owned()),
                )
                .help("enter shows its transfers.")
            })
            .collect(),
    };
    sections.push(Section::new(text::TAGS, tag_rows));

    let columns: Vec<&str> = app.columns.iter().map(|c| c.title()).collect();
    sections.push(Section::new(
        text::APPEARANCE,
        vec![
            Row::setting(
                "Theme",
                prefs::THEME,
                app.theme.name,
                Edit::Theme,
                "The palette. T changes it anywhere.",
            ),
            Row::setting(
                "Columns",
                prefs::COLUMNS,
                if columns.is_empty() {
                    text::NO_COLUMNS.to_owned()
                } else {
                    columns.join(" ")
                },
                Edit::Columns,
                "The transfers list's optional columns. | changes them from the list.",
            ),
        ],
    ));
    sections
}

/// The network defaults, each a switch with a sentence on what it shares (D-227).
fn network_section(settings: &tempest_api::settings::Settings) -> Section {
    let on = |key: &str| match key {
        keys::PORT_MAPPING => settings.port_mapping.enabled,
        keys::LAN => settings.lan.enabled,
        _ => settings.relays.from_trackers,
    };
    let rows = network::FEATURES
        .iter()
        .map(|feature| {
            let on = on(feature.key);
            Row::setting(
                feature.label,
                feature.key,
                switch(on),
                Edit::Toggle(on),
                feature.says,
            )
        })
        .collect();
    let mut section = Section::new(text::NETWORK, rows);
    // The TUI shows only the help of the row under the cursor.
    section.note = section
        .note
        .map(|note| format!("{note} Move to a row to read what it shares."));
    section
}

/// The trackers every transfer announces to, and the relays, as sections of Settings.
fn peer_finding_sections(view: &SettingsInfo) -> [Section; 2] {
    let settings = &view.settings;
    let mut trackers: Vec<Row> = settings
        .trackers
        .iter()
        .map(|url| {
            Row::new(url.clone(), "", Edit::Tracker(url.clone()))
                .help("enter changes the URL; x removes it.")
        })
        .collect();
    trackers.push(
        Row::new("+ Add a tracker", "", Edit::NewTracker)
            .help("An http:// or https:// URL. Every transfer announces to it."),
    );
    let mut relays: Vec<Row> = Vec::new();
    relays.extend(relay_identities(view).into_iter().map(|relay| {
        Row::new(relay.clone(), "", Edit::Relay(relay)).help("enter changes it; x removes it.")
    }));
    relays.push(
        Row::new("+ Add a relay", "", Edit::NewRelay)
            .help("A relay's key and address, as <key>@host[:port]."),
    );
    [
        Section::new(text::TRACKERS, trackers),
        Section::new(text::RELAYS, relays),
    ]
}

/// The known relays as `<key>@host:port`.
pub(super) fn relay_identities(view: &SettingsInfo) -> Vec<String> {
    let relays = serde_json::to_value(&view.settings.relays).expect("settings serialize");
    relays["known"]
        .as_array()
        .map(|known| {
            known
                .iter()
                .filter_map(|relay| relay.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// The schedule form, filled in from `schedule`, or for a weekday's working hours.
pub(super) fn schedule_form(schedule: Option<&Schedule>) -> Form {
    let on = |day: usize| schedule.map_or(day < 5, |s| s.days.contains(&Weekday::ALL[day]));
    let time = |time: Option<TimeOfDay>| time.map(|t| t.to_string()).unwrap_or_default();
    Form::new(vec![
        Field::days("Days", std::array::from_fn(on)),
        Field::text("From", &time(schedule.map(|s| s.from)), "09:00"),
        Field::text("To", &time(schedule.map(|s| s.to)), "17:00"),
    ])
}

/// The schedule `form` asks for, as the setting's value: `null` when both times are empty.
pub(super) fn schedule_value(form: &Form) -> Result<Value, String> {
    let days: [bool; 7] = std::array::from_fn(|day| form.days("Days")[day]);
    let schedule = Schedule::from_form(days, &form.text("From"), &form.text("To"))?;
    Ok(serde_json::to_value(schedule).expect("schedules serialize"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_schedule_form_reads_back_as_a_setting() {
        let mut form = schedule_form(None);
        assert_eq!(schedule_value(&form), Ok(Value::Null));
        form.fields[1] = Field::text("From", "22:00", "");
        form.fields[2] = Field::text("To", "7:30", "");
        assert_eq!(
            schedule_value(&form),
            Ok(
                json!({ "days": ["mon", "tue", "wed", "thu", "fri"], "from": "22:00", "to": "07:30" })
            )
        );
        form.fields[0] = Field::days("Days", [false; 7]);
        assert!(schedule_value(&form).is_err());
        form.fields[2] = Field::text("To", "25:00", "");
        assert!(schedule_value(&form).is_err());
    }
}
