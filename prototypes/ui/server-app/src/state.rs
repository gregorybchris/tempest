//! Server-owned state: the simulated world and per-session UI state, plus the
//! actions the browser can trigger.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use proto_core::chart::{self, Layout};
use proto_core::{Sample, World};
use tokio::sync::watch;

use crate::render::{Ui, CHART_HEIGHT, CHART_WIDTH, THEMES};

pub const TICK: Duration = Duration::from_millis(250);

pub struct AppState {
    pub world: RwLock<World>,
    /// Bumped after every world tick.
    pub ticks: watch::Receiver<u64>,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
}

/// One browser session's UI state. `changed` wakes its update streams.
pub struct Session {
    pub ui: Mutex<Ui>,
    pub changed: watch::Sender<()>,
}

impl AppState {
    /// Creates the world and starts ticking it every 250 ms.
    pub fn start() -> Arc<Self> {
        let (tx, ticks) = watch::channel(0);
        let state = Arc::new(Self {
            world: RwLock::new(World::new(42)),
            ticks,
            sessions: Mutex::new(HashMap::new()),
        });
        let s = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(TICK);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            loop {
                interval.tick().await;
                s.world.write().unwrap().tick(TICK.as_millis() as u64);
                tx.send_modify(|n| *n += 1);
            }
        });
        state
    }

    /// The session for `id`, created on first use.
    pub fn session(&self, id: &str) -> Arc<Session> {
        let mut sessions = self.sessions.lock().unwrap();
        sessions
            .entry(id.to_string())
            .or_insert_with(|| {
                Arc::new(Session {
                    ui: Mutex::new(Ui { live: true, ..Ui::default() }),
                    changed: watch::channel(()).0,
                })
            })
            .clone()
    }
}

/// Something the user did.
pub enum Action {
    Select(usize),
    Theme(&'static str),
    NextTheme,
    Scanlines,
    ToggleSeries(&'static str),
    ChunkHover(Option<usize>),
    ChartMove(f64),
    ChartDown(f64),
    ChartUp(f64),
    ChartLeave,
    ResetZoom,
    /// `j` / `k`: move the selection by this many transfers.
    Step(isize),
}

impl Action {
    /// Maps a keyboard shortcut to an action.
    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "j" => Self::Step(1),
            "k" => Self::Step(-1),
            "t" => Self::NextTheme,
            "s" => Self::Scanlines,
            "d" => Self::ToggleSeries("down"),
            "u" => Self::ToggleSeries("up"),
            "0" => Self::ResetZoom,
            _ => return None,
        })
    }
}

impl Session {
    /// Applies `action` and wakes this session's streams.
    pub fn apply(&self, world: &World, action: Action) {
        let mut ui = self.ui.lock().unwrap();
        let count = world.bundles.len();
        match action {
            Action::Select(i) => select(&mut ui, i.min(count - 1)),
            Action::Step(d) => {
                let i = (ui.selected as isize + d).rem_euclid(count as isize) as usize;
                select(&mut ui, i);
            }
            Action::Theme(t) => ui.theme = t,
            Action::NextTheme => {
                let i = THEMES.iter().position(|(k, _)| *k == ui.theme).unwrap_or(0);
                ui.theme = THEMES[(i + 1) % THEMES.len()].0;
            }
            Action::Scanlines => ui.scanlines = !ui.scanlines,
            Action::ToggleSeries("down") => ui.visible.down = !ui.visible.down,
            Action::ToggleSeries(_) => ui.visible.up = !ui.visible.up,
            Action::ChunkHover(cell) => ui.cell = cell,
            Action::ChartMove(x) => ui.chart_x = Some(x),
            Action::ChartDown(x) => {
                ui.chart_x = Some(x);
                ui.drag_from = Some(x);
            }
            Action::ChartUp(x) => {
                ui.chart_x = Some(x);
                if let Some(from) = ui.drag_from.take() {
                    let history: Vec<Sample> = world.bundles[ui.selected].history.iter().copied().collect();
                    let c = chart::rates(&history, ui.zoom, Layout::rates(CHART_WIDTH, CHART_HEIGHT), ui.visible);
                    if let Some(window) = chart::zoom(&c, from, x) {
                        ui.zoom = Some(window);
                    }
                }
            }
            Action::ChartLeave => {
                ui.chart_x = None;
                ui.drag_from = None;
            }
            Action::ResetZoom => ui.zoom = None,
        }
        drop(ui);
        self.changed.send_replace(());
    }
}

fn select(ui: &mut Ui, i: usize) {
    if ui.selected != i {
        ui.selected = i;
        ui.zoom = None;
        ui.cell = None;
    }
}
