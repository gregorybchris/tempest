//! The TUI's state and what keys do to it. Nothing here draws (see `ui`) or does I/O: what
//! needs the engine or the system comes back as a [`Request`] for the runtime to carry out.

mod modal;
mod request;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tempest_engine::api::QueueStep;
use tempest_engine::api::client::ApiEvent;
use tempest_engine::{Status, TransferInfo};
use tempest_protocol::BundleId;

pub use self::modal::{AddField, AddForm, Modal, PickItem, PickKind, TextKind};
pub use self::request::{Action, Reply, Request, Target, parse_tags};
use crate::commands::{Command, Section};
use crate::finding::{Finding, SortKey, StatusFilter};
use crate::input::Input;
use crate::model::Model;
use crate::theme::Theme;

/// The app's areas (D-136), in navigation order. Transfers is home.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Area {
    /// The transfers list.
    Transfers,
    /// Creating a bundle and getting its link.
    Share,
    /// Trackers and what they offer.
    Catalog,
    /// Limits, queue, categories and theme.
    Settings,
}

impl Area {
    /// Every area, in navigation order.
    pub const ALL: [Self; 4] = [Self::Transfers, Self::Share, Self::Catalog, Self::Settings];

    /// Its name in the navigation.
    pub fn title(self) -> &'static str {
        match self {
            Self::Transfers => "Transfers",
            Self::Share => "Share",
            Self::Catalog => "Catalog",
            Self::Settings => "Settings",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|area| *area == self).unwrap_or(0)
    }

    fn step(self, by: isize) -> Self {
        let count = Self::ALL.len() as isize;
        Self::ALL[(self.index() as isize + by).rem_euclid(count) as usize]
    }
}

/// Where the engine the TUI talks to runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// A daemon that outlives the TUI.
    Daemon,
    /// An engine started by the TUI, which stops when it quits.
    Embedded,
}

/// A key and what it does, as the hint bar shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hint {
    /// The key or keys.
    pub keys: &'static str,
    /// What they do.
    pub action: &'static str,
}

/// A message at the bottom of the screen, about something that just happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// The message.
    pub text: String,
    /// Whether it reports a failure.
    pub error: bool,
    /// Ticks left before it goes.
    ticks_left: u8,
}

/// How many ticks (seconds) a notice stays.
const NOTICE_TICKS: u8 = 5;

/// How far page up and down move.
const PAGE: isize = 10;

/// The whole TUI's state.
#[derive(Debug, Clone)]
pub struct App {
    /// What the engine has said.
    pub model: Model,
    /// Where the engine runs.
    pub origin: Origin,
    /// Whether the engine runs on this machine, so its folders can be opened here.
    pub local: bool,
    /// The area shown.
    pub area: Area,
    /// The palette.
    pub theme: Theme,
    /// Which transfers the list shows, and in what order.
    pub finding: Finding,
    /// The transfer the cursor is on.
    pub selected: Option<BundleId>,
    /// What is open over the list.
    pub modal: Option<Modal>,
    /// The categories' names, once fetched.
    pub categories: Option<Vec<String>>,
    /// A message about something that just happened.
    pub notice: Option<Notice>,
    /// Whether the user asked to quit.
    pub quit: bool,
}

impl App {
    /// A TUI talking to an engine that runs as `origin` on this machine, which it hasn't heard
    /// from yet.
    pub fn new(origin: Origin, theme: Theme) -> Self {
        Self {
            model: Model::new(),
            origin,
            local: true,
            area: Area::Transfers,
            theme,
            finding: Finding::default(),
            selected: None,
            modal: None,
            categories: None,
            notice: None,
            quit: false,
        }
    }

    /// The transfers the list shows, in order.
    pub fn visible(&self) -> Vec<&TransferInfo> {
        self.finding.apply(self.model.transfers())
    }

    /// Where the cursor is in [`App::visible`].
    pub fn cursor(&self) -> Option<usize> {
        let id = self.selected?;
        self.visible().iter().position(|transfer| transfer.id == id)
    }

    /// The transfer the cursor is on.
    pub fn selected_transfer(&self) -> Option<&TransferInfo> {
        self.model.transfer(&self.selected?)
    }

    /// Whether `command` would do anything now.
    pub fn works(&self, command: Command) -> bool {
        if !command.section().works_in(self.area) {
            return false;
        }
        let selected = self.selected.is_some();
        match command.section() {
            Section::Move => !self.visible().is_empty(),
            Section::Transfer | Section::Queue => command == Command::Add || selected,
            Section::Find => command != Command::ClearFilters || self.finding.filtering(),
            Section::App => true,
        }
    }

