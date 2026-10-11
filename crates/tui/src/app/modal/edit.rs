//! Modals that edit a value: a line of text about a transfer, a setting and the forms.

use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use super::{Modal, Outcome, is};
use crate::app::App;
use crate::app::request::{Action, Request, Target, parse_tags};
use crate::app::rows::ValueKind;
use crate::form::{self, Form};
use crate::input::Input;

/// What a [`Modal::Value`] sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueTarget {
    /// A setting, by dotted key.
    Setting {
        /// The setting.
        key: &'static str,
        /// What it takes.
        kind: ValueKind,
        /// What it is, for the notice.
        about: String,
    },
    /// A tracker's URL, replacing this one or added.
    Tracker(Option<String>),
    /// A relay's identity, replacing this one or added.
    Relay(Option<String>),
    /// The save folder of this category.
    CategoryFolder(String),
    /// The name to save the list's view under.
    ViewName,
}

/// A value being typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueEdit {
    /// The modal's title.
    pub title: String,
    /// What the value sets.
    pub target: ValueTarget,
    /// The text.
    pub input: Input,
    /// What shows while it is empty.
    pub placeholder: String,
    /// Why the value was refused, if it was.
    pub error: Option<String>,
}

impl ValueEdit {
    /// A prompt titled `title` for `target`, starting with `text`.
    pub fn new(title: String, target: ValueTarget, text: &str, placeholder: &str) -> Self {
        Self {
            title,
            target,
            input: Input::new(text),
            placeholder: placeholder.to_owned(),
            error: None,
        }
    }

    /// A line on what to type.
    pub fn help(&self) -> &'static str {
        match &self.target {
            ValueTarget::Setting { kind, .. } => kind.help(),
            ValueTarget::Tracker(_) => "An http:// or https:// URL.",
            ValueTarget::Relay(_) => "A relay's key and address: <key>@host[:port].",
            ValueTarget::CategoryFolder(_) => "Where its downloads go, as an absolute path.",
            ValueTarget::ViewName => "Its filters and order are saved under this name.",
        }
    }
}

/// What a [`Modal::Form`] is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormKind {
    /// A transfer's own speed and sharing limits.
    Limits(Target),
    /// A new category.
    NewCategory,
    /// When the alternative limits apply.
    Schedule,
}

impl FormKind {
    /// The modal's title.
    pub fn title(&self) -> String {
        match self {
            Self::Limits(target) => format!("Limits of {}", target.name),
            Self::NewCategory => "New category".to_owned(),
            Self::Schedule => "Alternative limits schedule".to_owned(),
        }
    }

    /// A line on how to fill it in.
    pub fn help(&self) -> &'static str {
        match self {
            Self::Limits(_) => {
                "Speeds such as 2M (bytes per second). Leave empty to use the Settings value, or type none for no limit."
            }
            Self::NewCategory => "Downloads added to this category save to its folder.",
            Self::Schedule => {
                "24-hour times, such as 22:00. Clear both times to turn the schedule off."
            }
        }
    }
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
            Self::Rename => "Leave empty to restore the bundle's original name.",
            Self::Tags => "Separate tags with commas, such as music, live.",
            Self::Move => "The absolute path of the folder to move the files into.",
        }
    }
}

impl App {
    pub(super) fn value_key(&mut self, mut edit: ValueEdit, key: KeyEvent) -> Outcome {
        if !is(key, KeyCode::Enter) {
            let before = edit.input.text();
            edit.input.key(key);
            // An error is about the text it refused, so editing clears it.
            if edit.input.text() != before {
                edit.error = None;
            }
            return Outcome::Open(Modal::Value(Box::new(edit)));
        }
        match self.value_request(&edit.target, &edit.input.text()) {
            // A view's name is kept at once, as the theme and columns are.
            Ok(request) if edit.target == ValueTarget::ViewName => Outcome::Ask(request),
            Ok(request) => {
                edit.error = None;
                Outcome::Submit(Modal::Value(Box::new(edit)), request)
            }
            Err(error) => {
                edit.error = Some(error);
                Outcome::Open(Modal::Value(Box::new(edit)))
            }
        }
    }

    pub(super) fn form_key(
        &mut self,
        (kind, mut form): (FormKind, Form),
        key: KeyEvent,
    ) -> Outcome {
        if form.key(key) == form::Outcome::Edited {
            return Outcome::Open(Modal::Form(Box::new((kind, form))));
        }
        match self.form_request(&kind, &form) {
            Ok(request) => {
                form.error = None;
                Outcome::Submit(Modal::Form(Box::new((kind, form))), request)
            }
            Err(error) => {
                form.error = Some(error);
                Outcome::Open(Modal::Form(Box::new((kind, form))))
            }
        }
    }
}

pub(super) fn text_key(
    kind: TextKind,
    target: Target,
    mut input: Input,
    mut error: Option<String>,
    key: KeyEvent,
) -> Outcome {
    if !is(key, KeyCode::Enter) {
        let before = input.text();
        input.key(key);
        if input.text() != before {
            error = None;
        }
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
        TextKind::Move => Err("Enter an absolute path, such as /home/you/films.".to_owned()),
    };
    match action {
        Ok(action) => {
            let request = Request::Transfer(target.clone(), action);
            let modal = Modal::Text {
                kind,
                target,
                input,
                error: None,
            };
            Outcome::Submit(modal, request)
        }
        Err(error) => Outcome::Open(Modal::Text {
            kind,
            target,
            input,
            error: Some(error),
        }),
    }
}
