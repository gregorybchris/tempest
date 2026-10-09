//! Modals: what opens over the list and takes the keys until it closes. `esc` closes any of
//! them without doing anything, except the filter prompt, where it also clears the filter.

mod add;
mod edit;
mod pick;

pub use add::{AddField, AddForm};
pub use edit::{FormKind, TextKind, ValueEdit, ValueTarget};
pub use pick::{PickItem, PickKind};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use self::edit::text_key;
use super::request::{Action, Request, Target};
use super::{App, Hint};
use crate::columns::Column;
use crate::commands::{self, Command};
use crate::form::Form;
use crate::input::Input;

/// What is open over the screen.
#[derive(Debug, Clone, PartialEq)]
pub enum Modal {
    /// Every key of the current screen.
    Help {
        /// How many rows it is scrolled down, when it is taller than the screen.
        scroll: usize,
    },
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
    /// A value typed for a setting, a tracker, a relay, a category's folder or a view's name.
    Value(Box<ValueEdit>),
    /// A form: a transfer's limits, a new category or the schedule.
    Form(Box<(FormKind, Form)>),
    /// Confirming a request.
    Confirm {
        /// The modal's title.
        title: String,
        /// The question.
        text: String,
        /// What saying yes does, in a sentence.
        detail: String,
        /// What saying yes asks.
        request: Box<Request>,
    },
    /// The first run's account of the network defaults (D-227). Any key closes it.
    Network,
    /// Quitting an engine that runs only for this TUI while transfers are active (D-228).
    Quit {
        /// How many transfers are active.
        active: usize,
    },
    /// Choosing the list's optional columns.
    Columns {
        /// The columns switched on.
        chosen: Vec<Column>,
        /// The highlighted one, in [`Column::OPTIONAL`].
        cursor: usize,
    },
}

