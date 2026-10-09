//! Choosing from a list, such as a category or a saved view, and saving and deleting views.

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use super::{Modal, Outcome, confirm_unpublish};
use crate::app::App;
use crate::app::request::{Action, Request, Target};
use crate::finding::{Finding, SortKey, StatusFilter};
use crate::prefs;

/// What a [`Modal::Pick`] chooses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickKind {
    /// A transfer's category.
    Category(Target),
    /// The status to show only.
    Status,
    /// What to sort by.
    Sort,
    /// A saved view.
    View,
    /// A tracker to unpublish a transfer from.
    Unpublish(Target),
}

impl PickKind {
    /// The modal's title.
    pub fn title(&self) -> &'static str {
        match self {
            Self::Category(_) => "Category",
            Self::Status => "Show",
            Self::Sort => "Sort by",
            Self::View => "Views",
            Self::Unpublish(_) => "Unpublish from",
        }
    }
}

/// An item of a [`Modal::Pick`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickItem {
    /// What it is.
    pub label: String,
    /// A note beside it: a count, or that it is the current choice.
    pub note: String,
}

impl App {
    /// The view of the list saved as `name`.
    pub(in crate::app) fn save_view(&mut self, name: &str) -> Result<Request, String> {
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

    pub(super) fn pick_key(&mut self, kind: PickKind, selected: usize, key: KeyEvent) -> Outcome {
        let count = self.pick_items(&kind).len();
        let last = count.saturating_sub(1);
        let selected = match key.code {
            KeyCode::Char('j') | KeyCode::Down | KeyCode::Tab => (selected + 1).min(last),
            KeyCode::Char('k') | KeyCode::Up | KeyCode::BackTab => selected.saturating_sub(1),
            KeyCode::Char('g') | KeyCode::Home => 0,
            KeyCode::Char('G') | KeyCode::End => last,
            KeyCode::Enter => return self.pick(kind, selected),
            KeyCode::Char('x') | KeyCode::Delete if kind == PickKind::View && selected > 0 => {
                return self.delete_view(selected - 1);
            }
            _ => selected,
        };
        Outcome::Open(Modal::Pick { kind, selected })
    }

    /// Applies the item at `index` of a pick.
    pub(super) fn pick(&mut self, kind: PickKind, index: usize) -> Outcome {
        match kind {
            PickKind::Category(target) => {
                let category = match index {
                    0 => None,
                    n => match self.categories.as_ref().and_then(|c| c.keys().nth(n - 1)) {
                        Some(category) => Some(category.clone()),
                        None => return Outcome::Close,
                    },
                };
                Outcome::Ask(Request::Transfer(target, Action::Category(category)))
            }
            PickKind::Unpublish(target) => {
                let published = self.model.transfer(&target.id).map(|t| &t.published);
                match published.and_then(|p| p.iter().nth(index)) {
                    Some(tracker) => Outcome::Open(confirm_unpublish(target, tracker.clone())),
                    None => Outcome::Close,
                }
            }
            PickKind::Status => {
                let status = index
                    .checked_sub(1)
                    .and_then(|n| StatusFilter::ALL.get(n).copied());
                self.change_finding(|finding| finding.status = status);
                Outcome::Close
            }
            PickKind::View => {
                let finding = match index {
                    0 => Finding::default(),
                    n => match self.views.get(n - 1) {
                        Some(view) => prefs::finding(view),
                        None => return Outcome::Close,
                    },
                };
                self.change_finding(|current| *current = finding);
                Outcome::Close
            }
            PickKind::Sort => {
                let key = SortKey::ALL.get(index).copied().unwrap_or(SortKey::Queue);
                self.change_finding(|finding| {
                    if finding.sort.key != key {
                        finding.sort.key = key;
                        finding.sort.reversed = false;
                    }
                });
                Outcome::Close
            }
        }
    }

    pub(super) fn delete_view(&mut self, index: usize) -> Outcome {
        if index >= self.views.len() {
            return Outcome::Close;
        }
        let view = self.views.remove(index);
        let value = prefs::views_value(&self.views);
        let about = Some(format!("the views, without {}", view.name));
        Outcome::Ask(self.change_settings([(prefs::VIEWS, value)], about))
    }

    /// The items a pick offers.
    pub fn pick_items(&self, kind: &PickKind) -> Vec<PickItem> {
        let item = |label: &str, current: bool| PickItem {
            label: label.to_owned(),
            note: if current { "current" } else { "" }.to_owned(),
        };
        match kind {
            PickKind::Category(target) => {
                let current = self
                    .model
                    .transfer(&target.id)
                    .and_then(|t| t.category.as_deref());
                let mut items = vec![item("None", current.is_none())];
                items.extend(
                    self.categories
                        .iter()
                        .flatten()
                        .map(|(c, _)| item(c, current == Some(c.as_str()))),
                );
                items
            }
            PickKind::Unpublish(target) => (self.model.transfer(&target.id))
                .into_iter()
                .flat_map(|t| &t.published)
                .map(|tracker| item(tracker, false))
                .collect(),
            PickKind::View => {
                let current = self.current_view();
                let describe = |finding: &Finding| match finding.describe() {
                    text if text.is_empty() => "everything".to_owned(),
                    text => text,
                };
                let mut items = vec![PickItem {
                    label: "All transfers".to_owned(),
                    note: if self.finding == Finding::default() {
                        "current".to_owned()
                    } else {
                        "queue order".to_owned()
                    },
                }];
                items.extend(self.views.iter().map(|view| PickItem {
                    label: view.name.clone(),
                    note: if current == Some(view.name.as_str()) {
                        "current".to_owned()
                    } else {
                        describe(&prefs::finding(view))
                    },
                }));
                items
            }
            PickKind::Status => {
                let transfers = self.model.transfers();
                let count = |status: Option<StatusFilter>| {
                    transfers
                        .iter()
                        .filter(|t| status.is_none_or(|s| StatusFilter::of(&t.status) == s))
                        .count()
                        .to_string()
                };
                let mut items = vec![PickItem {
                    label: "All".to_owned(),
                    note: count(None),
                }];
                items.extend(StatusFilter::ALL.into_iter().map(|status| PickItem {
                    label: status.title().to_owned(),
                    note: count(Some(status)),
                }));
                items
            }
            PickKind::Sort => SortKey::ALL
                .into_iter()
                .map(|key| {
                    let current = self.finding.sort.key == key;
                    let mut item = item(key.title(), current);
                    if current {
                        item.note = self.finding.sort.describe();
                    }
                    item
                })
                .collect(),
        }
    }
}
