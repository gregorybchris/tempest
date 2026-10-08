//! The page: the app bar, the view the address names, the hint bar, and the keyboard, theme
//! and comparison basket behind them.

use leptos::ev;
use leptos::html::Input;
use leptos::prelude::*;
use tempest_sim_api::query::{CompareQuery, VariantRef};
use tempest_ui_kit::components::*;
use tempest_ui_kit::theme::apply_look;
use tempest_ui_kit::{Look, Theme};
use wasm_bindgen::JsCast;

use crate::baselines::BaselinesPage;
use crate::compare::ComparePage;
use crate::experiments::{ExperimentPage, ExperimentsPage};
use crate::keys::{self, Command};
use crate::route::{Area, Route};
use crate::runs::{RunPage, RunsPage};

/// Where the theme and the basket are kept in this browser.
const THEME_KEY: &str = "tempest-sim-theme";
const BASKET_KEY: &str = "tempest-sim-compare";

/// What every view shares.
#[derive(Clone, Copy)]
pub struct Ctx {
    /// The view shown, as the address names it.
    pub route: RwSignal<Route>,
    /// Whether the next change of route replaces the browser's history entry.
    replace_next: StoredValue<bool>,
    /// The theme.
    pub theme: RwSignal<Theme>,
    /// The variants picked for comparing.
    pub basket: RwSignal<Vec<VariantRef>>,
    /// The search field of the view shown, which `/` focuses.
    pub search: NodeRef<Input>,
    /// Pages through runs: +1 for the next page, −1 for the previous.
    pub page_step: RwSignal<(i32, u32)>,
}

impl Ctx {
    /// Shows `route`, as a new entry in the browser's history.
    pub fn go(&self, route: Route) {
        self.route.set(route);
    }

    /// Shows `route` in place of the current entry, as filters do while they are typed.
    pub fn replace(&self, route: Route) {
        if self.route.with_untracked(|r| *r != route) {
            self.replace_next.set_value(true);
            self.route.set(route);
        }
    }

    /// The comparison of the basket's variants, if any are picked.
    pub fn basket_route(&self) -> Route {
        let variants = self.basket.get_untracked();
        Route::Compare((!variants.is_empty()).then_some(CompareQuery {
            variants,
            reference: 0,
        }))
    }
}

/// The shared context.
pub fn use_ctx() -> Ctx {
    expect_context()
}

