//! The page: the app bar, the area shown, the detail drawer, the dialogs and the hint bar, and
//! the keyboard and connections behind them.

use std::time::Duration;

use leptos::ev;
use leptos::html::{Input, Select};
use leptos::prelude::*;
use leptos::task::spawn_local;
use send_wrapper::SendWrapper;
use serde_json::json;
use tempest_api::prefs;
use tempest_protocol::BundleId;
use tempest_ui_kit::components::*;
use tempest_ui_kit::theme::{ThemePicker, apply_look};
use tempest_ui_kit::{Look, Theme};
use wasm_bindgen::JsCast;

use crate::controls::Controls;
use crate::detail::Detail;
use crate::dialogs::Dialogs;
use crate::keys::{self, Command, Scope};
use crate::list::{Move, Order, Transfers};
use crate::state::{Area, Deeper, Dialog, Ui};
use crate::store::Store;
use crate::stream::{Follower, Link};
use crate::{api, present};

/// How many seconds of rates the app bar's sparklines show.
const SPARK_SECONDS: usize = 60;

/// The fields that keys focus.
#[derive(Clone, Copy)]
pub struct Fields {
    /// The list's filter.
    pub search: NodeRef<Input>,
    /// The status filter.
    pub status: NodeRef<Select>,
    /// The sort.
    pub sort: NodeRef<Select>,
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
    let store = Store::new();
    let ui = Ui::new();
    let controls = Controls::new(use_toasts());
    let fields = Fields {
        search: NodeRef::new(),
        status: NodeRef::new(),
        sort: NodeRef::new(),
    };
    let order = Order::new(store, ui);
    provide_context(store);
    provide_context(ui);
    provide_context(controls);
    provide_context(fields);
    provide_context(order);

    // Registered before the drawer's and dialogs' own, so `esc` goes back one level at a time.
    keyboard(store, ui, controls, fields, order);
    follow(store);
    look(store, ui, controls);

    let area = move || match ui.area.get() {
        Area::Transfers => view! { <Transfers /> }.into_any(),
        Area::Settings => view! { <Settings /> }.into_any(),
        other => view! { <Later area=other /> }.into_any(),
    };
    let hints = Signal::derive(move || {
        keys::hints(ui.scope())
            .into_iter()
            .map(|(keys, label)| Hint { keys, label })
            .collect::<Vec<_>>()
    });
    view! {
        <div class="app">
            <header class="appbar">
                <Wordmark />
                <Nav items=Area::ALL.iter().map(|a| (*a, a.title())).collect() current=ui.area />
                <div class="appbar__spacer"></div>
                <Meters />
                <button
                    class="theme-button"
                    type="button"
                    title="Next theme (T)"
                    aria-label=move || format!("Theme: {}. Switch to the next.", ui.theme.get().name())
                    on:click=move |_| ui.theme.update(|t| *t = t.next())
                >
                    "◐"
                </button>
            </header>
            <LinkBanner />
            <main class="main">{area}</main>
            <HintBar hints />
        </div>
        <Detail />
        <Dialogs />
    }
}

/// Follows the engine's events into the store, and samples rates once a second.
fn follow(store: Store) {
    let follower = Follower::start(
        move |event| store.push(event),
        move |link| store.link.set(link),
    );
    provide_context(StoredValue::new(SendWrapper::new(follower)));
    if let Ok(sampling) = set_interval_with_handle(move || store.sample(), Duration::from_secs(1)) {
        on_cleanup(move || sampling.clear());
    }
}

/// Tries to reach the engine again now.
fn retry_now() {
    if let Some(follower) = use_context::<StoredValue<SendWrapper<Follower>>>() {
        follower.with_value(|f| f.retry_now());
    }
}

