//! Modals: what opens over the list and takes the keys until it closes. `esc` closes any of
//! them without doing anything, except the filter prompt, where it also clears the filter.

use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tempest_engine::api::AddRequest;

use super::request::{Action, Request, Target, parse_tags};
use super::{App, Hint};
use crate::commands::{self, Command};
use crate::finding::{SortKey, StatusFilter};
use crate::input::Input;

/// What is open over the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Modal {
    /// Every key of the current area.
    Help,
    /// The `:` command palette.
    Palette {
        /// What is typed.
        query: Input,
        /// The highlighted match.
        selected: usize,
    },
    /// The `/` prompt, which filters the list as it is typed.
    Filter(Input),
    /// Adding a download from a link.
    Add(Box<AddForm>),
    /// Confirming a removal.
    Remove {
        /// What goes.
        target: Target,
        /// Whether its files go too.
        delete_files: bool,
    },
    /// A line of text about a transfer.
    Text {
        /// What the text is.
        kind: TextKind,
        /// The transfer.
        target: Target,
        /// The text.
        input: Input,
        /// Why the text was refused, if it was.
        error: Option<String>,
    },
    /// A choice from a list.
    Pick {
        /// What is chosen.
        kind: PickKind,
        /// The highlighted item.
        selected: usize,
    },
}

/// What a [`Modal::Text`] asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextKind {
    /// A new name, or nothing for the bundle's own.
    Rename,
    /// Comma-separated tags.
    Tags,
    /// The absolute path of a folder to move the files into.
    Move,
}

impl TextKind {
    /// The modal's title.
    pub fn title(self) -> &'static str {
        match self {
            Self::Rename => "Rename",
            Self::Tags => "Tag",
            Self::Move => "Move",
        }
    }

    /// A line explaining what to type.
    pub fn help(self) -> &'static str {
        match self {
            Self::Rename => "Leave it empty to use the bundle's own name.",
            Self::Tags => "Separate tags with commas.",
            Self::Move => "The absolute path of the folder to move them into.",
        }
    }
}

/// What a [`Modal::Pick`] chooses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickKind {
    /// A transfer's category.
    Category(Target),
    /// The status to show only.
    Status,
    /// What to sort by.
    Sort,
}

impl PickKind {
    /// The modal's title.
    pub fn title(&self) -> &'static str {
        match self {
            Self::Category(_) => "Category",
            Self::Status => "Show",
            Self::Sort => "Sort by",
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

/// The add modal's fields.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AddField {
    /// The `tempest:` link.
    #[default]
    Link,
    /// Where to save, if not the category's or the engine's folder.
    SaveFolder,
    /// The category.
    Category,
    /// Comma-separated tags.
    Tags,
    /// Whether to add it stopped.
    Stopped,
}

impl AddField {
    /// Every field, top to bottom.
    pub const ALL: [Self; 5] = [
        Self::Link,
        Self::SaveFolder,
        Self::Category,
        Self::Tags,
        Self::Stopped,
    ];

    /// Its label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Link => "Link",
            Self::SaveFolder => "Save to",
            Self::Category => "Category",
            Self::Tags => "Tags",
            Self::Stopped => "Start",
        }
    }

    fn step(self, by: isize) -> Self {
        let index = Self::ALL.iter().position(|f| *f == self).unwrap_or(0) as isize;
        Self::ALL[(index + by).rem_euclid(Self::ALL.len() as isize) as usize]
    }
}

/// What the add modal holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AddForm {
    /// The link.
    pub link: Input,
    /// The save folder, or empty for the default.
    pub save_folder: Input,
    /// The category, if any.
    pub category: Option<String>,
    /// Comma-separated tags.
    pub tags: Input,
    /// Whether to add it stopped.
    pub stopped: bool,
    /// The field taking keys.
    pub focus: AddField,
    /// Why the form was refused, if it was.
    pub error: Option<String>,
}

impl AddForm {
    /// The request the form makes, or why it can't.
    fn request(&self) -> Result<AddRequest, String> {
        let link = self.link.text().trim().to_owned();
        if link.is_empty() {
            return Err("Paste a tempest: link.".to_owned());
        }
        let folder = self.save_folder.text().trim().to_owned();
        let save_folder = match folder.is_empty() {
            true => None,
            false if Path::new(&folder).is_absolute() => Some(PathBuf::from(folder)),
            false => return Err("The save folder must be an absolute path.".to_owned()),
        };
        Ok(AddRequest {
            link: Some(link),
            save_folder,
            category: self.category.clone(),
            tags: parse_tags(&self.tags.text()),
            stopped: self.stopped,
            ..AddRequest::default()
        })
    }
}

/// What a key does to a modal.
enum Outcome {
    /// It stays open.
    Open(Modal),
    /// It closes.
    Close,
    /// It closes, running this.
    Run(Command),
    /// It closes, asking this.
    Ask(Request),
}

