//! What the Settings area's keys do: moving between rows and sections, and what `enter`, `a`
//! and `x` ask of the engine for the row under the cursor (D-196).

use std::path::{Path, PathBuf};

use serde_json::json;

use super::modal::{FormKind, Modal, ValueEdit, ValueTarget};
use super::request::Request;
use super::rows::{
    Edit, Row, Section, capitalized, relay_identities, schedule_form, schedule_value,
    settings_sections,
};
use super::{App, Area, View};
use crate::form::{Field, Form};
use crate::format::limit_input;

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

    pub(super) fn set_row_cursor(&mut self, row: usize) {
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
                    "save_folder" => settings.save_folder.as_ref().cloned().unwrap_or_default(),
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
                let current = settings.sharing.action.as_str();
                let index = values.iter().position(|v| *v == current).unwrap_or(0);
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
}