/// Keeps the theme and columns as the engine's `ui` settings say (D-195), and saves the theme
/// when the user changes it. Like the TUI, the page takes the theme once, when it opens.
fn look(store: Store, ui: Ui, controls: Controls) {
    apply_look(Signal::derive(move || Look {
        theme: ui.theme.get(),
        ..Look::default()
    }));
    // The theme the engine has, once known.
    let saved = StoredValue::new(None::<Theme>);
    let loading = StoredValue::new(false);
    // Loads the settings once the engine answers.
    Effect::new(move |_| {
        if store.link.get() != Link::Live || saved.get_value().is_some() || loading.get_value() {
            return;
        }
        loading.set_value(true);
        spawn_local(async move {
            if let Ok(settings) = api::ui_settings().await {
                let theme = Theme::named(&settings.theme).unwrap_or_default();
                saved.set_value(Some(theme));
                ui.theme.set(theme);
                ui.columns.set(prefs::columns(&settings));
            }
            loading.set_value(false);
        });
    });
    Effect::new(move |_| {
        let theme = ui.theme.get();
        let Some(known) = saved.get_value() else {
            return;
        };
        if known == theme {
            return;
        }
        saved.set_value(Some(theme));
        spawn_local(async move {
            if let Err(error) = api::change_setting(prefs::THEME, json!(theme.name())).await {
                controls.say(Tone::Bad, format!("The theme wasn't saved. {error}"));
            }
        });
    });
}

/// The keyboard (D-191): one listener, reading the bindings table.
fn keyboard(store: Store, ui: Ui, controls: Controls, fields: Fields, order: Order) {
    let handle = window_event_listener(ev::keydown, move |e| {
        if e.ctrl_key() || e.meta_key() || e.alt_key() {
            return;
        }
        let key = e.key();
        let typing = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .is_some_and(|el| matches!(el.tag_name().as_str(), "INPUT" | "SELECT" | "TEXTAREA"));
        if key == "Escape" {
            if escape(ui, typing, &e) {
                e.prevent_default();
            }
            return;
        }
        if typing || ui.dialog.get_untracked().is_some() {
            return;
        }
        let Some(command) = keys::command(&key, ui.scope_now()) else {
            return;
        };
        e.prevent_default();
        act(command, store, ui, controls, fields, order);
    });
    on_cleanup(move || handle.remove());
}

/// Handles `esc`: closes a dialog, leaves a field, goes back a level, or clears the filters.
/// Returns whether it did anything.
fn escape(ui: Ui, typing: bool, e: &web_sys::KeyboardEvent) -> bool {
    if ui.dialog.get_untracked().is_some() {
        ui.dialog.set(None);
        return true;
    }
    if typing {
        if let Some(field) = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = field.blur();
        }
        return true;
    }
    match ui.scope_now() {
        Scope::Detail | Scope::Deeper => {
            ui.back();
            true
        }
        Scope::List if ui.finding.with_untracked(|f| f.filtering()) => {
            ui.finding.update(|f| {
                f.text.clear();
                f.status = None;
            });
            true
        }
        _ => false,
    }
}