    /// The keys that work right now, most useful first. The last always shows.
    pub fn hints(&self) -> Vec<Hint> {
        if let Some(modal) = &self.modal {
            return Self::modal_hints(modal);
        }
        const ORDER: [Command; 12] = [
            Command::ClearFilters,
            Command::Down,
            Command::StartStop,
            Command::Filter,
            Command::Add,
            Command::Palette,
            Command::Remove,
            Command::Sort,
            Command::GoTransfers,
            Command::Theme,
            Command::Quit,
            Command::Help,
        ];
        ORDER
            .into_iter()
            .filter(|command| self.works(*command))
            .filter_map(Command::hint)
            .map(|(keys, action)| Hint { keys, action })
            .collect()
    }

    /// The current area's commands by section, for the help overlay.
    pub fn help_sections(&self) -> Vec<(Section, Vec<Command>)> {
        let mut sections: Vec<(Section, Vec<Command>)> = Vec::new();
        for command in Command::ALL {
            let section = command.section();
            if !section.works_in(self.area) {
                continue;
            }
            match sections.last_mut() {
                Some((last, commands)) if *last == section => commands.push(command),
                _ => sections.push((section, vec![command])),
            }
        }
        sections
    }

    /// Takes in an event from the engine, keeping the cursor on a transfer that is shown.
    pub fn apply(&mut self, event: ApiEvent) {
        let cursor = self.cursor();
        self.model.apply(event);
        self.keep_cursor(cursor);
    }

    /// Moves the cursor to a shown transfer if it is on one that isn't, near where it was.
    fn keep_cursor(&mut self, was: Option<usize>) {
        if self.cursor().is_none() {
            let visible = self.visible();
            let index = was.unwrap_or(0).min(visible.len().saturating_sub(1));
            self.selected = visible.get(index).map(|transfer| transfer.id);
        }
    }

    /// Changes the filters or the order, keeping the cursor on a shown transfer.
    fn change_finding(&mut self, change: impl FnOnce(&mut Finding)) {
        let cursor = self.cursor();
        change(&mut self.finding);
        self.keep_cursor(cursor);
    }

    fn set_filter_text(&mut self, text: String) {
        self.change_finding(|finding| finding.text = text);
    }

    /// Called once a second.
    pub fn tick(&mut self) {
        self.model.sample();
        if let Some(notice) = &mut self.notice {
            notice.ticks_left = notice.ticks_left.saturating_sub(1);
            if notice.ticks_left == 0 {
                self.notice = None;
            }
        }
    }

    /// Shows a message about something that just happened.
    pub fn notify(&mut self, text: impl Into<String>, error: bool) {
        self.notice = Some(Notice {
            text: text.into(),
            error,
            ticks_left: NOTICE_TICKS,
        });
    }

    /// Reports how a request went: `Err` holds the engine's explanation.
    pub fn finished(&mut self, request: &Request, result: Result<Reply, String>) {
        match result {
            Ok(Reply::Categories(categories)) => self.categories = Some(categories),
            Ok(reply) => {
                if let Some(text) = request.success(&reply) {
                    self.notify(text, false);
                }
            }
            Err(error) => self.notify(request.failure(&error), true),
        }
    }

