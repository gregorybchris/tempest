//! The page: the app bar, the area shown, the detail drawer, the dialogs and the hint bar, and
//! the keyboard and connections behind them.

use std::time::Duration;

use leptos::ev;
use leptos::html::Input;
use leptos::prelude::*;
use leptos::task::spawn_local;
use send_wrapper::SendWrapper;
use serde_json::json;
use tempest_api::prefs;
use tempest_api::requests::QueueStep;
use tempest_protocol::BundleId;
use tempest_ui_kit::components::*;
use tempest_ui_kit::theme::{ThemeButton, apply_look};
use tempest_ui_kit::{Look, Theme};
use wasm_bindgen::JsCast;

use crate::areas::{Sections, Settings};
use crate::config::Config;
use crate::controls::Controls;
use crate::detail::Detail;
use crate::dialogs::Dialogs;
use crate::keys::{self, Command, Scope};
use crate::list::{Move, Order, Transfers};
use crate::picker::{Picker, PickerHost};
use crate::share::{Share, SharePath, share_ids};
use crate::signin::{self, Auth, SignIn};
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
    pub status: NodeRef<leptos::html::Button>,
    /// The sort.
    pub sort: NodeRef<leptos::html::Button>,
}

/// The whole page.
#[component]
pub fn App() -> impl IntoView {
    let auth = RwSignal::new(Auth::Checking);
    provide_context(auth);
    signin::watch(auth);
    // The sign-in page can't ask the engine for the look, so it wears the last one shown here.
    apply_look(Signal::derive(signin::remembered_look));
    // A memo, so the page is rebuilt only when the answer changes.
    let state = Memo::new(move |_| auth.get());
    view! {
        <Toaster>
            {move || match state.get() {
                Auth::Checking => ().into_any(),
                Auth::SignedOut => view! { <SignIn auth /> }.into_any(),
                Auth::SignedIn { .. } => view! { <Shell /> }.into_any(),
            }}
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
    provide_context(Picker::new());

    // Registered before the drawer's and dialogs' own, so `esc` goes back one level at a time.
    keyboard(store, ui, controls, fields, order);
    typing(ui);
    follow(store);
    look(config, ui);
    tell(store, ui, controls);
    close_with_transfer(store, ui);
    offered(ui);

    let area = move || match ui.area.get() {
        Area::Transfers => view! { <Transfers /> }.into_any(),
        Area::Share => view! { <Share /> }.into_any(),
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
                <SignOut />
                <ThemeButton theme=ui.theme />
            </header>
            <LinkBanner />
            <main class="main">{area}</main>
            <HintBar hints />
        </div>
        <Detail />
        <Dialogs />
        // After the dialogs, so it opens over the one that asked for it.
        <PickerHost />
    }
}

/// The prefix of the address's `#` that `tempest open` uses to hand the page a link (D-263).
const OFFER: &str = "#/add?link=";

/// The link that `hash` offers, decoded, if it is one.
fn offered_link(hash: &str) -> Option<String> {
    let encoded = hash.strip_prefix(OFFER)?;
    js_sys::decode_uri_component(encoded).ok()?.as_string()
}

/// Opens the download confirmation for the link the address offers, then clears the address so
/// a reload doesn't ask again.
fn offered(ui: Ui) {
    let window = window();
    let Some(link) = window.location().hash().ok().and_then(|h| offered_link(&h)) else {
        return;
    };
    if let Ok(history) = window.history() {
        let _ = history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some("#"));
    }
    ui.offer.set(Some(link));
    ui.dialog.set(Some(Dialog::Download));
}

/// Offers to sign out, when the engine asked for a sign-in.
#[component]
fn SignOut() -> impl IntoView {
    let auth = expect_context::<RwSignal<Auth>>();
    move || {
        matches!(auth.get(), Auth::SignedIn { remote: true }).then(|| {
            view! {
                <Button
                    small=true
                    kind=ButtonKind::Ghost
                    title="End this browser's session"
                    on_click=move || {
                        spawn_local(async move {
                            let _ = signin::sign_out().await;
                            signin::check(auth);
                        });
                    }
                >
                    "Sign out"
                </Button>
            }
        })
    }
}

