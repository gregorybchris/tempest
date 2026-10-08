//! The page: the app bar, the view the address names, the hint bar, and the keyboard, theme,
//! catalog info and session behind them.

use leptos::ev;
use leptos::html::Input;
use leptos::prelude::*;
use leptos::task::spawn_local;
use tempest_catalog_api::CatalogInfo;
use tempest_catalog_api::account::Account;
use tempest_ui_kit::components::*;
use tempest_ui_kit::theme::apply_look;
use tempest_ui_kit::{Look, Theme};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

use crate::browse::BrowsePage;
use crate::edit::EditPage;
use crate::keys::{self, Command};
use crate::listing::ListingPage;
use crate::me::MePage;
use crate::present;
use crate::request;
use crate::route::Route;
use crate::signin::SignInPage;

/// Where the theme is kept in this browser.
const THEME_KEY: &str = "tempest-catalog-theme";

/// Who is signed in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Me {
    /// Not known yet: the first answer is on its way.
    Unknown,
    /// Nobody.
    SignedOut,
    /// This account.
    SignedIn(Account),
}

impl Me {
    /// The account, if one is signed in.
    pub fn account(&self) -> Option<&Account> {
        match self {
            Me::SignedIn(account) => Some(account),
            _ => None,
        }
    }
}

/// What every view shares.
#[derive(Clone, Copy)]
pub struct Ctx {
    /// The view shown, as the address names it.
    pub route: RwSignal<Route>,
    /// Whether the next change of route replaces the browser's history entry.
    replace_next: StoredValue<bool>,
    /// The theme.
    pub theme: RwSignal<Theme>,
    /// The catalog's categories and rules, once the tracker answers.
    pub info: RwSignal<Option<CatalogInfo>>,
    /// Who is signed in.
    pub me: RwSignal<Me>,
    /// Where signing in returns to.
    pub after_sign_in: StoredValue<Route>,
    /// The search field, which `/` focuses.
    pub search: NodeRef<Input>,
    /// The last key command a view handles, with a count so that repeats register.
    pub pressed: RwSignal<(Option<Command>, u64)>,
    /// Now, in seconds since the Unix epoch, updated every half minute for ages.
    pub now: RwSignal<u64>,
}

impl Ctx {
    /// Shows `route`, as a new entry in the browser's history.
    pub fn go(&self, route: Route) {
        self.route.set(route);
    }

    /// Shows `route` in place of the current entry, as the search does while it is typed.
    pub fn replace(&self, route: Route) {
        if self.route.with_untracked(|r| *r != route) {
            self.replace_next.set_value(true);
            self.route.set(route);
        }
    }

    /// Opens the sign-in view, returning to the current view once signed in.
    pub fn sign_in(&self) {
        let here = self.route.get_untracked();
        if here != Route::SignIn {
            self.after_sign_in.set_value(here);
        }
        self.go(Route::SignIn);
    }

    /// The category's name, from its slug.
    pub fn category_name(&self, slug: &str) -> String {
        self.info.with(|info| match info {
            Some(info) => present::category_name(&info.categories, slug),
            None => slug.to_owned(),
        })
    }

    /// Asks the tracker who is signed in again, as after signing in or out.
    pub fn refresh_me(&self) {
        let me = self.me;
        spawn_local(async move {
            let answer = request::get::<Account>("me").await;
            let _ = me.try_set(match answer {
                Ok(account) => Me::SignedIn(account),
                Err(_) => Me::SignedOut,
            });
        });
    }

    /// The tracker's origin, such as `https://tracker.example.org`, as the browser reached it.
    pub fn origin(&self) -> String {
        window().location().origin().unwrap_or_default()
    }