/// A local signal that follows `from`, and hands its own changes to `to`: for a field that
/// edits part of the address.
pub fn synced<T>(from: Memo<T>, to: impl Fn(T) + 'static) -> RwSignal<T>
where
    T: Clone + PartialEq + Send + Sync + 'static,
{
    let local = RwSignal::new(from.get_untracked());
    Effect::new(move |_| {
        let value = from.get();
        if local.with_untracked(|l| *l != value) {
            local.set(value);
        }
    });
    Effect::new(move |_| {
        let value = local.get();
        if from.with_untracked(|f| *f != value) {
            to(value);
        }
    });
    local
}

/// Runs `then` a moment after the last call, so typing doesn't ask the server at every key.
pub fn debounce(then: impl FnOnce() + 'static) {
    thread_local! {
        static PENDING: std::cell::Cell<Option<TimeoutHandle>> = const { std::cell::Cell::new(None) };
    }
    PENDING.with(|pending| {
        if let Some(handle) = pending.take() {
            handle.clear();
        }
        let handle = set_timeout_with_handle(then, std::time::Duration::from_millis(180)).ok();
        pending.set(handle);
    });
}

fn storage() -> Option<web_sys::Storage> {
    window().local_storage().ok().flatten()
}

fn current_hash() -> String {
    window().location().hash().unwrap_or_default()
}

/// The whole page.
#[component]
pub fn App() -> impl IntoView {
    view! {
        <Toaster>
            <Shell />
        </Toaster>
    }
}

#[component]
fn Shell() -> impl IntoView {
    let ctx = Ctx {
        route: RwSignal::new(Route::parse(&current_hash())),
        replace_next: StoredValue::new(false),
        theme: RwSignal::new(initial_theme()),
        basket: RwSignal::new(saved_basket()),
        search: NodeRef::new(),
        page_step: RwSignal::new((0, 0)),
    };
    provide_context(ctx);
    follow_address(ctx);
    keep_look(ctx);
    keep_basket(ctx);
    keyboard(ctx);

    // The navigation shows the route's area, and opens an area's first view.
    let area = RwSignal::new(ctx.route.get_untracked().area());
    Effect::new(move |_| {
        let now = ctx.route.with(Route::area);
        if area.get_untracked() != now {
            area.set(now);
        }
    });
    Effect::new(move |_| {
        let chosen = area.get();
        if ctx.route.with_untracked(Route::area) != chosen {
            ctx.go(open(ctx, chosen));
        }
    });

    // Views are rebuilt only when the kind of view or its subject changes, not its filters,
    // so fields keep their focus while they are typed in.
    let page = Memo::new(move |_| match ctx.route.get() {
        Route::Experiments(_) => Page::Experiments,
        Route::Experiment(id) => Page::Experiment(id),
        Route::Runs(_) => Page::Runs,
        Route::Run(id) => Page::Run(id),
        Route::Compare(_) => Page::Compare,
        Route::Baselines => Page::Baselines,
    });
    let view = move || match page.get() {
        Page::Experiments => view! { <ExperimentsPage /> }.into_any(),
        Page::Experiment(id) => view! { <ExperimentPage id /> }.into_any(),
        Page::Runs => view! { <RunsPage /> }.into_any(),
        Page::Run(id) => view! { <RunPage id /> }.into_any(),
        Page::Compare => view! { <ComparePage /> }.into_any(),
        Page::Baselines => view! { <BaselinesPage /> }.into_any(),
    };
    let picked = move || ctx.basket.with(Vec::len);
    view! {
        <div class="app">
            <header class="appbar">
                <a class="appbar__home" href="#/experiments" aria-label="Experiments">
                    <Wordmark />
                </a>
                <span class="appbar__product">"sim"</span>
                <Nav items=Area::ALL.iter().map(|a| (*a, a.title())).collect() current=area />
                <div class="appbar__spacer"></div>
                <Show when=move || { picked() > 0 }>
                    <a class="basket" href=move || ctx.basket_route().hash() title="Compare the picked variants">
                        <span class="basket__count num">{picked}</span>
                        " picked"
                    </a>
                </Show>
                <button
                    class="theme-button"
                    type="button"
                    title="Next theme (T)"
                    aria-label=move || format!("Theme: {}. Switch to the next.", ctx.theme.get().name())
                    on:click=move |_| ctx.theme.update(|t| *t = t.next())
                >
                    "◐"
                </button>
            </header>
            <main class="main">{view}</main>
            <HintBar hints=keys::hints() />
        </div>
    }
}

/// The kind of view shown, and its subject.
#[derive(Clone, Copy, PartialEq)]
enum Page {
    Experiments,
    Experiment(i64),
    Runs,
    Run(i64),
    Compare,
    Baselines,
}

/// The first view of `area`.
fn open(ctx: Ctx, area: Area) -> Route {
    match area {
        Area::Experiments => Route::default(),
        Area::Runs => Route::Runs(Default::default()),
        Area::Compare => ctx.basket_route(),
        Area::Baselines => Route::Baselines,
    }
}

/// Keeps the route and the address's `#` in step, both ways.
fn follow_address(ctx: Ctx) {
    let handle = window_event_listener(ev::hashchange, move |_| {
        let route = Route::parse(&current_hash());
        if ctx.route.with_untracked(|r| *r != route) {
            ctx.route.set(route);
        }
    });
    on_cleanup(move || handle.remove());
    Effect::new(move |_| {
        let hash = ctx.route.with(Route::hash);
        let replace = ctx.replace_next.get_value();
        ctx.replace_next.set_value(false);
        if current_hash() == hash {
            return;
        }
        let location = window().location();
        let _ = if replace {
            location.replace(&hash)
        } else {
            location.set_hash(&hash)
        };
    });
}

/// The theme: `?theme=` in the address, or the one last chosen in this browser, or green.
fn initial_theme() -> Theme {
    let search = window().location().search().unwrap_or_default();
    let asked = search
        .trim_start_matches('?')
        .split('&')
        .find_map(|p| p.strip_prefix("theme="))
        .and_then(Theme::named);
    let saved = || {
        storage()?
            .get_item(THEME_KEY)
            .ok()
            .flatten()
            .and_then(|n| Theme::named(&n))
    };
    asked.or_else(saved).unwrap_or_default()
}

fn keep_look(ctx: Ctx) {
    apply_look(Signal::derive(move || Look {
        theme: ctx.theme.get(),
        ..Look::default()
    }));
    Effect::new(move |_| {
        let theme = ctx.theme.get();
        if let Some(storage) = storage() {
            let _ = storage.set_item(THEME_KEY, theme.name());
        }
    });
}

fn saved_basket() -> Vec<VariantRef> {
    storage()
        .and_then(|s| s.get_item(BASKET_KEY).ok().flatten())
        .map(|text| text.split(',').filter_map(|v| v.parse().ok()).collect())
        .unwrap_or_default()
}

/// The basket follows the comparison shown, and is kept in this browser.
fn keep_basket(ctx: Ctx) {
    Effect::new(move |_| {
        if let Route::Compare(Some(query)) = ctx.route.get()
            && ctx.basket.with_untracked(|b| *b != query.variants)
        {
            ctx.basket.set(query.variants);
        }
    });
    Effect::new(move |_| {
        let text: Vec<String> = ctx.basket.get().iter().map(|v| v.to_string()).collect();
        if let Some(storage) = storage() {
            let _ = storage.set_item(BASKET_KEY, &text.join(","));
        }
    });
}

/// The keyboard: one listener, reading the bindings table.
fn keyboard(ctx: Ctx) {
    let handle = window_event_listener(ev::keydown, move |e| {
        if e.ctrl_key() || e.meta_key() || e.alt_key() || e.default_prevented() {
            return;
        }
        let target = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok());
        let typing = target.as_ref().is_some_and(takes_keys);
        if e.key() == "Escape" {
            if typing {
                if let Some(field) = target.and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok())
                {
                    let _ = field.blur();
                }
            } else if let Some(up) = up(&ctx.route.get_untracked()) {
                ctx.go(up);
            }
            return;
        }
        if typing {
            return;
        }
        let Some(command) = keys::command(&e.key()) else {
            return;
        };
        e.prevent_default();
        match command {
            Command::Area(area) => ctx.go(open(ctx, area)),
            Command::Search => {
                if let Some(input) = ctx.search.get_untracked() {
                    let _ = input.focus();
                    input.select();
                }
            }
            Command::Theme => ctx.theme.update(|t| *t = t.next()),
            Command::NextPage => ctx.page_step.update(|s| *s = (1, s.1 + 1)),
            Command::PreviousPage => ctx.page_step.update(|s| *s = (-1, s.1 + 1)),
        }
    });
    on_cleanup(move || handle.remove());
}

/// The view `esc` goes back to.
fn up(route: &Route) -> Option<Route> {
    match route {
        Route::Experiment(_) => Some(Route::default()),
        Route::Run(_) => Some(Route::Runs(Default::default())),
        _ => None,
    }
}
