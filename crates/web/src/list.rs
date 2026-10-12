//! The Transfers area: a toolbar for finding (D-192) and the list, list first (D-131). Each row
//! answers "is it working, how far, how fast, how long".

use leptos::ev;
use leptos::html::{Div, Tbody};
use leptos::prelude::*;
use tempest_api::TransferInfo;
use tempest_api::columns::Column;
use tempest_api::finding::{SortKey, StatusFilter};
use tempest_protocol::BundleId;
use tempest_ui_kit::chart::use_width;
use tempest_ui_kit::components::*;
use tempest_ui_kit::format;
use tempest_ui_kit::theme::bool_attr;

use crate::app::Fields;
use crate::columns::{cell_class, fitting, hiding};
use crate::present;
use crate::state::{Dialog, Ui};
use crate::store::Store;
use crate::viewport::View;

/// The transfers the list shows, in order, which of them are drawn, and moving the cursor
/// through them.
#[derive(Clone, Copy)]
pub struct Order {
    /// The IDs of the transfers that pass the filters, in the list's order.
    pub ids: Memo<Vec<BundleId>>,
    ui: Ui,
    /// The list's body, once drawn.
    body: NodeRef<Tbody>,
    /// Where the body is on the screen.
    view: RwSignal<View>,
}

impl Order {
    /// The order of `store`'s transfers under `ui`'s filters.
    pub fn new(store: Store, ui: Ui) -> Self {
        let ids = Memo::new(move |_| {
            store.model.with(|m| {
                ui.finding
                    .with(|f| f.apply(m.transfers()).iter().map(|t| t.id).collect())
            })
        });
        Self {
            ids,
            ui,
            body: NodeRef::new(),
            view: RwSignal::new(View::default()),
        }
    }

    /// Moves the cursor as `to` says, and scrolls its row into view.
    pub fn go(self, to: Move) {
        let ids = self.ids.get_untracked();
        let current = self
            .ui
            .cursor
            .get_untracked()
            .and_then(|id| ids.iter().position(|i| *i == id));
        if let Some(next) = to.from(current, ids.len()) {
            self.ui.cursor.set(Some(ids[next]));
            self.reveal(next);
        }
    }

    /// Reads where the list's body is, after a scroll, a resize or a change of rows.
    fn measure(self) {
        let Some(body) = self.body.get_untracked() else {
            return;
        };
        let height = window()
            .inner_height()
            .ok()
            .and_then(|h| h.as_f64())
            .unwrap_or_default();
        let row = body
            .query_selector("tr[aria-rowindex]")
            .ok()
            .flatten()
            .map(|row| row.get_bounding_client_rect().height())
            .filter(|&h| h > 0.0);
        let view = View {
            top: body.get_bounding_client_rect().top(),
            height,
            row: row.unwrap_or_else(|| self.view.with_untracked(|v| v.row)),
        };
        if self.view.with_untracked(|v| *v != view) {
            self.view.set(view);
        }
    }

    /// Scrolls the page so that row `index` shows clear of the app bar and the key hints.
    fn reveal(self, index: usize) {
        // The first row brings the toolbar and the headings back too.
        if index == 0 {
            window().scroll_to_with_x_and_y(0.0, 0.0);
            self.measure();
            return;
        }
        self.measure();
        let edge = |selector: &str, bottom: bool| {
            let element = document().query_selector(selector).ok().flatten()?;
            let rect = element.get_bounding_client_rect();
            (rect.height() > 0.0).then(|| if bottom { rect.bottom() } else { rect.top() })
        };
        let view = self.view.get_untracked();
        let top = edge(".appbar", true).unwrap_or(0.0);
        // The hint bar, or on phones the navigation along the bottom.
        let bottom = [edge(".hints", false), edge(".nav", false)]
            .into_iter()
            .flatten()
            .filter(|&y| y > top + view.row)
            .fold(view.height, f64::min);
        let by = view.scroll_to(index, top, bottom);
        if by != 0.0 {
            window().scroll_by_with_x_and_y(0.0, by);
            self.measure();
        }
    }
}

/// A move of the list's cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    /// One row down.
    Down,
    /// One row up.
    Up,
    /// To the first row.
    First,
    /// To the last row.
    Last,
}