fn is(key: KeyEvent, code: KeyCode) -> bool {
    key.code == code && !key.modifiers.contains(KeyModifiers::CONTROL)
}

fn ctrl(key: KeyEvent, c: char) -> bool {
    key.code == KeyCode::Char(c) && key.modifiers.contains(KeyModifiers::CONTROL)
}

impl App {
    /// Handles `key` while `modal` is open.
    pub(super) fn modal_key(&mut self, modal: Modal, key: KeyEvent) -> Option<Request> {
        let outcome = if key.code == KeyCode::Esc {
            if let Modal::Filter(_) = modal {
                self.set_filter_text(String::new());
            }
            Outcome::Close
        } else {
            match modal {
                Modal::Help => Outcome::Close,
                Modal::Palette { query, selected } => self.palette_key(query, selected, key),
                Modal::Filter(input) => self.filter_key(input, key),
                Modal::Add(form) => self.add_key(*form, key),
                Modal::Remove {
                    target,
                    delete_files,
                } => remove_key(target, delete_files, key),
                Modal::Text {
                    kind,
                    target,
                    input,
                    error,
                } => text_key(kind, target, input, error, key),
                Modal::Pick { kind, selected } => self.pick_key(kind, selected, key),
            }
        };
        match outcome {
            Outcome::Open(modal) => {
                self.modal = Some(modal);
                None
            }
            Outcome::Close => None,
            Outcome::Run(command) => self.run(command),
            Outcome::Ask(request) => Some(request),
        }
    }

    /// Types pasted text into the open modal's input, if it has one.
    pub fn paste(&mut self, text: &str) {
        match &mut self.modal {
            Some(Modal::Palette { query, selected }) => {
                query.insert(text);
                *selected = 0;
            }
            Some(Modal::Filter(input)) => {
                input.insert(text);
                let text = input.text();
                self.set_filter_text(text);
            }
            Some(Modal::Add(form)) => match form.focus {
                AddField::Link => form.link.insert(text),
                AddField::SaveFolder => form.save_folder.insert(text),
                AddField::Tags => form.tags.insert(text),
                AddField::Category | AddField::Stopped => {}
            },
            Some(Modal::Text { input, .. }) => input.insert(text),
            _ => {}
        }
    }

    fn palette_key(&mut self, mut query: Input, selected: usize, key: KeyEvent) -> Outcome {
        let matches = commands::palette_matches(self.screen(), &query.text());
        let last = matches.len().saturating_sub(1);
        let selected = if is(key, KeyCode::Down) || is(key, KeyCode::Tab) || ctrl(key, 'n') {
            (selected + 1).min(last)
        } else if is(key, KeyCode::Up) || key.code == KeyCode::BackTab || ctrl(key, 'p') {
            selected.saturating_sub(1)
        } else if is(key, KeyCode::Enter) {
            return matches
                .get(selected)
                .map_or(Outcome::Close, |command| Outcome::Run(*command));
        } else if query.key(key) {
            0
        } else {
            selected
        };
        Outcome::Open(Modal::Palette { query, selected })
    }

    fn filter_key(&mut self, mut input: Input, key: KeyEvent) -> Outcome {
        if is(key, KeyCode::Enter) {
            return Outcome::Close;
        }
        if input.key(key) {
            self.set_filter_text(input.text());
        }
        Outcome::Open(Modal::Filter(input))
    }

    fn add_key(&mut self, mut form: AddForm, key: KeyEvent) -> Outcome {
        let focus = form.focus;
        if is(key, KeyCode::Enter) {
            return match form.request() {
                Ok(request) => Outcome::Ask(Request::Add(Box::new(request))),
                Err(error) => {
                    form.error = Some(error);
                    Outcome::Open(Modal::Add(Box::new(form)))
                }
            };
        }
        if is(key, KeyCode::Tab) || is(key, KeyCode::Down) {
            form.focus = focus.step(1);
        } else if key.code == KeyCode::BackTab || is(key, KeyCode::Up) {
            form.focus = focus.step(-1);
        } else {
            match focus {
                AddField::Link => {
                    form.link.key(key);
                }
                AddField::SaveFolder => {
                    form.save_folder.key(key);
                }
                AddField::Tags => {
                    form.tags.key(key);
                }
                AddField::Category => {
                    let by = if is(key, KeyCode::Left) || is(key, KeyCode::Char('h')) {
                        -1
                    } else if is(key, KeyCode::Right)
                        || is(key, KeyCode::Char('l'))
                        || is(key, KeyCode::Char(' '))
                    {
                        1
                    } else {
                        0
                    };
                    form.category = self.step_category(form.category.as_deref(), by);
                }
                AddField::Stopped => {
                    if is(key, KeyCode::Char(' '))
                        || is(key, KeyCode::Left)
                        || is(key, KeyCode::Right)
                    {
                        form.stopped = !form.stopped;
                    }
                }
            }
        }
        Outcome::Open(Modal::Add(Box::new(form)))
    }