/// Follows the engine's events into the store, and samples rates once a second. When the
/// stream breaks it also asks whether the session is still good, as the stream can't say.
fn follow(store: Store) {
    let auth = expect_context::<RwSignal<Auth>>();
    let follower = Follower::start(
        move |event| store.push(event),
        move |link| {
            if matches!(link, Link::Lost { .. }) {
                signin::check(auth);
            }
            store.link.set(link);
        },
    );
    let stopper = SendWrapper::new(follower.clone());
    on_cleanup(move || stopper.stop());
    provide_context(StoredValue::new(SendWrapper::new(follower)));
    // Rates known before the link broke are no measure of now, so they aren't sampled.
    let sample = move || {
        if store.link.get_untracked() == Link::Live {
            store.sample();
        }
    };
    if let Ok(sampling) = set_interval_with_handle(sample, Duration::from_secs(1)) {
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
    let look = Signal::derive(move || Look {
        theme: ui.theme.get(),
        scanlines: ui.scanlines.get(),
        clean: ui.clean.get(),
    });
    apply_look(look);
    Effect::new(move |_| signin::remember_look(&look.get()));
    ui.notify.set(notify::enabled());
    // The look the engine has, once known.
    let saved = StoredValue::new(None::<(Theme, bool, bool)>);
    let saved_network = StoredValue::new(None::<()>);
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
            // The first run says what the network defaults share, once, when no other dialog
            // is open.
            if !settings.network_explained
                && saved_network.get_value().is_none()
                && ui.dialog.with(Option::is_none)
            {
                saved_network.set_value(Some(()));
                ui.dialog.set(Some(Dialog::Network));
                config.change_quietly(prefs::NETWORK_EXPLAINED, json!(true));
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

/// Closes a dialog about a transfer once the engine no longer has it, as when another client
/// removed it. Left open, a dialog with nothing to show would still take the keys.
fn close_with_transfer(store: Store, ui: Ui) {
    Effect::new(move |_| {
        let Some(id) = ui.dialog.with(|d| d.as_ref().and_then(Dialog::transfer)) else {
            return;
        };
        if store.loaded.get() && store.model.with(|m| m.transfer(&id).is_none()) {
            ui.dialog.set(None);
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
            .is_some_and(|el| takes_keys(&el))
    };
    let focus = window_event_listener(ev::focusin, move |e| ui.typing.set(field(&e)));
    let blur = window_event_listener(ev::focusout, move |_| ui.typing.set(false));
    // A dialog's field that goes with the dialog fires no focusout.
    Effect::new(move |_| {
        if ui.dialog.with(Option::is_none) {
            ui.typing.set(false);
        }
    });
    on_cleanup(move || {
        focus.remove();
        blur.remove();
    });
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
        // A control that handled the key, such as tabs taking the arrows, says so.
        if e.ctrl_key() || e.meta_key() || e.alt_key() || e.default_prevented() {
            return;
        }
        let key = e.key();
        // Enter and space on a focused button, link or switch are the control's own.
        if matches!(key.as_str(), "Enter" | " ") && e.target().is_some_and(|t| activates(&t)) {
            return;
        }
        let typing = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .is_some_and(|el| takes_keys(&el));
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

/// Whether `target` is a control that enter or space activates.
fn activates(target: &web_sys::EventTarget) -> bool {
    let Some(element) = target.dyn_ref::<web_sys::Element>() else {
        return false;
    };
    let role = element.get_attribute("role").unwrap_or_default();
    matches!(
        element.tag_name().to_ascii_lowercase().as_str(),
        "button" | "a" | "input" | "select" | "textarea" | "summary"
    ) || matches!(
        role.as_str(),
        "button" | "tab" | "switch" | "option" | "checkbox" | "link"
    )
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
        Scope::List
            if ui
                .finding
                .with_untracked(tempest_api::finding::Finding::filtering) =>
        {
            ui.finding.update(|f| {
                f.text.clear();
                f.status = None;
            });
            true
        }
        _ => false,
    }
}

/// What the keys move through: the transfers, the rows of Settings, the shares.
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
    // The transfer a command acts on: the open one, or the one under the cursor while the
    // filters show it.
    let target = || {
        let shown = |id: &BundleId| order.ids.with_untracked(|ids| ids.contains(id));
        let id = ui
            .open
            .get_untracked()
            .or(ui.cursor.get_untracked().filter(shown))?;
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
            let cursor = ui.cursor.get_untracked();
            if let Some(id) = cursor.filter(|id| order.ids.with_untracked(|ids| ids.contains(id))) {
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
        | Command::OpenFolder
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
                Command::OpenFolder => controls.reveal(&transfer),
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

/// The app bar's ↓ and ↑ meters, over every transfer.
#[component]
fn Meters() -> impl IntoView {
    let store = expect_context::<Store>();
    let rates = move || store.model.with(tempest_api::model::Model::total_rates);
    let history = move |upload: bool| {
        Signal::derive(move || {
            store.sampled.track();
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

/// Says when the engine can't be reached, and offers to try again now. Between tries the
/// stream is briefly connecting again, which doesn't count as reached, so the banner and its
/// live region stay put until the engine answers.
#[component]
fn LinkBanner() -> impl IntoView {
    let store = expect_context::<Store>();
    let lost = RwSignal::new(false);
    Effect::new(move |_| match store.link.get() {
        Link::Lost { .. } => lost.set(true),
        Link::Live => lost.set(false),
        Link::Connecting => {}
    });
    let banner = move || {
        if lost.get() {
            view! {
                <div class="banner tone--bad">
                    <span>"Can't reach the engine. Trying again by itself; is it still running?"</span>
                    <Button small=true on_click=move || retry_now()>"Retry now"</Button>
                </div>
            }
            .into_any()
        } else if store.link.get() == Link::Connecting && store.loaded.get_untracked() {
            view! { <div class="banner tone--warn">"Reconnecting to the engine…"</div> }.into_any()
        } else {
            ().into_any()
        }
    };
    view! { <div role="status">{banner}</div> }
}