impl Move {
    /// The row the cursor moves to from `current` in a list of `len` rows. With no cursor yet,
    /// moving down starts at the top and moving up at the bottom.
    pub fn from(self, current: Option<usize>, len: usize) -> Option<usize> {
        let last = len.checked_sub(1)?;
        Some(match (self, current) {
            (Move::First, _) | (Move::Down, None) => 0,
            (Move::Last, _) | (Move::Up, None) => last,
            (Move::Down, Some(i)) => (i + 1).min(last),
            (Move::Up, Some(i)) => i.saturating_sub(1).min(last),
        })
    }
}

/// The Transfers area.
#[component]
pub fn Transfers() -> impl IntoView {
    view! {
        <div class="transfers-area">
            <Toolbar />
            <List />
        </div>
    }
}

#[component]
fn Toolbar() -> impl IntoView {
    let store = expect_context::<Store>();
    let ui = expect_context::<Ui>();
    let fields = expect_context::<Fields>();
    let order = expect_context::<Order>();
    // The search box starts with the filter, which outlives the area.
    let query = RwSignal::new(ui.finding.with_untracked(|f| f.text.clone()));
    // The search box types into the finding.
    Effect::new(move |_| {
        let text = query.get();
        ui.finding.update(|f| f.text = text);
    });
    Effect::new(move |_| {
        let text = ui.finding.with(|f| f.text.clone());
        if query.get_untracked() != text {
            query.set(text);
        }
    });
    let counts = move || {
        store.model.with(|m| {
            StatusFilter::ALL.map(|status| {
                let count = m
                    .transfers()
                    .iter()
                    .filter(|t| StatusFilter::of(&t.status) == status)
                    .count();
                (status, count)
            })
        })
    };
    let total = move || store.model.with(|m| m.transfers().len());
    let on_status = move |id: String| {
        ui.finding.update(|f| f.status = StatusFilter::from_id(&id));
    };
    let on_sort = move |id: String| {
        ui.finding.update(|f| {
            f.sort.key = SortKey::from_id(&id).unwrap_or(SortKey::Queue);
            f.sort.reversed = false;
        });
    };
    let statuses = Signal::derive(move || {
        let mut options = vec![(String::new(), format!("all statuses ({})", total()))];
        options.extend(counts().into_iter().map(|(status, count)| {
            let name = format!("{} ({count})", status.title().to_lowercase());
            (status.id().to_owned(), name)
        }));
        options
    });
    let sorts: Vec<(String, String)> = SortKey::ALL
        .iter()
        .map(|key| (key.id().to_owned(), format!("by {}", key.title())))
        .collect();
    let shown = move || {
        let (shown, total) = (order.ids.with(Vec::len), total());
        match ui.finding.with(tempest_api::finding::Finding::filtering) {
            true => format!("{shown} of {total}"),
            false => format!("{total} transfers"),
        }
    };
    view! {
        <div class="toolbar">
            <SearchBox value=query placeholder="Filter by name or #tag" node_ref=fields.search />
            <Select
                label="Status"
                node_ref=fields.status
                options=statuses
                value=Signal::derive(move || ui.finding.with(|f| f.status.map_or("", StatusFilter::id).to_owned()))
                on_change=on_status
            />
            <Select
                label="Sort by"
                node_ref=fields.sort
                options=sorts
                value=Signal::derive(move || ui.finding.with(|f| f.sort.key.id().to_owned()))
                on_change=on_sort
            />
            <button
                class="btn btn--ghost btn--small"
                type="button"
                title="Reverse the order (S)"
                on:click=move |_| ui.finding.update(|f| f.sort.reversed = !f.sort.reversed)
            >
                {move || ui.finding.with(|f| if f.sort.ascending() { "▲ ascending" } else { "▼ descending" })}
            </button>
            <span class="toolbar__count dim">{shown}</span>
            <div class="toolbar__spacer"></div>
            <Button small=true kind=ButtonKind::Ghost title="Saved views (v)" on_click=move || ui.dialog.set(Some(Dialog::Views))>
                "views"
            </Button>
            <Button small=true kind=ButtonKind::Ghost title="Choose columns (|)" on_click=move || ui.dialog.set(Some(Dialog::Columns))>
                "columns"
            </Button>
            <Button kind=ButtonKind::Primary title="Add a transfer from a link (a)" on_click=move || ui.dialog.set(Some(Dialog::Add))>
                "+ Add"
            </Button>
        </div>
    }
}

