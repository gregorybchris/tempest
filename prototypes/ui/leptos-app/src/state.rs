//! App state: the simulated client plus UI state, as signals.

use leptos::prelude::*;
use proto_core::chart::Visible;
use proto_core::{format, Bundle, BundleState, World};

pub const THEMES: [&str; 4] = ["green", "amber", "vector", "beige"];

/// Every signal the page reads. `Copy`, so closures can capture it freely.
#[derive(Clone, Copy)]
pub struct Ui {
    pub world: RwSignal<World>,
    pub selected: RwSignal<usize>,
    pub theme: RwSignal<&'static str>,
    pub scanlines: RwSignal<bool>,
    pub visible: RwSignal<Visible>,
    pub zoom: RwSignal<Option<(u64, u64)>>,
}

impl Ui {
    pub fn new(world: World) -> Self {
        Self {
            world: RwSignal::new(world),
            selected: RwSignal::new(0),
            theme: RwSignal::new(THEMES[0]),
            scanlines: RwSignal::new(false),
            visible: RwSignal::new(Visible::default()),
            zoom: RwSignal::new(None),
        }
    }

    /// Reads the selected bundle, tracking both the world and the selection.
    pub fn bundle<T>(self, f: impl FnOnce(&Bundle) -> T) -> T {
        let i = self.selected.get();
        self.world.with(|w| f(&w.bundles[i]))
    }

    /// Selects a bundle. A new bundle starts unzoomed.
    pub fn select(self, i: usize) {
        if self.selected.get_untracked() != i {
            self.selected.set(i);
            self.zoom.set(None);
        }
    }

    pub fn step_selection(self, delta: isize) {
        let n = self.world.with_untracked(|w| w.bundles.len()) as isize;
        let i = (self.selected.get_untracked() as isize + delta).rem_euclid(n);
        self.select(i as usize);
    }
}

pub fn use_ui() -> Ui {
    expect_context()
}

pub fn bool_attr(b: bool) -> &'static str {
    if b {
        "true"
    } else {
        "false"
    }
}

pub fn state_class(s: BundleState) -> &'static str {
    match s {
        BundleState::Downloading => "state--downloading",
        BundleState::Sharing => "state--sharing",
        BundleState::WaitingForPeers => "state--waiting",
    }
}

pub fn transfer_rate(b: &Bundle) -> String {
    match b.state {
        BundleState::Downloading => format!("↓ {}", format::rate(b.down_bps)),
        BundleState::Sharing => format!("↑ {}", format::rate(b.up_bps)),
        BundleState::WaitingForPeers => "0 sources".to_string(),
    }
}
