//! The page's own state: which area and transfer show, what is open over them, and how the
//! list is filtered. Shared as context, so components read what they need.

use leptos::prelude::*;
use tempest_api::columns::Column;
use tempest_api::finding::Finding;
use tempest_api::ui::SavedView;
use tempest_protocol::BundleId;
use tempest_ui_kit::Theme;

use crate::keys::Scope;

/// The app's areas, in the navigation's order (D-136).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Area {
    /// The transfers list, home.
    Transfers,
    /// Making a bundle to share.
    Share,
    /// Trackers and what they list.
    Catalog,
    /// Limits, queue, categories and the look.
    Settings,
}

impl Area {
    /// Every area, in order.
    pub const ALL: [Area; 4] = [Area::Transfers, Area::Share, Area::Catalog, Area::Settings];

    /// Its name in the navigation.
    pub fn title(self) -> &'static str {
        match self {
            Area::Transfers => "Transfers",
            Area::Share => "Share",
            Area::Catalog => "Catalog",
            Area::Settings => "Settings",
        }
    }

    /// The area `step` places on, wrapping around.
    pub fn step(self, step: isize) -> Area {
        let i = Area::ALL.iter().position(|&a| a == self).unwrap_or(0) as isize;
        Area::ALL[(i + step).rem_euclid(Area::ALL.len() as isize) as usize]
    }
}

/// The detail's tabs (D-137).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    /// What it is doing, in a sentence, and its figures.
    Summary,
    /// Its files.
    Files,
    /// Its connections.
    Peers,
    /// Its session's decision log.
    Decisions,
}

impl Tab {
    /// Every tab, in order.
    pub const ALL: [Tab; 4] = [Tab::Summary, Tab::Files, Tab::Peers, Tab::Decisions];

    /// Its name.
    pub fn title(self) -> &'static str {
        match self {
            Tab::Summary => "Summary",
            Tab::Files => "Files",
            Tab::Peers => "Peers",
            Tab::Decisions => "Decisions",
        }
    }

    /// The tab `step` places on, wrapping around.
    pub fn step(self, step: isize) -> Tab {
        let i = Tab::ALL.iter().position(|&t| t == self).unwrap_or(0) as isize;
        Tab::ALL[(i + step).rem_euclid(Tab::ALL.len() as isize) as usize]
    }
}

/// A view one level into the detail, entered explicitly (D-137).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Deeper {
    /// Every chunk, as cells.
    ChunkMap,
    /// Rates over the last ten minutes.
    Rates,
}

/// A dialog over everything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialog {
    /// Add a download from a link.
    Add,
    /// Remove a transfer.
    Remove(BundleId),
    /// Rename a transfer.
    Rename(BundleId),
    /// Edit a transfer's tags.
    Tags(BundleId),
    /// Set a transfer's category.
    Category(BundleId),
    /// Move a transfer's files.
    Move(BundleId),
    /// A transfer's own speed and sharing limits.
    Limits(BundleId),
    /// Choose the list's columns.
    Columns,
    /// The saved views, and saving the current one.
    Views,
    /// The keys.
    Help,
    /// What the network defaults share, on the first run (D-227).
    Network,
}

/// The page's state. It is `Copy`; every copy is the same state.
#[derive(Clone, Copy)]
pub struct Ui {
    /// The area shown.
    pub area: RwSignal<Area>,
    /// The transfer the keyboard acts on in the list.
    pub cursor: RwSignal<Option<BundleId>>,
    /// The transfer whose detail is open.
    pub open: RwSignal<Option<BundleId>>,
    /// The detail's tab.
    pub tab: RwSignal<Tab>,
    /// The view one level into the detail, if any.
    pub deeper: RwSignal<Option<Deeper>>,
    /// The dialog open, if any.
    pub dialog: RwSignal<Option<Dialog>>,
    /// The list's filters and order.
    pub finding: RwSignal<Finding>,
    /// The palette.
    pub theme: RwSignal<Theme>,
    /// Scanlines over the page.
    pub scanlines: RwSignal<bool>,
    /// No glow, scanlines or animation.
    pub clean: RwSignal<bool>,
    /// The optional columns the user chose (D-197).
    pub columns: RwSignal<Vec<Column>>,
    /// The saved views (D-197).
    pub views: RwSignal<Vec<SavedView>>,
    /// Whether this browser shows notifications.
    pub notify: RwSignal<bool>,
    /// Whether a text field has the keys.
    pub typing: RwSignal<bool>,
    /// The share the keyboard acts on in the Share area.
    pub share: RwSignal<Option<BundleId>>,
    /// The row the keyboard acts on in the Catalog and Settings areas, by its ID.
    pub row: RwSignal<Option<String>>,
}