/// The sort key a column's header sorts by, if any.
fn sort_key(column: Column) -> Option<SortKey> {
    Some(match column {
        Column::Name => SortKey::Name,
        Column::Size => SortKey::Size,
        Column::Progress => SortKey::Progress,
        Column::Down => SortKey::Down,
        Column::Up => SortKey::Up,
        Column::Eta => SortKey::TimeLeft,
        Column::Status => SortKey::Status,
        Column::Peers => SortKey::Peers,
        Column::Ratio => SortKey::Ratio,
        Column::Added => SortKey::Added,
        Column::Category | Column::Tags => return None,
    })
}

#[component]
fn List() -> impl IntoView {
    let store = expect_context::<Store>();
    let ui = expect_context::<Ui>();
    let order = expect_context::<Order>();
    let frame = NodeRef::<Div>::new();
    let width = use_width(frame);
    let class = Memo::new(move |_| ui.columns.with(|c| hiding(&fitting(width.get(), c))));
    let empty = move || {
        if !store.loaded.get() {
            return Some(
                view! { <Empty title="Connecting" text="Asking the daemon for its transfers…" /> }
                    .into_any(),
            );
        }
        if !order.ids.with(Vec::is_empty) {
            return None;
        }
        Some(match store.model.with(|m| m.transfers().is_empty()) {
            true => view! {
                <Empty title="No transfers yet" text="Add a tempest: link to download a bundle, or share a file from the Share tab.">
                    <Button kind=ButtonKind::Primary on_click=move || ui.dialog.set(Some(Dialog::Add))>"+ Add a link"</Button>
                </Empty>
            }
            .into_any(),
            false => view! {
                <Empty title="No matches" text="No transfers match the filters.">
                    <Button kind=ButtonKind::Ghost on_click=move || ui.finding.update(|f| { f.text.clear(); f.status = None; })>
                        "Clear filters"
                    </Button>
                </Empty>
            }
            .into_any(),
        })
    };

    // Only the rows in view are drawn (`viewport`); the spacers stand in for the rest.
    let count = Memo::new(move |_| order.ids.with(Vec::len));
    let drawn = Memo::new(move |_| order.view.with(|v| v.rows(count.get())));
    let rows = move || {
        let range = drawn.get();
        order.ids.with(|ids| {
            ids[range.clone()]
                .iter()
                .zip(range)
                .map(|(id, i)| (i, *id))
                .collect::<Vec<_>>()
        })
    };
    let spacer = move |rows: Signal<usize>| {
        let height = move || format!("{}px", rows.get() as f64 * order.view.with(|v| v.row));
        view! {
            <tr class="list__spacer" aria-hidden="true" style:height=height>
                <td colspan=Column::ORDER.len()></td>
            </tr>
        }
    };
    let above = Signal::derive(move || drawn.with(|r| r.start));
    let below = Signal::derive(move || count.get() - drawn.with(|r| r.end));
    let measure = move || order.measure();
    let on_scroll = window_event_listener(ev::scroll, move |_| measure());
    let on_resize = window_event_listener(ev::resize, move |_| measure());
    on_cleanup(move || {
        on_scroll.remove();
        on_resize.remove();
    });
    // Rows coming and going, or the toolbar wrapping, move the body without a scroll.
    Effect::new(move |_| {
        count.track();
        width.track();
        request_animation_frame(measure);
    });

    // One selector, so moving the cursor touches two rows rather than every row.
    let cursor = Selector::new(move || ui.cursor.get());
    view! {
        <div class="list" node_ref=frame>
            <table
                class=class
                aria-label="Transfers"
                aria-rowcount=move || count.get() + 1
            >
                <thead>
                    <tr aria-rowindex="1">{Column::ORDER.map(|column| view! { <Header column /> })}</tr>
                </thead>
                <tbody node_ref=order.body>
                    {spacer(above)}
                    <For each=rows key=|row| *row let:row>
                        <Row id=row.1 index=row.0 cursor=cursor.clone() />
                    </For>
                    {spacer(below)}
                </tbody>
            </table>
            {empty}
        </div>
    }
}

