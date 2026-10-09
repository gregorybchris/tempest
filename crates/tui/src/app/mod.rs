//! The TUI's state and what keys do to it. Nothing here draws (see `ui`) or does I/O: what
//! needs the engine or the system comes back as a [`Request`] for the runtime to carry out.

mod hints;
mod inside;
mod keys;
mod limits;
mod modal;
mod request;
mod rows;
mod screen;
mod share;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use serde_json::json;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tempest_api::ApiEvent;
use tempest_api::requests::QueueStep;
use tempest_api::ui::SavedView;
use tempest_api::{Status, TransferInfo, settings::SettingsInfo};
use tempest_protocol::BundleId;

pub use self::inside::{Fetched, Inside, Look};
use self::modal::confirm_unpublish;
pub use self::modal::{
    AddField, AddForm, FormKind, Modal, PickItem, PickKind, TextKind, ValueEdit, ValueTarget,
};
pub use self::request::{Action, Reply, Request, Target, parse_tags};
pub use self::rows::{Edit, Row, Section as RowSection, ValueKind, describe_schedule};
pub use self::screen::{Area, Exit, Origin, Screen, Tab, View};
pub use self::share::ShareArea;
use crate::chunkmap;
use crate::columns::Column;
use crate::commands::{Command, Section};
use crate::finding::{Finding, SortKey, StatusFilter};
use crate::input::Input;
use crate::model::Model;
use crate::prefs;
use crate::theme::Theme;
use tempest_api::model::Connection;
use tempest_api::notices::{self, Seen};

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

/// How many ticks (seconds) the Settings area waits between fetching the settings,
/// which have no events.
const SETTINGS_TICKS: u8 = 5;