    /// Handles a key press, returning what to ask for, if anything.
    pub fn key(&mut self, key: KeyEvent) -> Option<Request> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return None;
        }
        if let Some(modal) = self.modal.take() {
            return self.modal_key(modal, key);
        }
        self.run(Command::for_key(self.area, key)?)
    }

    /// Runs `command`, from its key or the palette.
    pub fn run(&mut self, command: Command) -> Option<Request> {
        if !command.section().works_in(self.area) {
            return None;
        }
        match command {
            Command::Down => self.move_cursor(1),
            Command::Up => self.move_cursor(-1),
            Command::First => self.move_cursor(isize::MIN),
            Command::Last => self.move_cursor(isize::MAX),
            Command::PageDown => self.move_cursor(PAGE),
            Command::PageUp => self.move_cursor(-PAGE),

            Command::Add => {
                self.modal = Some(Modal::Add(Box::default()));
                return Some(Request::Categories);
            }
            Command::Filter => {
                self.modal = Some(Modal::Filter(Input::new(&self.finding.text)));
            }
            Command::StatusFilter => {
                let selected = self.finding.status.map_or(0, |status| {
                    1 + StatusFilter::ALL
                        .iter()
                        .position(|s| *s == status)
                        .unwrap_or(0)
                });
                self.modal = Some(Modal::Pick {
                    kind: PickKind::Status,
                    selected,
                });
            }
            Command::Sort => {
                let key = self.finding.sort.key;
                let selected = SortKey::ALL.iter().position(|k| *k == key).unwrap_or(0);
                self.modal = Some(Modal::Pick {
                    kind: PickKind::Sort,
                    selected,
                });
            }
            Command::Reverse => {
                self.change_finding(|finding| finding.sort.reversed = !finding.sort.reversed);
            }
            Command::ClearFilters => self.change_finding(|finding| {
                finding.text.clear();
                finding.status = None;
            }),

            Command::Palette => {
                self.modal = Some(Modal::Palette {
                    query: Input::default(),
                    selected: 0,
                });
            }
            Command::GoTransfers => self.area = Area::Transfers,
            Command::GoShare => self.area = Area::Share,
            Command::GoCatalog => self.area = Area::Catalog,
            Command::GoSettings => self.area = Area::Settings,
            Command::NextArea => self.area = self.area.step(1),
            Command::PreviousArea => self.area = self.area.step(-1),
            Command::Theme => self.theme = self.theme.next(),
            Command::Help => self.modal = Some(Modal::Help),
            Command::Quit => self.quit = true,

            transfer_command => return self.run_on_selected(transfer_command),
        }
        None
    }

    /// Runs a command that acts on the selected transfer.
    fn run_on_selected(&mut self, command: Command) -> Option<Request> {
        let Some(transfer) = self.selected_transfer() else {
            self.notify("No transfer is selected.", true);
            return None;
        };
        let target = Target {
            id: transfer.id,
            name: transfer.name.clone(),
        };
        let text = |kind, text: &str| Modal::Text {
            kind,
            target: target.clone(),
            input: Input::new(text),
            error: None,
        };
        let action = match command {
            Command::StartStop => return self.start_or_stop(),
            Command::ForceStart => Action::ForceStart,
            Command::Recheck => Action::Recheck,
            Command::QueueUp => Action::Queue(QueueStep::Up),
            Command::QueueDown => Action::Queue(QueueStep::Down),
            Command::QueueTop => Action::Queue(QueueStep::Top),
            Command::QueueBottom => Action::Queue(QueueStep::Bottom),
            Command::Rename => {
                self.modal = Some(text(TextKind::Rename, &transfer.name));
                return None;
            }
            Command::Tags => {
                let tags: Vec<&str> = transfer.tags.iter().map(String::as_str).collect();
                self.modal = Some(text(TextKind::Tags, &tags.join(", ")));
                return None;
            }
            Command::MoveStorage => {
                let folder = transfer.save_folder.display().to_string();
                self.modal = Some(text(TextKind::Move, &folder));
                return None;
            }
            Command::Remove => {
                self.modal = Some(Modal::Remove {
                    target,
                    delete_files: false,
                });
                return None;
            }
            Command::Category => {
                let selected = transfer.category.as_ref().map_or(0, |current| {
                    self.categories
                        .iter()
                        .flatten()
                        .position(|c| c == current)
                        .map_or(0, |n| n + 1)
                });
                self.modal = Some(Modal::Pick {
                    kind: PickKind::Category(target),
                    selected,
                });
                return Some(Request::Categories);
            }
            Command::Open => {
                let folder = transfer
                    .content_dir
                    .clone()
                    .unwrap_or_else(|| transfer.save_folder.clone());
                if self.local {
                    return Some(Request::Open(folder));
                }
                let text = format!(
                    "{} is in {} on the engine's machine.",
                    target.name,
                    folder.display()
                );
                self.notify(text, false);
                return None;
            }
            _ => return None,
        };
        Some(Request::Transfer(target, action))
    }

    fn move_cursor(&mut self, by: isize) {
        let visible = self.visible();
        let Some(last) = visible.len().checked_sub(1) else {
            return;
        };
        let target = match self.cursor() {
            Some(cursor) => cursor.saturating_add_signed(by).min(last),
            None => 0,
        };
        self.selected = Some(visible[target].id);
    }

    fn start_or_stop(&mut self) -> Option<Request> {
        let transfer = self.selected_transfer()?;
        let target = Target {
            id: transfer.id,
            name: transfer.name.clone(),
        };
        let action = match transfer.status {
            Status::Stopped | Status::Completed | Status::Errored { .. } => Action::Start,
            Status::Moving => {
                let text = format!("{} can't stop while its files are moving.", transfer.name);
                self.notify(text, true);
                return None;
            }
            _ => Action::Stop,
        };
        Some(Request::Transfer(target, action))
    }
}

#[cfg(test)]
mod tests;