#[component]
fn Header(column: Column) -> impl IntoView {
    let ui = expect_context::<Ui>();
    let class = cell_class(column).replace(" table__clip", "");
    let Some(key) = sort_key(column) else {
        return view! { <th class=class>{column.title()}</th> }.into_any();
    };
    let direction = move || {
        ui.finding.with(|f| {
            (f.sort.key == key).then_some(if f.sort.ascending() {
                "ascending"
            } else {
                "descending"
            })
        })
    };
    // The same column reverses; another starts in its natural direction (D-192).
    let click = move |_| {
        ui.finding.update(|f| {
            if f.sort.key == key {
                f.sort.reversed = !f.sort.reversed;
            } else {
                f.sort.key = key;
                f.sort.reversed = false;
            }
        });
    };
    view! {
        <th class=class aria-sort=direction>
            <button class="sort" type="button" data-sort=direction on:click=click>
                {column.title()}
            </button>
        </th>
    }
    .into_any()
}

#[component]
fn Row(id: BundleId, index: usize, cursor: Selector<Option<BundleId>>) -> impl IntoView {
    let store = expect_context::<Store>();
    let ui = expect_context::<Ui>();
    let Some(row) = store.row(id) else {
        return ().into_any();
    };
    let text = move |f: fn(&TransferInfo) -> String| move || row.with(f);
    let tone = Signal::derive(move || row.with(|t| present::tone(&t.status)));
    let cell = move |column: Column| cell_class(column);
    view! {
        <tr
            aria-rowindex=index + 2
            aria-selected=move || bool_attr(cursor.selected(&Some(id)))
            on:click=move |_| ui.open(id)
        >
            <td class=cell(Column::Name) title=text(|t| t.name.clone())>
                <span class="row__name">{text(|t| t.name.clone())}</span>
            </td>
            <td class=cell(Column::Size)>{text(|t| present::size(t.size))}</td>
            <td class=cell(Column::Progress)>
                <ProgressBar fraction=Signal::derive(move || row.with(|t| t.progress)) tone />
            </td>
            <td class=cell(Column::Down)>{text(|t| present::rate(t.download_rate))}</td>
            <td class=cell(Column::Up)>{text(|t| present::rate(t.upload_rate))}</td>
            <td class=cell(Column::Eta)>{text(|t| present::eta(t.eta))}</td>
            <td class=cell(Column::Status) title=text(|t| tempest_api::explain::explain(t).text)>
                <Badge tone label=Signal::derive(move || row.with(|t| present::label(&t.status).to_owned())) />
            </td>
            <td class=cell(Column::Peers)>{text(present::peers)}</td>
            <td class=cell(Column::Ratio)>{text(|t| present::ratio(t.ratio))}</td>
            <td class=cell(Column::Category)>{text(|t| t.category.clone().unwrap_or_else(|| "–".to_owned()))}</td>
            <td class=cell(Column::Tags)>
                {move || row.with(|t| t.tags.iter().map(|tag| view! { <Chip label=tag.clone() /> }).collect_view())}
            </td>
            <td class=cell(Column::Added) title=move || row.with(|t| format::datetime(t.added))>{text(|t| present::date(t.added))}</td>
        </tr>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cursor_stops_at_either_end() {
        assert_eq!(Move::Down.from(None, 3), Some(0));
        assert_eq!(Move::Up.from(None, 3), Some(2));
        assert_eq!(Move::Down.from(Some(2), 3), Some(2));
        assert_eq!(Move::Up.from(Some(0), 3), Some(0));
        assert_eq!(Move::Last.from(Some(0), 3), Some(2));
        assert_eq!(Move::First.from(None, 3), Some(0));
        // A cursor past the end, after rows went, comes back in.
        assert_eq!(Move::Up.from(Some(7), 3), Some(2));
        assert_eq!(Move::Down.from(None, 0), None);
    }
}