/// The most rows the summary tab has.
pub const SUMMARY_ROWS: usize = 14;

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
    /// How far into the selected transfer the Transfers area goes.
    pub view: View,
    /// The detail's tab.
    pub tab: Tab,
    /// How far the detail's tab is scrolled: rows from the top, or for decisions from the
    /// newest.
    pub scroll: usize,
    /// What has been fetched from inside the selected transfer.
    pub inside: Inside,
    /// The terminal's width and height.
    pub size: (u16, u16),
    /// What is open over the list.
    pub modal: Option<Modal>,
    /// Every category's save folder by name, once fetched.
    pub categories: Option<BTreeMap<String, PathBuf>>,
    /// The engine's settings, once fetched.
    pub settings: Option<SettingsInfo>,
    /// Why the settings couldn't be fetched, if they couldn't.
    pub settings_error: Option<String>,
    /// Whether the settings are being fetched.
    settings_pending: bool,
    /// Ticks since the settings were last fetched.
    settings_age: u8,
    /// Whether the theme, columns and views kept in the settings have been taken in.
    prefs_loaded: bool,
    /// Changes to what the user keeps that haven't been answered yet.
    ui_pending: usize,
    /// The list's optional columns that show (D-197).
    pub columns: Vec<Column>,
    /// The saved views.
    pub views: Vec<SavedView>,
    /// The Share area's state.
    pub share: ShareArea,
    /// The cursor among the Settings area's rows.
    settings_row: usize,
    /// A message about something that just happened.
    pub notice: Option<Notice>,
    /// What each transfer showed when last looked at, to notice when one completes or fails.
    seen: HashMap<BundleId, Seen>,
    /// Whether the user asked to quit.
    pub quit: bool,
    /// Whether quitting hands the active transfers to a daemon rather than stopping them.
    pub hand_off: bool,
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
            view: View::List,
            tab: Tab::Summary,
            scroll: 0,
            inside: Inside::default(),
            size: (80, 24),
            modal: None,
            categories: None,
            settings: None,
            settings_error: None,
            settings_pending: false,
            settings_age: 0,
            prefs_loaded: false,
            ui_pending: 0,
            columns: Column::DEFAULT.to_vec(),
            views: Vec::new(),
            share: ShareArea::default(),
            settings_row: 0,
            notice: None,
            seen: HashMap::new(),
            quit: false,
            hand_off: false,
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

    /// What has the keys.
    pub fn screen(&self) -> Screen {
        match (self.area, self.view) {
            (Area::Transfers, View::List) => Screen::List,
            (Area::Transfers, View::Detail) => Screen::Detail,
            (Area::Transfers, View::Chunks) => Screen::Chunks,
            (Area::Transfers, View::Rates) => Screen::Rates,
            (Area::Share, _) => Screen::Share,
            (Area::Settings, _) => Screen::Settings,
        }
    }

    /// Takes in an event from the engine, keeping the cursor on a transfer that is shown.
    pub fn apply(&mut self, event: ApiEvent) {
        let cursor = self.cursor();
        let touched = match &event {
            ApiEvent::Transfer(transfer) => Some(transfer.id),
            ApiEvent::Removed(removed) => {
                self.seen.remove(&removed.id);
                None
            }
            _ => None,
        };
        let whole = matches!(event, ApiEvent::Snapshot(_) | ApiEvent::Reset(_));
        self.model.apply(event);
        self.notice_changes(touched, whole);
        self.keep_cursor(cursor);
        // With nothing selected, there is nothing to look inside.
        if self.selected.is_none() {
            self.view = View::List;
        }
    }

    /// Tells the user of a transfer that completed or failed (D-226). `touched` is the one an
    /// event changed; `whole` means the event replaced every transfer, which says nothing the
    /// first time.
    fn notice_changes(&mut self, touched: Option<BundleId>, whole: bool) {
        let transfers = self.model.transfers();
        let after = match touched {
            Some(id) => transfers.iter().filter(|t| t.id == id).cloned().collect(),
            None if whole => transfers.to_vec(),
            None => return,
        };
        let told = notices::notices(&self.seen, &after);
        if whole {
            self.seen.clear();
        }
        self.seen.extend(after.iter().map(|t| (t.id, Seen::of(t))));
        for notice in told {
            let (title, body) = notice.text();
            let failed = matches!(notice, notices::Notice::Failed { .. });
            self.notify(format!("{title}. {body}"), failed);
        }
    }

    /// Moves the cursor to a shown transfer if it is on one that isn't, near where it was.
    fn keep_cursor(&mut self, was: Option<usize>) {
        if self.cursor().is_none() {
            let visible = self.visible();
            let index = was.unwrap_or(0).min(visible.len().saturating_sub(1));
            self.selected = visible.get(index).map(|transfer| transfer.id);
            // Another transfer's detail starts at its top.
            self.scroll = 0;
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

    /// Called once a second. Returns the fetches that keep the open view up to date.
    pub fn tick(&mut self) -> Vec<Request> {
        // Rates known before the stream broke are no measure of now.
        if self.model.connection == Connection::Live {
            self.model.sample();
        }
        if let Some(notice) = &mut self.notice {
            notice.ticks_left = notice.ticks_left.saturating_sub(1);
            if notice.ticks_left == 0 {
                self.notice = None;
            }
        }
        self.settings_age = self.settings_age.saturating_add(1);
        let rows = self.area == Area::Settings;
        let stale = rows && self.settings_age >= SETTINGS_TICKS;
        // Until the first answer, ask at once, then again every few seconds after a failure.
        let unloaded = !self.prefs_loaded
            && (self.settings_error.is_none() || self.settings_age >= SETTINGS_TICKS);
        let settings = (stale || unloaded).then(|| self.load_settings()).flatten();
        self.fetch().into_iter().chain(settings).collect()
    }

    /// The fetch of the settings and categories, unless one is under way.
    pub fn load_settings(&mut self) -> Option<Request> {
        if self.settings_pending {
            return None;
        }
        self.settings_pending = true;
        Some(Request::Settings)
    }

    /// Takes in the engine's settings. What the user keeps (D-195) is taken in once, at the
    /// start, and again whenever no change to it is waiting for an answer, so a fetch never
    /// undoes a change just made.
    fn take_settings(&mut self, view: SettingsInfo) {
        let ui = &view.settings.ui;
        if !self.prefs_loaded {
            // A theme changed before the settings arrived stays.
            if self.ui_pending == 0 {
                self.theme = prefs::theme(ui);
            }
            self.prefs_loaded = true;
        }
        if self.ui_pending == 0 {
            self.columns = prefs::columns(ui);
            self.views = ui.views.clone();
        }
        self.settings = Some(view);
        self.settings_error = None;
        self.settings_age = 0;
    }

    /// On the first run, shows what the network defaults share, and records that it did
    /// (D-227). Returns the request that records it.
    fn explain_network(&mut self) -> Option<Request> {
        let explained = self.settings.as_ref()?.settings.ui.network_explained;
        if explained || self.modal.is_some() {
            return None;
        }
        self.modal = Some(Modal::Network);
        Some(self.change_settings([(prefs::NETWORK_EXPLAINED, json!(true))], None))
    }

    fn take_categories(&mut self, categories: BTreeMap<String, PathBuf>) {
        let names: Vec<String> = categories.keys().cloned().collect();
        self.offer_categories(&names);
        self.categories = Some(categories);
    }

    /// Shows `area`, fetching what it needs.
    fn go(&mut self, area: Area) -> Option<Request> {
        self.area = area;
        match area {
            Area::Transfers => None,
            Area::Share => Some(Request::Categories),
            Area::Settings => self.load_settings(),
        }
    }

    /// Notes the terminal's size, which decides how many cells the chunk map asks for.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.size = (width, height);
    }

    /// The fetch the open view needs from inside the selected transfer, unless one is under
    /// way.
    pub fn fetch(&mut self) -> Option<Request> {
        let id = self.selected?;
        let look = match self.screen() {
            Screen::Detail if self.tab == Tab::Files => Look::Files,
            Screen::Detail if self.tab == Tab::Peers => Look::Peers,
            Screen::Chunks => Look::Chunks(chunkmap::cells_for_screen(self.size.0, self.size.1)),
            _ => return None,
        };
        self.inside
            .start(id, look)
            .then_some(Request::Look(id, look))
    }

    /// What has been fetched from inside the selected transfer.
    pub fn inside(&self) -> Option<&Inside> {
        self.inside.of(self.selected?)
    }

    /// The number of transfers that run or wait to run.
    pub fn active(&self) -> usize {
        (self.model.transfers().iter())
            .filter(|transfer| transfer.status.is_active())
            .count()
    }

    /// Quits, or first asks what to do with the active transfers when the engine runs only
    /// for this TUI (D-228).
    fn ask_to_quit(&mut self) {
        let active = self.active();
        match self.origin {
            Origin::Embedded if active > 0 => self.modal = Some(Modal::Quit { active }),
            _ => self.quit = true,
        }
    }

    /// What the engine should do now that the TUI has quit.
    pub fn exit(&self) -> Exit {
        if self.hand_off {
            Exit::HandOff
        } else {
            Exit::Stop
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

    /// Reports how a request went: `Err` holds the engine's explanation. Returns what to ask
    /// next, if anything.
    pub fn finished(
        &mut self,
        request: &Request,
        result: Result<Reply, String>,
    ) -> Option<Request> {
        match request {
            Request::Look(id, look) => {
                let result = result.map_err(|error| request.failure(&error));
                self.inside.receive(*id, *look, result);
                return None;
            }
            Request::Settings => {
                self.settings_pending = false;
                if let Err(error) = &result {
                    self.settings_error = Some(request.failure(error));
                    self.settings_age = 0;
                }
            }
            Request::ChangeSettings { changes, .. }
                if changes.keys().any(|key| key.starts_with("ui.")) =>
            {
                self.ui_pending = self.ui_pending.saturating_sub(1);
            }
            Request::Share(_) => {
                let error = result.as_ref().err().map(|e| request.failure(e));
                match (&result, &error) {
                    (Ok(Reply::Shared(transfer)), _) => self.shared(Ok(transfer)),
                    (_, Some(error)) => self.shared(Err(error)),
                    _ => self.shared(Err("The engine's answer was unexpected.")),
                }
            }
            _ => {}
        }
        let mut next = None;
        match result {
            Ok(Reply::Categories(categories)) => self.take_categories(categories),
            Ok(Reply::Loaded(view, categories)) => {
                self.take_settings(*view);
                self.take_categories(categories);
                next = self.explain_network();
            }
            Ok(reply) => {
                if let Reply::Settings(view) = &reply {
                    self.take_settings((**view).clone());
                }
                if let Request::SetCategory { .. } | Request::RemoveCategory(_) = request {
                    next = Some(Request::Categories);
                }
                if let Some(text) = request.success(&reply) {
                    self.notify(text, false);
                }
            }
            Err(_) if matches!(request, Request::Settings) => {}
            Err(error) => {
                self.notify(request.failure(&error), true);
                // A refused change to what the user keeps: show what the engine has.
                if matches!(request, Request::ChangeSettings { .. }) && self.ui_pending == 0 {
                    next = self.load_settings();
                }
            }
        }
        next
    }
}

/// What was fetched, if it was.
pub fn loaded<T>(fetched: &Fetched<T>) -> Option<&T> {
    fetched.as_ref()?.as_ref().ok()
}

#[cfg(test)]
mod area_tests;
#[cfg(test)]
mod tests;
