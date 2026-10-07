//! The TUI's state and what keys do to it. Nothing here draws (see `ui`) or does I/O: keys that
//! need the engine return a [`Request`] for the runtime to send.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tempest_engine::api::client::ApiEvent;
use tempest_engine::{Status, TransferInfo};
use tempest_protocol::BundleId;

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

/// Something only the engine can do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Start a transfer through the queue.
    Start(BundleId),
    /// Start a transfer regardless of the queue.
    ForceStart(BundleId),
    /// Stop a transfer.
    Stop(BundleId),
}

impl Request {
    /// The transfer the request is about.
    pub fn id(&self) -> BundleId {
        match self {
            Self::Start(id) | Self::ForceStart(id) | Self::Stop(id) => *id,
        }
    }

    /// What it does, as a verb for messages: "start", "stop".
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Start(_) => "start",
            Self::ForceStart(_) => "force-start",
            Self::Stop(_) => "stop",
        }
    }
}

/// A key and what it does, as the hint bar and help show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    /// The key or keys.
    pub keys: &'static str,
    /// What they do.
    pub action: &'static str,
}

const fn bind(keys: &'static str, action: &'static str) -> Binding {
    Binding { keys, action }
}

const MOVE: Binding = bind("j/k", "move");
const TOP_BOTTOM: Binding = bind("g/G", "top/bottom");
const START_STOP: Binding = bind("space", "start/stop");
const FORCE_START: Binding = bind("F", "force start");
const AREAS: Binding = bind("1-4", "areas");
const NEXT_PREVIOUS_AREA: Binding = bind("[ ]", "previous/next area");
const THEME: Binding = bind("T", "theme");
const HELP: Binding = bind("?", "help");
const QUIT: Binding = bind("q", "quit");
const CLOSE: Binding = bind("esc", "close");

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

/// The whole TUI's state.
#[derive(Debug, Clone)]
pub struct App {
    /// What the engine has said.
    pub model: Model,
    /// Where the engine runs.
    pub origin: Origin,
    /// The area shown.
    pub area: Area,
    /// The palette.
    pub theme: Theme,
    /// The transfer the cursor is on.
    pub selected: Option<BundleId>,
    /// Whether the help overlay is open.
    pub help: bool,
    /// A message about something that just happened.
    pub notice: Option<Notice>,
    /// Whether the user asked to quit.
    pub quit: bool,
}

impl App {
    /// A TUI talking to an engine that runs as `origin`, which it hasn't heard from yet.
    pub fn new(origin: Origin, theme: Theme) -> Self {
        Self {
            model: Model::new(),
            origin,
            area: Area::Transfers,
            theme,
            selected: None,
            help: false,
            notice: None,
            quit: false,
        }
    }

    /// The transfers the list shows, in order.
    pub fn visible(&self) -> Vec<&TransferInfo> {
        self.model.transfers().iter().collect()
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

    /// The keys that work right now, most useful first, and help last.
    pub fn hints(&self) -> Vec<Binding> {
        if self.help {
            return vec![CLOSE];
        }
        let mut hints = Vec::new();
        if self.area == Area::Transfers && self.selected.is_some() {
            hints.extend([MOVE, START_STOP, FORCE_START]);
        }
        hints.extend([AREAS, THEME, QUIT, HELP]);
        hints
    }

    /// Every key of the current screen, for the help overlay.
    pub fn help_bindings(&self) -> Vec<Binding> {
        let mut bindings = Vec::new();
        if self.area == Area::Transfers {
            bindings.extend([MOVE, TOP_BOTTOM, START_STOP, FORCE_START]);
        }
        bindings.extend([AREAS, NEXT_PREVIOUS_AREA, THEME, HELP, QUIT]);
        bindings
    }

    /// Takes in an event from the engine, keeping the cursor on a transfer that exists.
    pub fn apply(&mut self, event: ApiEvent) {
        let cursor = self.cursor();
        self.model.apply(event);
        if self.cursor().is_none() {
            let visible = self.visible();
            let index = cursor.unwrap_or(0).min(visible.len().saturating_sub(1));
            self.selected = visible.get(index).map(|transfer| transfer.id);
        }
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
    pub fn finished(&mut self, request: &Request, result: Result<(), String>) {
        let name = self
            .model
            .transfer(&request.id())
            .map_or_else(|| "the transfer".to_owned(), |t| t.name.clone());
        match result {
            Ok(()) => {}
            Err(error) => self.notify(format!("Couldn't {} {name}: {error}", request.verb()), true),
        }
    }

    /// Handles a key press, returning what to ask the engine, if anything.
    pub fn key(&mut self, key: KeyEvent) -> Option<Request> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return None;
        }
        if self.help {
            self.help = false;
            return None;
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('T') => self.theme = self.theme.next(),
            KeyCode::Char(digit @ '1'..='4') => {
                self.area = Area::ALL[digit as usize - '1' as usize];
            }
            KeyCode::Char(']') => self.area = self.area.step(1),
            KeyCode::Char('[') => self.area = self.area.step(-1),
            _ if self.area == Area::Transfers => return self.transfers_key(key),
            _ => {}
        }
        None
    }