    /// Copies `text`, and says so.
    pub fn copy(&self, text: String, what: &'static str) {
        let toasts = use_toasts();
        spawn_local(async move {
            let promise = window().navigator().clipboard().write_text(&text);
            match JsFuture::from(promise).await.is_ok() {
                true => toasts.push(Tone::Neutral, format!("Copied the {what}")),
                false => toasts.push(
                    Tone::Bad,
                    "The browser wouldn't copy it. Select it and copy it yourself.",
                ),
            }
        });
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

/// Runs `then` a moment after the last call, so typing doesn't ask the tracker at every key.
pub fn debounce(then: impl FnOnce() + 'static) {
    thread_local! {
        static PENDING: std::cell::Cell<Option<TimeoutHandle>> = const { std::cell::Cell::new(None) };
    }
    PENDING.with(|pending| {
        if let Some(handle) = pending.take() {
            handle.clear();
        }
        let handle = set_timeout_with_handle(then, std::time::Duration::from_millis(120)).ok();
        pending.set(handle);
    });
}

/// Now, in seconds since the Unix epoch.
pub fn unix_now() -> u64 {
    (js_sys::Date::now() / 1000.0) as u64
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
        info: RwSignal::new(None),
        me: RwSignal::new(Me::Unknown),
        after_sign_in: StoredValue::new(Route::default()),
        search: NodeRef::new(),
        pressed: RwSignal::new((None, 0)),
        now: RwSignal::new(unix_now()),
    };
    provide_context(ctx);
    follow_address(ctx);
    keep_look(ctx);
    keyboard(ctx);
    load_info(ctx);
    ctx.refresh_me();
    if let Ok(timer) = set_interval_with_handle(
        move || ctx.now.set(unix_now()),
        std::time::Duration::from_secs(30),
    ) {
        on_cleanup(move || timer.clear());
    }
    // Each view starts at the top.
    Effect::new(move |_| {
        let _ = page_of(&ctx.route.get());
        window().scroll_to_with_x_and_y(0.0, 0.0);
    });

    // Views are rebuilt only when the kind of view or its subject changes, not its query, so
    // the search field keeps its focus while it is typed in.
    let page = Memo::new(move |_| page_of(&ctx.route.get()));
    let view = move || match page.get() {
        Page::Browse => view! { <BrowsePage /> }.into_any(),
        Page::Listing(bundle) => view! { <ListingPage bundle /> }.into_any(),
        Page::Edit(bundle) => view! { <EditPage bundle /> }.into_any(),
        Page::SignIn => view! { <SignInPage /> }.into_any(),
        Page::Me => view! { <MePage /> }.into_any(),
    };
    let hints = Signal::derive(move || ctx.route.with(keys::hints));
    let browsing = move || matches!(page.get(), Page::Browse);
    view! {
        <div class="app">
            <header class="appbar">
                <a class="appbar__home" href="#/" aria-label="Catalog home">
                    <Wordmark />
                </a>
                <span class="appbar__product">"catalog"</span>
                <nav class="appbar__links" aria-label="Areas">
                    <a href="#/" aria-current=move || browsing().then_some("page")>"Browse"</a>
                    <Show when=move || ctx.me.with(|m| m.account().is_some())>
                        <a href="#/me" aria-current=move || (page.get() == Page::Me).then_some("page")>"My listings"</a>
                    </Show>
                </nav>
                <div class="appbar__spacer"></div>
                <AccountButton />
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
            <HintBar hints />
        </div>
    }
}

/// The signed-in account's name, or a way to sign in.
#[component]
fn AccountButton() -> impl IntoView {
    let ctx = use_ctx();
    move || {
        match ctx.me.get() {
        Me::Unknown => ().into_any(),
        Me::SignedOut => view! {
            <button class="btn btn--small btn--primary account-button" type="button" on:click=move |_| ctx.sign_in()>
                "Sign in"
            </button>
        }
        .into_any(),
        Me::SignedIn(account) => {
            let name = account
                .name
                .clone()
                .unwrap_or_else(|| present::short_key(&account.key.to_string()));
            view! {
                <a class="account-link" href="#/me" title="Your account (a)">
                    <span class="account-link__dot" aria-hidden="true"></span>
                    {name}
                </a>
            }
            .into_any()
        }
    }
    }
}

/// The kind of view shown, and its subject.
#[derive(Clone, Copy, PartialEq)]
enum Page {
    Browse,
    Listing(tempest_protocol::BundleId),
    Edit(tempest_protocol::BundleId),
    SignIn,
    Me,
}

fn page_of(route: &Route) -> Page {
    match route {
        Route::Browse(_) => Page::Browse,
        Route::Listing(bundle) => Page::Listing(*bundle),
        Route::Edit(bundle) => Page::Edit(*bundle),
        Route::SignIn => Page::SignIn,
        Route::Me => Page::Me,
    }
}

fn load_info(ctx: Ctx) {
    spawn_local(async move {
        if let Ok(info) = request::get::<CatalogInfo>("info").await {
            let _ = ctx.info.try_set(Some(info));
        }
    });
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

/// The theme: `?theme=` in the address, or the one last chosen in this browser, or else the
/// light theme for a browser that prefers light and green otherwise.
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
    let preferred = || {
        let light = window()
            .match_media("(prefers-color-scheme: light)")
            .ok()
            .flatten()
            .is_some_and(|m| m.matches());
        light.then_some(Theme::Beige)
    };
    asked.or_else(saved).or_else(preferred).unwrap_or_default()
}

fn keep_look(ctx: Ctx) {
    apply_look(Signal::derive(move || Look {
        theme: ctx.theme.get(),
        ..Look::default()
    }));
    // Saved only once chosen, so the browser's preference keeps deciding until then.
    Effect::new(move |previous: Option<Theme>| {
        let theme = ctx.theme.get();
        if previous.is_some_and(|p| p != theme)
            && let Some(storage) = storage()
        {
            let _ = storage.set_item(THEME_KEY, theme.name());
        }
        theme
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

/// Focuses the search field once it exists.
pub fn focus_search(ctx: Ctx) {
    request_animation_frame(move || {
        if let Some(input) = ctx.search.get_untracked() {
            let _ = input.focus();
            input.select();
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
        let typing = target.as_ref().is_some_and(typing_in);
        if e.key() == "Escape" {
            if typing {
                if let Some(field) = target.and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok())
                {
                    let _ = field.blur();
                }
            } else if let Some(up) = ctx.route.with_untracked(Route::up) {
                ctx.go(up);
            }
            return;
        }
        // Enter on a link or button is the browser's.
        let on_control = target
            .as_ref()
            .is_some_and(|t| matches!(t.tag_name().as_str(), "A" | "BUTTON"));
        if typing || (e.key() == "Enter" && on_control) {
            return;
        }
        let Some(command) = ctx.route.with_untracked(|r| keys::command(r, &e.key())) else {
            return;
        };
        e.prevent_default();
        match command {
            Command::Search => {
                if !matches!(ctx.route.get_untracked(), Route::Browse(_)) {
                    ctx.go(Route::default());
                }
                focus_search(ctx);
            }
            Command::Theme => ctx.theme.update(|t| *t = t.next()),
            Command::Account => match ctx.me.get_untracked() {
                Me::SignedIn(_) => ctx.go(Route::Me),
                _ => ctx.sign_in(),
            },
            command => ctx.pressed.update(|p| *p = (Some(command), p.1 + 1)),
        }
    });
    on_cleanup(move || handle.remove());
}

/// Runs `then` with each key command a view handles, as it is pressed.
pub fn on_command(ctx: Ctx, then: impl Fn(Command) + 'static) {
    Effect::new(move |previous: Option<u64>| {
        let (command, count) = ctx.pressed.get();
        if previous.is_some_and(|p| p != count)
            && let Some(command) = command
        {
            then(command);
        }
        count
    });
}