fn act(command: Command, store: Store, ui: Ui, controls: Controls, fields: Fields, order: Order) {
    // The transfer a command acts on: the open one, or the one under the cursor.
    let target = || {
        let id = ui.open.get_untracked().or(ui.cursor.get_untracked())?;
        store.model.with_untracked(|m| m.transfer(&id).cloned())
    };
    match command {
        Command::Area(i) => ui.area.set(Area::ALL[i]),
        Command::NextArea => ui.area.update(|a| *a = a.step(1)),
        Command::PreviousArea => ui.area.update(|a| *a = a.step(-1)),
        Command::Theme => ui.theme.update(|t| *t = t.next()),
        Command::Help => ui.dialog.set(Some(Dialog::Help)),
        Command::Down => order.go(Move::Down),
        Command::Up => order.go(Move::Up),
        Command::First => order.go(Move::First),
        Command::Last => order.go(Move::Last),
        Command::Open => {
            if let Some(id) = ui.cursor.get_untracked() {
                ui.open(id);
            }
        }
        Command::Add => ui.dialog.set(Some(Dialog::Add)),
        Command::Filter => focus(fields.search.get_untracked().map(Into::into)),
        Command::StatusFilter => focus(fields.status.get_untracked().map(Into::into)),
        Command::Sort => focus(fields.sort.get_untracked().map(Into::into)),
        Command::Reverse => ui.finding.update(|f| f.sort.reversed = !f.sort.reversed),
        Command::NextTab => ui.tab.update(|t| *t = t.step(1)),
        Command::PreviousTab => ui.tab.update(|t| *t = t.step(-1)),
        Command::ChunkMap => ui.deeper.set(Some(Deeper::ChunkMap)),
        Command::Rates => ui.deeper.set(Some(Deeper::Rates)),
        Command::StartStop
        | Command::ForceStart
        | Command::Remove
        | Command::Rename
        | Command::Recheck
        | Command::CopyLink => {
            let Some(transfer) = target() else {
                controls.say(Tone::Dim, "Choose a transfer first: j and k move.");
                return;
            };
            match command {
                Command::StartStop => controls.start_stop(&transfer),
                Command::ForceStart => controls.force_start(&transfer),
                Command::Recheck => controls.recheck(&transfer),
                Command::CopyLink => controls.copy_link(&transfer),
                Command::Remove => ui.dialog.set(Some(Dialog::Remove(transfer.id))),
                _ => ui.dialog.set(Some(Dialog::Rename(transfer.id))),
            }
        }
    }
}

fn focus(element: Option<web_sys::HtmlElement>) {
    if let Some(element) = element {
        let _ = element.focus();
    }
}

/// The DOM ID of transfer `id`'s row.
pub fn row_id(id: &BundleId) -> String {
    format!("row-{id}")
}

/// The app bar's ↓ and ↑ meters, over every transfer.
#[component]
fn Meters() -> impl IntoView {
    let store = expect_context::<Store>();
    let rates = move || store.model.with(|m| m.total_rates());
    let history = move |upload: bool| {
        Signal::derive(move || {
            store
                .model
                .with(|m| present::recent(m.total_history(), SPARK_SECONDS, upload))
        })
    };
    view! {
        <div class="meters">
            <Meter
                label="↓ download"
                tone=Tone::Down
                value=Signal::derive(move || present::rate(rates().0))
                history=history(false)
            />
            <Meter
                label="↑ upload"
                tone=Tone::Up
                value=Signal::derive(move || present::rate(rates().1))
                history=history(true)
            />
        </div>
    }
}

/// Says when the engine can't be reached, and offers to try again now.
#[component]
fn LinkBanner() -> impl IntoView {
    let store = expect_context::<Store>();
    move || match store.link.get() {
        Link::Lost { retry_in } => view! {
            <div class="banner tone--bad" role="alert">
                <span>
                    "Can't reach the engine. "
                    {format!("Trying again in {} s.", retry_in.as_secs().max(1))}
                    " Is it still running?"
                </span>
                <Button small=true on_click=move || retry_now()>"Retry now"</Button>
            </div>
        }
        .into_any(),
        Link::Connecting if store.loaded.get_untracked() => {
            view! { <div class="banner tone--warn">"Reconnecting to the engine…"</div> }.into_any()
        }
        _ => ().into_any(),
    }
}

/// The Settings area: the look, for now.
#[component]
fn Settings() -> impl IntoView {
    let ui = expect_context::<Ui>();
    view! {
        <div class="area">
            <Panel title="look">
                <div class="area__stack">
                    <p class="dim">"Every Tempest client on this engine shares the theme."</p>
                    <ThemePicker theme=ui.theme />
                </div>
            </Panel>
            <Empty
                title="More settings are on the way"
                text="Limits, the queue and categories are in the TUI and the config file for now."
            />
        </div>
    }
}

/// An area the web UI doesn't have yet.
#[component]
fn Later(area: Area) -> impl IntoView {
    let text = match area {
        Area::Share => "Share files and folders from the TUI or with `tempest share` for now.",
        _ => "Browse trackers from the TUI for now.",
    };
    view! {
        <div class="area">
            <Empty title=area.title() text=text.to_owned() />
        </div>
    }
}