    fn transfers_key(&mut self, key: KeyEvent) -> Option<Request> {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_cursor(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_cursor(-1),
            KeyCode::Char('g') | KeyCode::Home => self.move_cursor(isize::MIN),
            KeyCode::Char('G') | KeyCode::End => self.move_cursor(isize::MAX),
            KeyCode::PageDown => self.move_cursor(10),
            KeyCode::PageUp => self.move_cursor(-10),
            KeyCode::Char(' ') => return self.start_or_stop(),
            KeyCode::Char('F') => {
                return self.selected_transfer().map(|t| Request::ForceStart(t.id));
            }
            _ => {}
        }
        None
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
        let id = transfer.id;
        match transfer.status {
            Status::Stopped | Status::Completed | Status::Errored { .. } => {
                Some(Request::Start(id))
            }
            Status::Moving => {
                let text = format!("{} can't stop while its files are moving.", transfer.name);
                self.notify(text, true);
                None
            }
            _ => Some(Request::Stop(id)),
        }
    }
}

#[cfg(test)]
mod tests {
    use tempest_engine::api::RemovedInfo;

    use super::*;
    use crate::model::tests::transfer;

    fn press(app: &mut App, code: KeyCode) -> Option<Request> {
        app.key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn app_with(transfers: Vec<TransferInfo>) -> App {
        let mut app = App::new(Origin::Daemon, Theme::default());
        app.apply(ApiEvent::Snapshot(transfers));
        app
    }

    fn selected_name(app: &App) -> Option<&str> {
        app.selected_transfer().map(|t| t.name.as_str())
    }

    #[test]
    fn the_cursor_moves_within_the_list_and_survives_removals() {
        let mut app = app_with(vec![
            transfer(1, "a", 0),
            transfer(2, "b", 1),
            transfer(3, "c", 2),
        ]);
        assert_eq!(selected_name(&app), Some("a"));
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(selected_name(&app), Some("a"));
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(selected_name(&app), Some("b"));
        press(&mut app, KeyCode::Char('G'));
        assert_eq!(selected_name(&app), Some("c"));

        // Removing the selected transfer moves the cursor to its neighbor.
        let id = app.selected.unwrap();
        app.apply(ApiEvent::Removed(RemovedInfo { id }));
        assert_eq!(selected_name(&app), Some("b"));
        app.apply(ApiEvent::Snapshot(vec![]));
        assert_eq!(app.selected, None);
        assert_eq!(press(&mut app, KeyCode::Char(' ')), None);
    }

    #[test]
    fn space_starts_what_is_stopped_and_stops_what_runs() {
        let mut running = transfer(2, "running", 1);
        running.status = Status::Downloading;
        let mut app = app_with(vec![transfer(1, "stopped", 0), running]);
        let stopped_id = app.selected.unwrap();
        assert_eq!(
            press(&mut app, KeyCode::Char(' ')),
            Some(Request::Start(stopped_id))
        );
        press(&mut app, KeyCode::Down);
        let running_id = app.selected.unwrap();
        assert_eq!(
            press(&mut app, KeyCode::Char(' ')),
            Some(Request::Stop(running_id))
        );
        assert_eq!(
            press(&mut app, KeyCode::Char('F')),
            Some(Request::ForceStart(running_id))
        );
    }

    #[test]
    fn areas_switch_by_number_or_bracket_and_wrap() {
        let mut app = app_with(vec![]);
        press(&mut app, KeyCode::Char('3'));
        assert_eq!(app.area, Area::Catalog);
        press(&mut app, KeyCode::Char(']'));
        press(&mut app, KeyCode::Char(']'));
        assert_eq!(app.area, Area::Transfers);
        press(&mut app, KeyCode::Char('['));
        assert_eq!(app.area, Area::Settings);
    }

    #[test]
    fn hints_list_only_keys_that_work() {
        let mut app = app_with(vec![]);
        assert!(!app.hints().contains(&START_STOP), "nothing to start");
        app.apply(ApiEvent::Snapshot(vec![transfer(1, "a", 0)]));
        assert!(app.hints().contains(&START_STOP));
        press(&mut app, KeyCode::Char('2'));
        assert!(!app.hints().contains(&START_STOP), "not on Share");
        press(&mut app, KeyCode::Char('?'));
        assert_eq!(app.hints(), [CLOSE]);
        press(&mut app, KeyCode::Char('x'));
        assert!(!app.help, "any key closes help");
    }

    #[test]
    fn notices_report_failures_and_fade() {
        let mut app = app_with(vec![transfer(1, "a", 0)]);
        let request = Request::Start(app.selected.unwrap());
        app.finished(&request, Err("The engine is shutting down.".to_owned()));
        let notice = app.notice.clone().unwrap();
        assert_eq!(
            notice.text,
            "Couldn't start a: The engine is shutting down."
        );
        assert!(notice.error);
        for _ in 0..NOTICE_TICKS {
            app.tick();
        }
        assert_eq!(app.notice, None);
    }
}
