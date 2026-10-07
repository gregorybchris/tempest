//! The page: the app bar, the area shown, the detail drawer, the dialogs and the hint bar, and
//! the keyboard and connections behind them.

use std::time::Duration;

use leptos::ev;
use leptos::html::{Input, Select};
use leptos::prelude::*;
use send_wrapper::SendWrapper;
use serde_json::json;
use tempest_api::prefs;
use tempest_api::requests::QueueStep;
use tempest_protocol::BundleId;
use tempest_ui_kit::components::*;
use tempest_ui_kit::theme::apply_look;
use tempest_ui_kit::{Look, Theme};
use wasm_bindgen::JsCast;

use crate::areas::{Catalog, Sections, Settings};
use crate::config::Config;
use crate::controls::Controls;
use crate::detail::Detail;
use crate::dialogs::Dialogs;
use crate::keys::{self, Command, Scope};
use crate::list::{Move, Order, Transfers};
use crate::share::{Share, SharePath, share_ids};
use crate::state::{Area, Deeper, Dialog, Ui};
use crate::store::Store;
use crate::stream::{Follower, Link};
use crate::{notify, present, share};

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
    let toasts = use_toasts();
    let controls = Controls::new(toasts);
    let fields = Fields {
        search: NodeRef::new(),
        status: NodeRef::new(),
        sort: NodeRef::new(),
    };
    let order = Order::new(store, ui);
    let config = Config::new(store, ui, toasts);
    let sections = Sections::new(config, store, ui);
    provide_context(store);
    provide_context(ui);
    provide_context(controls);
    provide_context(fields);
    provide_context(order);
    provide_context(config);
    provide_context(sections);
    provide_context(SharePath(NodeRef::new()));

    // Registered before the drawer's and dialogs' own, so `esc` goes back one level at a time.
    keyboard(store, ui, controls, fields, order);
    typing(ui);
    follow(store);
    look(config, ui);
    tell(store, ui, controls);

    let area = move || match ui.area.get() {
        Area::Transfers => view! { <Transfers /> }.into_any(),
        Area::Share => view! { <Share /> }.into_any(),
        Area::Catalog => view! { <Catalog /> }.into_any(),
        Area::Settings => view! { <Settings /> }.into_any(),
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

/// Keeps the look, columns and views as the engine's `ui` settings say (D-195), and saves the
/// look when the user changes it. Like the TUI, the page takes the look once, when the
/// settings first arrive, and the columns and views whenever they change.
fn look(config: Config, ui: Ui) {
    apply_look(Signal::derive(move || Look {
        theme: ui.theme.get(),
        scanlines: ui.scanlines.get(),
        clean: ui.clean.get(),
    }));
    ui.notify.set(notify::enabled());
    // The look the engine has, once known.
    let saved = StoredValue::new(None::<(Theme, bool, bool)>);
    Effect::new(move |_| {
        config.info.with(|info| {
            let Some(settings) = info.as_ref().map(|i| &i.settings.ui) else {
                return;
            };
            if saved.get_value().is_none() {
                let theme = Theme::named(&settings.theme).unwrap_or_default();
                saved.set_value(Some((theme, settings.scanlines, settings.clean)));
                ui.theme.set(theme);
                ui.scanlines.set(settings.scanlines);
                ui.clean.set(settings.clean);
            }
            let columns = prefs::columns(settings);
            if ui.columns.with_untracked(|c| *c != columns) {
                ui.columns.set(columns);
            }
            if ui.views.with_untracked(|v| *v != settings.views) {
                ui.views.set(settings.views.clone());
            }
        });
    });
    Effect::new(move |_| {
        let now = (ui.theme.get(), ui.scanlines.get(), ui.clean.get());
        let Some(known) = saved.get_value() else {
            return;
        };
        if known == now {
            return;
        }
        saved.set_value(Some(now));
        let changes = [
            (known.0 != now.0).then(|| (prefs::THEME, json!(now.0.name()))),
            (known.1 != now.1).then(|| (prefs::SCANLINES, json!(now.1))),
            (known.2 != now.2).then(|| (prefs::CLEAN, json!(now.2))),
        ];
        for (key, value) in changes.into_iter().flatten() {
            config.change_quietly(key, value);
        }
    });
}

/// Tells the user when a transfer completes or fails (`notify`).
fn tell(store: Store, ui: Ui, controls: Controls) {
    Effect::new(move |_| {
        store.notices.with(|notices| {
            let browser = ui.notify.get_untracked();
            for notice in notices {
                notify::tell(notice, browser, controls);
            }
        });
    });
}

/// Follows whether a text field has the keys, for the hint bar.
fn typing(ui: Ui) {
    let field = |e: &web_sys::FocusEvent| {
        e.target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .is_some_and(|el| typing_in(&el))
    };
    let focus = window_event_listener(ev::focusin, move |e| ui.typing.set(field(&e)));
    let blur = window_event_listener(ev::focusout, move |_| ui.typing.set(false));
    on_cleanup(move || {
        focus.remove();
        blur.remove();
    });
}

/// Whether `element` takes typed keys.
fn typing_in(element: &web_sys::Element) -> bool {
    match element.tag_name().as_str() {
        "SELECT" | "TEXTAREA" => true,
        "INPUT" => element
            .get_attribute("type")
            .is_none_or(|t| !matches!(t.as_str(), "checkbox" | "radio" | "submit" | "button")),
        _ => false,
    }
}

/// The keyboard (D-191): one listener, reading the bindings table.
fn keyboard(store: Store, ui: Ui, controls: Controls, fields: Fields, order: Order) {
    // Contexts are read here: the listener runs outside the component tree.
    let lists = Lists {
        order,
        sections: expect_context(),
        share_path: expect_context(),
    };
    let handle = window_event_listener(ev::keydown, move |e| {
        if e.ctrl_key() || e.meta_key() || e.alt_key() {
            return;
        }
        let key = e.key();
        let typing = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .is_some_and(|el| typing_in(&el));
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
        act(command, store, ui, controls, fields, lists);
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

/// What the keys move through: the transfers, the rows of Catalog and Settings, the shares.
#[derive(Clone, Copy)]
struct Lists {
    order: Order,
    sections: Sections,
    share_path: SharePath,
}

fn act(command: Command, store: Store, ui: Ui, controls: Controls, fields: Fields, lists: Lists) {
    let Lists {
        order,
        sections,
        share_path: SharePath(share_path),
    } = lists;
    // The transfer a command acts on: the open one, or the one under the cursor.
    let target = || {
        let id = ui.open.get_untracked().or(ui.cursor.get_untracked())?;
        store.model.with_untracked(|m| m.transfer(&id).cloned())
    };
    let scope = ui.scope_now();
    // Moving acts on whatever the area lists.
    let go = |to: Move| match scope {
        Scope::Share => share::go(ui, &untrack(|| share_ids(store)), to),
        Scope::Rows => sections.go(ui, to),
        _ => order.go(to),
    };
    match command {
        Command::Area(i) => ui.area.set(Area::ALL[i]),
        Command::NextArea => ui.area.update(|a| *a = a.step(1)),
        Command::PreviousArea => ui.area.update(|a| *a = a.step(-1)),
        Command::Theme => ui.theme.update(|t| *t = t.next()),
        Command::Help => ui.dialog.set(Some(Dialog::Help)),
        Command::Down => go(Move::Down),
        Command::Up => go(Move::Up),
        Command::First => go(Move::First),
        Command::Last => go(Move::Last),
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
        Command::Views | Command::SaveView => ui.dialog.set(Some(Dialog::Views)),
        Command::Columns => ui.dialog.set(Some(Dialog::Columns)),
        Command::NextTab => ui.tab.update(|t| *t = t.step(1)),
        Command::PreviousTab => ui.tab.update(|t| *t = t.step(-1)),
        Command::ChunkMap => ui.deeper.set(Some(Deeper::ChunkMap)),
        Command::Rates => ui.deeper.set(Some(Deeper::Rates)),
        Command::NewShare => focus(share_path.get_untracked().map(Into::into)),
        Command::ShowShare => {
            let ids = untrack(|| share_ids(store));
            let chosen = ui.share.get_untracked().filter(|id| ids.contains(id));
            if let Some(id) = chosen.or(ids.first().copied()) {
                ui.show(id);
            }
        }
        Command::Change => sections.use_control(ui, "data-change"),
        Command::AddEntry => sections.use_control(ui, "data-add"),
        Command::RemoveEntry => sections.use_control(ui, "data-remove"),
        Command::CopyLink if scope == Scope::Share => {
            let ids = untrack(|| share_ids(store));
            let chosen = ui.share.get_untracked().filter(|id| ids.contains(id));
            match chosen.or(ids.first().copied()) {
                Some(id) => {
                    let link = store
                        .model
                        .with_untracked(|m| m.transfer(&id).map(|t| t.link.clone()));
                    controls.copy(link.unwrap_or_default());
                }
                None => controls.say(Tone::Dim, "Share something first."),
            }
        }
        Command::StartStop
        | Command::ForceStart
        | Command::Remove
        | Command::Rename
        | Command::Recheck
        | Command::CopyLink
        | Command::Tags
        | Command::Category
        | Command::MoveStorage
        | Command::Limits
        | Command::QueueUp
        | Command::QueueDown
        | Command::QueueTop
        | Command::QueueBottom => {
            let Some(transfer) = target() else {
                controls.say(Tone::Dim, "Choose a transfer first: j and k move.");
                return;
            };
            let id = transfer.id;
            match command {
                Command::StartStop => controls.start_stop(&transfer),
                Command::ForceStart => controls.force_start(&transfer),
                Command::Recheck => controls.recheck(&transfer),
                Command::CopyLink => controls.copy_link(&transfer),
                Command::QueueUp => controls.queue(&transfer, QueueStep::Up),
                Command::QueueDown => controls.queue(&transfer, QueueStep::Down),
                Command::QueueTop => controls.queue(&transfer, QueueStep::Top),
                Command::QueueBottom => controls.queue(&transfer, QueueStep::Bottom),
                Command::Remove => ui.dialog.set(Some(Dialog::Remove(id))),
                Command::Tags => ui.dialog.set(Some(Dialog::Tags(id))),
                Command::Category => ui.dialog.set(Some(Dialog::Category(id))),
                Command::MoveStorage => ui.dialog.set(Some(Dialog::Move(id))),
                Command::Limits => ui.dialog.set(Some(Dialog::Limits(id))),
                _ => ui.dialog.set(Some(Dialog::Rename(id))),
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