impl Modal {
    /// Shows why the engine refused what the modal sent, returning whether it can.
    pub(super) fn refused(&mut self, error: &str) -> bool {
        let shown = match self {
            Self::Add(form) => &mut form.error,
            Self::Text { error, .. } => error,
            Self::Value(edit) => &mut edit.error,
            Self::Form(form) => &mut form.1.error,
            _ => return false,
        };
        *shown = Some(error.to_owned());
        true
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
    /// It stays open, asking this, until the engine accepts it; a refusal shows in it.
    Submit(Modal, Request),
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
        // A key on the modal starts over: an answer to what it sent before shows as a notice.
        self.submitted = None;
        let outcome = if key.code == KeyCode::Esc {
            if let Modal::Filter(_) = modal {
                self.set_filter_text(String::new());
            }
            Outcome::Close
        } else {
            match modal {
                Modal::Help { scroll } => self.help_key(scroll, key),
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
                Modal::Value(edit) => self.value_key(*edit, key),
                Modal::Form(form) => self.form_key(*form, key),
                Modal::Confirm { request, .. } if is(key, KeyCode::Enter) => Outcome::Ask(*request),
                Modal::Confirm { .. } => Outcome::Close,
                Modal::Network => Outcome::Close,
                Modal::Quit { active } => self.quit_key(active, key),
                Modal::Columns { chosen, cursor } => self.columns_key(chosen, cursor, key),
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
            Outcome::Submit(modal, request) => {
                self.modal = Some(modal);
                self.submitted = Some(request.clone());
                Some(request)
            }
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
            Some(Modal::Value(edit)) => edit.input.insert(text),
            Some(Modal::Form(form)) => form.1.paste(text),
            None if self.share.editing && self.area == super::Area::Share => {
                self.share.form.paste(text);
            }
            _ => {}
        }
    }

    /// `d` or enter hands the active transfers to a daemon, `s` stops them with the engine,
    /// and esc stays.
    fn quit_key(&mut self, active: usize, key: KeyEvent) -> Outcome {
        match key.code {
            KeyCode::Enter | KeyCode::Char('d') => {
                self.hand_off = true;
                self.quit = true;
                Outcome::Close
            }
            KeyCode::Char('s') => {
                self.quit = true;
                Outcome::Close
            }
            _ => Outcome::Open(Modal::Quit { active }),
        }
    }

    fn help_key(&mut self, scroll: usize, key: KeyEvent) -> Outcome {
        let limit = crate::ui::help_overflow(self);
        let scroll = match key.code {
            KeyCode::Char('j') | KeyCode::Down => scroll + 1,
            KeyCode::Char('k') | KeyCode::Up => scroll.saturating_sub(1),
            KeyCode::PageDown | KeyCode::Char(' ') => scroll + 10,
            KeyCode::PageUp => scroll.saturating_sub(10),
            KeyCode::Home => 0,
            KeyCode::End => limit,
            KeyCode::Left | KeyCode::Right => scroll,
            _ => return Outcome::Close,
        };
        Outcome::Open(Modal::Help {
            scroll: scroll.min(limit),
        })
    }

    fn columns_key(&mut self, mut chosen: Vec<Column>, cursor: usize, key: KeyEvent) -> Outcome {
        let last = Column::OPTIONAL.len() - 1;
        let cursor = match key.code {
            KeyCode::Char('j') | KeyCode::Down | KeyCode::Tab => (cursor + 1).min(last),
            KeyCode::Char('k') | KeyCode::Up | KeyCode::BackTab => cursor.saturating_sub(1),
            KeyCode::Char(' ') | KeyCode::Char('x') => {
                let column = Column::OPTIONAL[cursor];
                match chosen.iter().position(|c| *c == column) {
                    Some(index) => {
                        chosen.remove(index);
                    }
                    None => chosen.push(column),
                }
                cursor
            }
            KeyCode::Enter => return Outcome::Ask(self.set_columns(chosen)),
            _ => cursor,
        };
        Outcome::Open(Modal::Columns { chosen, cursor })
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

    /// The modal's keys for the hint bar, the last always shown.
    pub(super) fn modal_hints(&self, modal: &Modal) -> Vec<Hint> {
        let scrolls = crate::ui::help_overflow(self) > 0;
        let form_hints = |form: &Form, enter: &'static str| {
            let mut hints: Vec<Hint> = form
                .hints()
                .iter()
                .map(|(keys, action)| Hint { keys, action })
                .collect();
            hints.push(Hint {
                keys: "enter",
                action: enter,
            });
            hints.push(Hint {
                keys: "esc",
                action: "cancel",
            });
            hints
        };
        let hints: &[(&'static str, &'static str)] = match modal {
            Modal::Help { .. } if scrolls => &[("j/k", "scroll"), ("esc", "close")],
            Modal::Help { .. } => &[("esc", "close")],
            Modal::Form(form) => return form_hints(&form.1, "save"),
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
            Modal::Text { .. } | Modal::Value(_) => &[("enter", "save"), ("esc", "cancel")],
            Modal::Pick {
                kind: PickKind::View,
                ..
            } => &[
                ("j/k", "choose"),
                ("enter", "show"),
                ("x", "delete"),
                ("esc", "cancel"),
            ],
            Modal::Pick { .. } => &[("j/k", "choose"), ("enter", "pick"), ("esc", "cancel")],
            Modal::Confirm { .. } => &[("enter", "yes"), ("esc", "no")],
            Modal::Network => &[("esc", "close")],
            Modal::Quit { .. } => &[
                ("d", "hand to a daemon"),
                ("s", "stop them"),
                ("esc", "stay"),
            ],
            Modal::Columns { .. } => &[
                ("j/k", "choose"),
                ("space", "show/hide"),
                ("enter", "save"),
                ("esc", "cancel"),
            ],
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

/// Asks whether to unpublish `target` from `tracker`.
pub(super) fn confirm_unpublish(target: Target, tracker: String) -> Modal {
    Modal::Confirm {
        title: "Unpublish".to_owned(),
        text: format!("Unpublish {} from {tracker}?", target.name),
        detail:
            "The tracker stops listing it, and this peer stops asking it to. The transfer keeps \
                 sharing."
                .to_owned(),
        request: Box::new(Request::Transfer(target, Action::Unpublish(tracker))),
    }
}