impl Ui {
    /// The state of a page that just opened.
    pub fn new() -> Self {
        Self {
            area: RwSignal::new(Area::Transfers),
            cursor: RwSignal::new(None),
            open: RwSignal::new(None),
            tab: RwSignal::new(Tab::Summary),
            deeper: RwSignal::new(None),
            dialog: RwSignal::new(None),
            finding: RwSignal::new(Finding::default()),
            theme: RwSignal::new(Theme::default()),
            scanlines: RwSignal::new(false),
            clean: RwSignal::new(false),
            columns: RwSignal::new(Column::DEFAULT.to_vec()),
            views: RwSignal::new(Vec::new()),
            notify: RwSignal::new(false),
            typing: RwSignal::new(false),
            share: RwSignal::new(None),
            row: RwSignal::new(None),
        }
    }

    /// Where the keyboard's focus is, tracked, for hints.
    pub fn scope(self) -> Scope {
        if self.typing.get() {
            return Scope::Typing;
        }
        scope(self.area.get(), self.open.get(), self.deeper.get())
    }

    /// Where the keyboard's focus is now, untracked, for event handlers, which ignore keys
    /// while typing.
    pub fn scope_now(self) -> Scope {
        scope(
            self.area.get_untracked(),
            self.open.get_untracked(),
            self.deeper.get_untracked(),
        )
    }

    /// Opens transfer `id`'s detail at its summary.
    pub fn open(self, id: BundleId) {
        self.cursor.set(Some(id));
        self.open.set(Some(id));
        self.tab.set(Tab::Summary);
        self.deeper.set(None);
    }

    /// Shows transfer `id`'s detail in the Transfers area, clearing filters that hide it.
    pub fn show(self, id: BundleId) {
        self.finding.update(|f| {
            f.text.clear();
            f.status = None;
        });
        self.area.set(Area::Transfers);
        self.open(id);
    }

    /// Goes back one level: out of a deeper view, or out of the detail.
    pub fn back(self) {
        if self.deeper.get_untracked().is_some() {
            self.deeper.set(None);
        } else {
            self.open.set(None);
        }
    }
}

fn scope(area: Area, open: Option<BundleId>, deeper: Option<Deeper>) -> Scope {
    match (area, open, deeper) {
        (Area::Transfers, None, _) => Scope::List,
        (Area::Transfers, Some(_), None) => Scope::Detail,
        (Area::Transfers, Some(_), Some(_)) => Scope::Deeper,
        (Area::Share, ..) => Scope::Share,
        (Area::Catalog | Area::Settings, ..) => Scope::Rows,
    }
}

impl Default for Ui {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_wrap_around() {
        assert_eq!(Area::Transfers.step(-1), Area::Settings);
        assert_eq!(Area::Settings.step(1), Area::Transfers);
        assert_eq!(Tab::Summary.step(1), Tab::Files);
        assert_eq!(Tab::Summary.step(-1), Tab::Decisions);
    }

    #[test]
    fn each_area_has_its_scope() {
        let id = tempest_protocol::Hash::new([1; 32]);
        assert_eq!(scope(Area::Transfers, None, None), Scope::List);
        assert_eq!(scope(Area::Transfers, Some(id), None), Scope::Detail);
        assert_eq!(
            scope(Area::Transfers, Some(id), Some(Deeper::Rates)),
            Scope::Deeper
        );
        // A detail left open behind another area doesn't take its keys.
        assert_eq!(scope(Area::Share, Some(id), None), Scope::Share);
        assert_eq!(scope(Area::Catalog, None, None), Scope::Rows);
        assert_eq!(scope(Area::Settings, None, None), Scope::Rows);
    }
}