    /// The category `by` places from `current` among none and every category, wrapping.
    fn step_category(&self, current: Option<&str>, by: isize) -> Option<String> {
        let mut options: Vec<Option<&str>> = vec![None];
        options.extend(self.categories.iter().flatten().map(|c| Some(c.as_str())));
        let index = options.iter().position(|o| *o == current).unwrap_or(0) as isize;
        let index = (index + by).rem_euclid(options.len() as isize) as usize;
        options[index].map(str::to_owned)
    }

    fn pick_key(&mut self, kind: PickKind, selected: usize, key: KeyEvent) -> Outcome {
        let count = self.pick_items(&kind).len();
        let last = count.saturating_sub(1);
        let selected = match key.code {
            KeyCode::Char('j') | KeyCode::Down | KeyCode::Tab => (selected + 1).min(last),
            KeyCode::Char('k') | KeyCode::Up | KeyCode::BackTab => selected.saturating_sub(1),
            KeyCode::Char('g') | KeyCode::Home => 0,
            KeyCode::Char('G') | KeyCode::End => last,
            KeyCode::Enter => return self.pick(kind, selected),
            _ => selected,
        };
        Outcome::Open(Modal::Pick { kind, selected })
    }

    /// Applies the item at `index` of a pick.
    fn pick(&mut self, kind: PickKind, index: usize) -> Outcome {
        match kind {
            PickKind::Category(target) => {
                let category = match index {
                    0 => None,
                    n => match self.categories.as_ref().and_then(|c| c.get(n - 1)) {
                        Some(category) => Some(category.clone()),
                        None => return Outcome::Close,
                    },
                };
                Outcome::Ask(Request::Transfer(target, Action::Category(category)))
            }
            PickKind::Status => {
                let status = index
                    .checked_sub(1)
                    .and_then(|n| StatusFilter::ALL.get(n).copied());
                self.change_finding(|finding| finding.status = status);
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
                        .map(|c| item(c, current == Some(c.as_str()))),
                );
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

    /// The modal's keys for the hint bar, the last always shown.
    pub(super) fn modal_hints(modal: &Modal) -> Vec<Hint> {
        let hints: &[(&'static str, &'static str)] = match modal {
            Modal::Help => &[("esc", "close")],
            Modal::Palette { .. } => &[("↑/↓", "choose"), ("enter", "run"), ("esc", "close")],
            Modal::Filter(_) => &[("enter", "keep"), ("esc", "clear")],
            Modal::Add(form) => match form.focus {
                AddField::Category | AddField::Stopped => &[
                    ("←/→", "change"),
                    ("tab", "next"),
                    ("enter", "add"),
                    ("esc", "cancel"),
                ],
                _ => &[("tab", "next"), ("enter", "add"), ("esc", "cancel")],
            },
            Modal::Remove { .. } => &[
                ("tab", "keep/delete files"),
                ("enter", "remove"),
                ("esc", "cancel"),
            ],
            Modal::Text { .. } => &[("enter", "save"), ("esc", "cancel")],
            Modal::Pick { .. } => &[("j/k", "choose"), ("enter", "pick"), ("esc", "cancel")],
        };
        hints
            .iter()
            .map(|(keys, action)| Hint { keys, action })
            .collect()
    }
}

fn remove_key(target: Target, delete_files: bool, key: KeyEvent) -> Outcome {
    let delete_files = match key.code {
        KeyCode::Enter => {
            return Outcome::Ask(Request::Transfer(target, Action::Remove { delete_files }));
        }
        KeyCode::Tab
        | KeyCode::BackTab
        | KeyCode::Left
        | KeyCode::Right
        | KeyCode::Char('h')
        | KeyCode::Char('l') => !delete_files,
        KeyCode::Char('k') => false,
        KeyCode::Char('d') => true,
        _ => delete_files,
    };
    Outcome::Open(Modal::Remove {
        target,
        delete_files,
    })
}

fn text_key(
    kind: TextKind,
    target: Target,
    mut input: Input,
    error: Option<String>,
    key: KeyEvent,
) -> Outcome {
    if !is(key, KeyCode::Enter) {
        input.key(key);
        return Outcome::Open(Modal::Text {
            kind,
            target,
            input,
            error,
        });
    }
    let text = input.text().trim().to_owned();
    let action = match kind {
        TextKind::Rename => Ok(Action::Rename((!text.is_empty()).then_some(text))),
        TextKind::Tags => Ok(Action::Tags(parse_tags(&text))),
        TextKind::Move if Path::new(&text).is_absolute() => Ok(Action::Move(PathBuf::from(text))),
        TextKind::Move => Err("Enter an absolute path.".to_owned()),
    };
    match action {
        Ok(action) => Outcome::Ask(Request::Transfer(target, action)),
        Err(error) => Outcome::Open(Modal::Text {
            kind,
            target,
            input,
            error: Some(error),
        }),
    }
}
