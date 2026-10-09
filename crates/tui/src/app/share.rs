//! The Share area (D-139): a form for a file or folder to share, the bundle being built, and
//! the shares with their links.

use std::cell::Cell;
use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use tempest_api::requests::AddRequest;
use tempest_api::{Kind, TransferInfo};
use tempest_protocol::BundleId;

use super::request::{Request, parse_tags};
use super::{App, Area, Hint, View};
use crate::form::{self, Field, Form, Value};

/// The Share area's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareArea {
    /// What to share, and how.
    pub form: Form,
    /// Whether the form has the keys.
    pub editing: bool,
    /// The path whose bundle the engine is building, while it is.
    pub building: Option<PathBuf>,
    /// The share the cursor is on.
    pub selected: Option<BundleId>,
    /// The first row the list of shares showed when last drawn, kept as the Transfers list's
    /// is.
    pub offset: Cell<usize>,
}

impl Default for ShareArea {
    fn default() -> Self {
        Self {
            form: share_form(&[]),
            editing: false,
            building: None,
            selected: None,
            offset: Cell::new(0),
        }
    }
}

/// The form's labels.
const PATH: &str = "Path";
const NAME: &str = "Name";
const COMMENT: &str = "Comment";
const CATEGORY: &str = "Category";
const TAGS: &str = "Tags";

/// An empty share form offering `categories`.
fn share_form(categories: &[String]) -> Form {
    Form::new(vec![
        Field::text(PATH, "", "/absolute/path/to/a/file/or/folder"),
        Field::text(NAME, "", "the file or folder's name"),
        Field::text(COMMENT, "", "optional description"),
        Field::choice(CATEGORY, category_options(categories), 0),
        Field::text(TAGS, "", "comma-separated"),
    ])
}

fn category_options(categories: &[String]) -> Vec<String> {
    let mut options = vec!["none".to_owned()];
    options.extend(categories.iter().cloned());
    options
}

impl App {
    /// Every share, in queue order.
    pub fn shares(&self) -> Vec<&TransferInfo> {
        self.model
            .transfers()
            .iter()
            .filter(|t| t.kind == Kind::Share)
            .collect()
    }

    /// The share the cursor is on, or the first.
    pub fn selected_share(&self) -> Option<&TransferInfo> {
        let shares = self.shares();
        let selected = self.share.selected;
        shares
            .iter()
            .find(|t| Some(t.id) == selected)
            .or(shares.first())
            .copied()
    }

    /// Moves the cursor among the shares by `by`.
    pub(super) fn move_share(&mut self, by: isize) {
        let shares = self.shares();
        let Some(last) = shares.len().checked_sub(1) else {
            return;
        };
        let current = self
            .selected_share()
            .and_then(|s| shares.iter().position(|t| t.id == s.id))
            .unwrap_or(0);
        let next = current.saturating_add_signed(by).min(last);
        self.share.selected = Some(shares[next].id);
    }

    /// Opens the selected share in the Transfers area's detail.
    pub(super) fn show_share(&mut self) {
        let Some(id) = self.selected_share().map(|t| t.id) else {
            return;
        };
        self.area = Area::Transfers;
        self.selected = Some(id);
        if self.cursor().is_none() {
            // The filters hide it: show everything.
            self.change_finding(|finding| {
                finding.text.clear();
                finding.status = None;
            });
            self.selected = Some(id);
        }
        self.view = View::Detail;
        self.scroll = 0;
    }

    /// Offers `categories` in the share form, keeping the chosen one if it is still there.
    pub(super) fn offer_categories(&mut self, categories: &[String]) {
        let options = category_options(categories);
        for field in &mut self.share.form.fields {
            if let Value::Choice {
                options: old,
                chosen,
            } = &mut field.value
            {
                let current = old.get(*chosen).cloned();
                *chosen = current
                    .and_then(|c| options.iter().position(|o| *o == c))
                    .unwrap_or(0);
                *old = options.clone();
            }
        }
    }

    /// Handles `key` while the share form has the keys.
    pub(super) fn share_key(&mut self, key: KeyEvent) -> Option<Request> {
        if key.code == KeyCode::Esc {
            self.share.editing = false;
            return None;
        }
        match self.share.form.key(key) {
            form::Outcome::Edited => None,
            form::Outcome::Submit => match self.share_request() {
                Ok(request) => {
                    self.share.editing = false;
                    self.share.form.error = None;
                    self.share.building = Some(PathBuf::from(self.share.form.text(PATH)));
                    Some(request)
                }
                Err(error) => {
                    self.share.form.error = Some(error);
                    None
                }
            },
        }
    }

    /// The request the share form makes, or why it can't.
    fn share_request(&self) -> Result<Request, String> {
        if self.share.building.is_some() {
            return Err("Wait for the bundle being built.".to_owned());
        }
        let form = &self.share.form;
        let path = form.text(PATH);
        if path.is_empty() {
            return Err("Type the path of a file or folder to share.".to_owned());
        }
        if !Path::new(&path).is_absolute() {
            return Err("The path must be absolute, on the engine's machine.".to_owned());
        }
        let name = form.text(NAME);
        let category = match form.chosen(CATEGORY) {
            0 => None,
            n => self
                .categories
                .as_ref()
                .and_then(|c| c.keys().nth(n - 1).cloned()),
        };
        Ok(Request::Share(Box::new(AddRequest {
            path: Some(PathBuf::from(path)),
            name: (!name.is_empty()).then_some(name),
            comment: form.text(COMMENT),
            category,
            tags: parse_tags(&form.text(TAGS)),
            ..AddRequest::default()
        })))
    }

    /// Takes in how sharing went: on success the form empties and the cursor goes to the new
    /// share.
    pub(super) fn shared(&mut self, result: Result<&TransferInfo, &str>) {
        self.share.building = None;
        match result {
            Ok(transfer) => {
                let categories: Vec<String> = self
                    .categories
                    .as_ref()
                    .map(|c| c.keys().cloned().collect())
                    .unwrap_or_default();
                self.share.form = share_form(&categories);
                self.share.selected = Some(transfer.id);
            }
            Err(error) => {
                self.share.form.error = Some(error.to_owned());
                self.share.editing = true;
            }
        }
    }

    /// The form's keys for the hint bar.
    pub(super) fn share_hints(&self) -> Vec<Hint> {
        let mut hints: Vec<Hint> = self
            .share
            .form
            .hints()
            .iter()
            .map(|(keys, action)| Hint { keys, action })
            .collect();
        hints.push(Hint {
            keys: "enter",
            action: "share",
        });
        hints.push(Hint {
            keys: "esc",
            action: "done",
        });
        hints
    }
}
