//! What the user keeps in the engine's `ui` settings (D-195): the theme, the columns and the
//! saved views.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::App;
use super::modal::Modal;
use super::request::Request;
use crate::columns::Column;
use crate::prefs;

impl App {
    /// The view of the list saved as `name`.
    pub(super) fn save_view(&mut self, name: &str) -> Result<Request, String> {
        if name.trim().is_empty() {
            return Err("Name the view.".to_owned());
        }
        let view = prefs::saved(name, &self.finding);
        self.views = prefs::with_view(&self.views, view);
        let value = prefs::views_value(&self.views);
        let about = Some(format!("the view {}", name.trim()));
        Ok(self.change_settings([(prefs::VIEWS, value)], about))
    }

    /// The name of the saved view the list shows, if it shows one.
    pub fn current_view(&self) -> Option<&str> {
        self.views
            .iter()
            .find(|view| prefs::finding(view) == self.finding)
            .map(|view| view.name.as_str())
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
