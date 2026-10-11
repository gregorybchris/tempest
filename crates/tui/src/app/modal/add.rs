//! The add modal: a link, where to save it, its category and tags, and whether it starts.

use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use tempest_api::requests::AddRequest;

use super::{Modal, Outcome, is};
use crate::app::App;
use crate::app::request::{Request, parse_tags};
use crate::input::Input;

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

    pub(super) fn step(self, by: isize) -> Self {
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
    pub(super) fn request(&self) -> Result<AddRequest, String> {
        let link = self.link.text().trim().to_owned();
        if link.is_empty() {
            return Err("Enter a tempest: link.".to_owned());
        }
        let folder = self.save_folder.text().trim().to_owned();
        let save_folder = match folder.is_empty() {
            true => None,
            false if Path::new(&folder).is_absolute() => Some(PathBuf::from(folder)),
            false => return Err("Enter an absolute path, such as /home/you/films.".to_owned()),
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

impl App {
    pub(super) fn add_key(&mut self, mut form: AddForm, key: KeyEvent) -> Outcome {
        let focus = form.focus;
        if is(key, KeyCode::Enter) {
            return match form.request() {
                Ok(request) => {
                    form.error = None;
                    Outcome::Submit(Modal::Add(Box::new(form)), Request::Add(Box::new(request)))
                }
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
    pub(super) fn step_category(&self, current: Option<&str>, by: isize) -> Option<String> {
        let mut options: Vec<Option<&str>> = vec![None];
        options.extend(
            self.categories
                .iter()
                .flatten()
                .map(|(c, _)| Some(c.as_str())),
        );
        let index = options.iter().position(|o| *o == current).unwrap_or(0) as isize;
        let index = (index + by).rem_euclid(options.len() as isize) as usize;
        options[index].map(str::to_owned)
    }
}
