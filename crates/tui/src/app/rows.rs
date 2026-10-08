//! The Settings area: the engine's settings as sections of rows, and what `enter`,
//! `a` and `x` do to the row under the cursor (D-196).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tempest_engine::SettingsView;
use tempest_engine::schedule::{Schedule, TimeOfDay, Weekday};

use super::modal::{FormKind, Modal, ValueEdit, ValueTarget};
use super::request::Request;
use super::{App, Area, View};
use crate::columns::Column;
use crate::form::{Field, Form};
use crate::format;
use crate::prefs;

use tempest_api::network;
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
fn capitalized(text: &str) -> String {
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
pub fn settings_sections(app: &App, view: &SettingsView) -> Vec<Section> {
    let settings = &view.settings;
    let limits = &settings.limits;
    let alternative = &limits.alternative;
    let sharing = serde_json::to_value(settings.sharing).expect("settings serialize");
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
        settings.save_folder.as_ref().map_or_else(
            || text::DEFAULT_FOLDER.to_owned(),
            |folder| folder.display().to_string(),
        ),
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
fn network_section(settings: &tempest_engine::EngineConfig) -> Section {
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
fn peer_finding_sections(view: &SettingsView) -> [Section; 2] {
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
fn relay_identities(view: &SettingsView) -> Vec<String> {
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
fn schedule_form(schedule: Option<&Schedule>) -> Form {
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

pub(super) use crate::format::limit_input;

impl App {
    /// The sections of the area shown, if it lists rows and the settings have loaded.
    pub fn sections(&self) -> Vec<Section> {
        let Some(view) = &self.settings else {
            return Vec::new();
        };
        match self.area {
            Area::Settings => settings_sections(self, view),
            _ => Vec::new(),
        }
    }

    /// The rows of the area shown, in order.
    pub fn rows(&self) -> Vec<Row> {
        self.sections().into_iter().flat_map(|s| s.rows).collect()
    }

    /// Where the cursor is among the area's rows.
    pub fn row_cursor(&self) -> usize {
        let count = self.rows().len();
        self.settings_row.min(count.saturating_sub(1))
    }

    fn set_row_cursor(&mut self, row: usize) {
        self.settings_row = row;
    }

    /// Moves the cursor among the rows by `by`.
    pub(super) fn move_row(&mut self, by: isize) {
        let last = self.rows().len().saturating_sub(1);
        let row = self.row_cursor().saturating_add_signed(by).min(last);
        self.set_row_cursor(row);
    }

    /// Moves the cursor to the first row of the next (or previous) section.
    pub(super) fn move_section(&mut self, by: isize) {
        let sections = self.sections();
        let mut starts = Vec::new();
        let mut row = 0;
        for section in &sections {
            if !section.rows.is_empty() {
                starts.push(row);
            }
            row += section.rows.len();
        }
        let Some(current) = starts.iter().rposition(|start| *start <= self.row_cursor()) else {
            return;
        };
        let count = starts.len() as isize;
        let next = (current as isize + by).rem_euclid(count) as usize;
        self.set_row_cursor(starts[next]);
    }

    /// The row under the cursor.
    pub fn current_row(&self) -> Option<Row> {
        self.rows().into_iter().nth(self.row_cursor())
    }

    /// Changes the row under the cursor.
    pub(super) fn change_row(&mut self) -> Option<Request> {
        let row = self.current_row()?;
        let view = self.settings.clone()?;
        let settings = &view.settings;
        let about = format!("the {}", row.label.to_lowercase());
        let about = match row.key {
            Some(key) if key.starts_with("limits.alternative.") => {
                format!("the alternative {}", row.label.to_lowercase())
            }
            Some(key) if key.starts_with("limits.") => {
                format!("the {} limit", row.label.to_lowercase())
            }
            Some(key) if key.starts_with("sharing.") => {
                format!("the sharing {}", row.label.to_lowercase())
            }
            _ => about,
        };
        let value_edit = |target: ValueTarget, text: String, placeholder: &str| {
            let title = match &target {
                ValueTarget::Setting { about, .. } => capitalized(about),
                ValueTarget::CategoryFolder(name) => format!("Folder of {name}"),
                _ => row.label.clone(),
            };
            Modal::Value(Box::new(ValueEdit::new(title, target, &text, placeholder)))
        };
        match row.edit {
            Edit::None => return None,
            Edit::Value(kind) => {
                let key = row.key?;
                let text = match key {
                    "limits.download" => limit_input(settings.limits.download),
                    "limits.upload" => limit_input(settings.limits.upload),
                    "limits.alternative.download" => {
                        limit_input(settings.limits.alternative.download)
                    }
                    "limits.alternative.upload" => limit_input(settings.limits.alternative.upload),
                    "save_folder" => settings
                        .save_folder
                        .as_ref()
                        .map(|f| f.display().to_string())
                        .unwrap_or_default(),
                    _ if row.value == "none" || row.value == "no limit" => String::new(),
                    _ => row.value.clone(),
                };
                let target = ValueTarget::Setting { key, kind, about };
                self.modal = Some(value_edit(target, text, "none"));
            }
            Edit::Toggle(on) => {
                let key = row.key?;
                return Some(self.change_settings([(key, json!(!on))], Some(about)));
            }
            Edit::Cycle(values) => {
                let key = row.key?;
                let current = serde_json::to_value(settings.sharing.action).ok();
                let index = values
                    .iter()
                    .position(|v| current.as_ref().and_then(Value::as_str) == Some(v))
                    .unwrap_or(0);
                let next = values[(index + 1) % values.len()];
                return Some(self.change_settings([(key, json!(next))], Some(about)));
            }
            Edit::Schedule => {
                let schedule = settings.limits.alternative.schedule.as_ref();
                self.modal = Some(Modal::Form(Box::new((
                    FormKind::Schedule,
                    schedule_form(schedule),
                ))));
            }
            Edit::Theme => return self.next_theme(),
            Edit::Columns => self.open_columns(),
            Edit::Category(name) => {
                let folder = self
                    .categories
                    .as_ref()
                    .and_then(|c| c.get(&name))
                    .map(|f| f.display().to_string())
                    .unwrap_or_default();
                self.modal = Some(value_edit(
                    ValueTarget::CategoryFolder(name),
                    folder,
                    "/path/to/folder",
                ));
            }
            Edit::NewCategory => {
                let form = Form::new(vec![
                    Field::text("Name", "", "films, music…"),
                    Field::text("Folder", "", "/path/to/folder"),
                ]);
                self.modal = Some(Modal::Form(Box::new((FormKind::NewCategory, form))));
            }
            Edit::Tracker(url) => {
                self.modal = Some(value_edit(
                    ValueTarget::Tracker(Some(url.clone())),
                    url,
                    "https://…",
                ));
            }
            Edit::NewTracker => {
                self.modal = Some(value_edit(
                    ValueTarget::Tracker(None),
                    String::new(),
                    "https://…",
                ));
            }
            Edit::Relay(relay) => {
                self.modal = Some(value_edit(
                    ValueTarget::Relay(Some(relay.clone())),
                    relay,
                    "<key>@host:port",
                ));
            }
            Edit::NewRelay => {
                self.modal = Some(value_edit(
                    ValueTarget::Relay(None),
                    String::new(),
                    "<key>@host:port",
                ));
            }
            Edit::Tag(tag) => {
                self.area = Area::Transfers;
                self.view = View::List;
                self.change_finding(|finding| {
                    finding.text = format!("#{tag}");
                    finding.status = None;
                });
            }
        }
        None
    }

    /// `a`: adds to the list under the cursor: trackers, relays or, anywhere else, categories.
    pub(super) fn add_entry(&mut self) -> Option<Request> {
        self.settings.as_ref()?;
        let edit = self.current_row().map(|row| row.edit);
        let target = match edit {
            Some(Edit::Tracker(_) | Edit::NewTracker) => ValueTarget::Tracker(None),
            Some(Edit::Relay(_) | Edit::NewRelay) => ValueTarget::Relay(None),
            _ => {
                let form = Form::new(vec![
                    Field::text("Name", "", "films, music…"),
                    Field::text("Folder", "", "/path/to/folder"),
                ]);
                self.modal = Some(Modal::Form(Box::new((FormKind::NewCategory, form))));
                return None;
            }
        };
        let (title, placeholder) = match target {
            ValueTarget::Relay(_) => ("+ Add a relay", "<key>@host:port"),
            _ => ("+ Add a tracker", "https://…"),
        };
        self.modal = Some(Modal::Value(Box::new(ValueEdit::new(
            title.to_owned(),
            target,
            "",
            placeholder,
        ))));
        None
    }

    /// `x`: removes the row's entry, or gives its setting the config file's value again.
    pub(super) fn remove_row(&mut self) -> Option<Request> {
        let row = self.current_row()?;
        let view = self.settings.clone()?;
        match row.edit {
            Edit::Category(name) => {
                self.modal = Some(Modal::Confirm {
                    title: "Remove category".to_owned(),
                    text: format!("Remove the category {name}?"),
                    detail: "Its transfers stay, with no category; no files move.".to_owned(),
                    request: Box::new(Request::RemoveCategory(name)),
                });
                None
            }
            Edit::Tracker(url) => {
                let trackers: Vec<&String> = view
                    .settings
                    .trackers
                    .iter()
                    .filter(|t| **t != url)
                    .collect();
                let about = Some(format!("the trackers, without {url}"));
                Some(self.change_settings([("trackers", json!(trackers))], about))
            }
            Edit::Relay(relay) => {
                let relays: Vec<String> = relay_identities(&view)
                    .into_iter()
                    .filter(|r| *r != relay)
                    .collect();
                let about = Some("the relays".to_owned());
                Some(self.change_settings([("relays.known", json!(relays))], about))
            }
            _ => match row.key {
                Some(key) if view.changed.contains_key(key) => Some(Request::ResetSetting {
                    key: key.to_owned(),
                    about: row.label.to_lowercase(),
                }),
                Some(_) => {
                    let text = format!("{} already has the config file's value.", row.label);
                    self.notify(text, false);
                    None
                }
                None => {
                    self.notify("There is nothing to remove here.", false);
                    None
                }
            },
        }
    }

    /// The request a value prompt makes once `text` is typed, or why it can't.
    pub(super) fn value_request(
        &mut self,
        target: &ValueTarget,
        text: &str,
    ) -> Result<Request, String> {
        let text = text.trim();
        let view = self.settings.clone();
        let settings = view.as_ref().map(|v| &v.settings);
        match target {
            ValueTarget::Setting { key, kind, about } => {
                let value = kind.parse(text)?;
                Ok(self.change_settings([(*key, value)], Some(about.clone())))
            }
            ValueTarget::Tracker(old) => {
                if let Some(problem) = tempest_api::settings::tracker_problem(text) {
                    return Err(problem.to_owned());
                }
                let mut trackers = settings.map(|s| s.trackers.clone()).unwrap_or_default();
                match old
                    .as_ref()
                    .and_then(|old| trackers.iter().position(|t| t == old))
                {
                    Some(index) => trackers[index] = text.to_owned(),
                    None if trackers.iter().any(|t| t == text) => {
                        return Err("That tracker is already listed.".to_owned());
                    }
                    None => trackers.push(text.to_owned()),
                }
                Ok(self.change_settings(
                    [("trackers", json!(trackers))],
                    Some("the trackers".to_owned()),
                ))
            }
            ValueTarget::Relay(old) => {
                if let Some(problem) = tempest_api::settings::relay_problem(text) {
                    return Err(problem.to_owned());
                }
                let mut relays = view.as_ref().map(relay_identities).unwrap_or_default();
                match old
                    .as_ref()
                    .and_then(|old| relays.iter().position(|r| r == old))
                {
                    Some(index) => relays[index] = text.to_owned(),
                    None if relays.iter().any(|r| r == text) => {
                        return Err("That relay is already listed.".to_owned());
                    }
                    None => relays.push(text.to_owned()),
                }
                Ok(self.change_settings(
                    [("relays.known", json!(relays))],
                    Some("the relays".to_owned()),
                ))
            }
            ValueTarget::CategoryFolder(name) => {
                if !Path::new(text).is_absolute() {
                    return Err("Type an absolute path.".to_owned());
                }
                Ok(Request::SetCategory {
                    name: name.clone(),
                    folder: PathBuf::from(text),
                    new: false,
                })
            }
            ValueTarget::ViewName => self.save_view(text),
        }
    }

    /// The request a form modal makes, or why it can't.
    pub(super) fn form_request(&mut self, kind: &FormKind, form: &Form) -> Result<Request, String> {
        match kind {
            FormKind::Schedule => {
                let value = schedule_value(form)?;
                let about = Some("the schedule".to_owned());
                Ok(self.change_settings([("limits.alternative.schedule", value)], about))
            }
            FormKind::NewCategory => {
                let name = form.text("Name");
                let folder = form.text("Folder");
                if name.is_empty() {
                    return Err("Name the category.".to_owned());
                }
                if self
                    .categories
                    .as_ref()
                    .is_some_and(|c| c.contains_key(&name))
                {
                    return Err(format!("There is already a category named {name}."));
                }
                if !Path::new(&folder).is_absolute() {
                    return Err("Its folder is an absolute path.".to_owned());
                }
                Ok(Request::SetCategory {
                    name,
                    folder: PathBuf::from(folder),
                    new: true,
                })
            }
            FormKind::Limits(target) => self.limits_request(target, form),
        }
    }

    /// Changes settings, noting a change to what the user keeps (D-195) so a fetch already
    /// under way doesn't undo it.
    pub(super) fn change_settings<const N: usize>(
        &mut self,
        changes: [(&str, Value); N],
        about: Option<String>,
    ) -> Request {
        let changes: BTreeMap<String, Value> = changes
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect();
        if changes.keys().any(|key| key.starts_with("ui.")) {
            self.ui_pending += 1;
        }
        Request::ChangeSettings { changes, about }
    }

    /// Opens the column picker on the columns shown.
    pub(super) fn open_columns(&mut self) {
        self.modal = Some(Modal::Columns {
            chosen: self.columns.clone(),
            cursor: 0,
        });
    }

    /// Shows the next theme and keeps it.
    pub(super) fn next_theme(&mut self) -> Option<Request> {
        self.theme = self.theme.next();
        let name = self.theme.name;
        Some(self.change_settings([(prefs::THEME, json!(name))], None))
    }

    /// Shows `columns` and keeps them.
    pub(super) fn set_columns(&mut self, columns: Vec<Column>) -> Request {
        let value = prefs::columns_value(&columns);
        self.columns = Column::OPTIONAL
            .into_iter()
            .filter(|column| columns.contains(column))
            .collect();
        self.change_settings([(prefs::COLUMNS, value)], Some("the columns".to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
