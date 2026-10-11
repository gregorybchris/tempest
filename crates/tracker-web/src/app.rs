//! The page: the app bar, the view the address names, the hint bar, and the keyboard, theme,
//! tracker info and session behind them.

use leptos::html::Input;
use leptos::prelude::*;
use leptos::task::spawn_local;
use tempest_tracker_api::TrackerInfo;
use tempest_tracker_api::account::Account;
use tempest_ui_kit::Theme;
use tempest_ui_kit::components::*;
use tempest_ui_kit::shell::{Address, PageKey, page_keys, use_theme};
use tempest_ui_kit::theme::ThemeButton;
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
use crate::user::UserPage;

/// Where the theme is kept in this browser.
const THEME_KEY: &str = "tempest-tracker-theme";

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
    address: Address<Route>,
    /// The theme.
    pub theme: RwSignal<Theme>,
    /// The tracker's categories and rules, once it answers.
    pub info: RwSignal<Option<TrackerInfo>>,
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
        self.address.go(route);
    }

    /// Shows `route` in place of the current entry, as the search does while it is typed.
    pub fn replace(&self, route: Route) {
        self.address.replace(route);
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
            let answer = request::get::<Account>("account").await;
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

    /// The tracker's host, with its port if it has one, such as `tracker.example.org`.
    pub fn host(&self) -> String {
        window().location().host().unwrap_or_default()
    }

    /// Copies `text`, and says so.
    pub fn copy(&self, text: String, what: &'static str) {
        let toasts = use_toasts();
        spawn_local(async move {
            let copied = match clipboard_available() {
                true => {
                    let promise = window().navigator().clipboard().write_text(&text);
                    JsFuture::from(promise).await.is_ok()
                }
                false => copy_with_selection(&text),
            };
            match copied {
                true => toasts.push(Tone::Neutral, format!("Copied the {what}")),
                false => toasts.push(
                    Tone::Bad,
                    "Copy failed. Select the text and copy it manually.",
                ),
            }
        });
    }
}

/// Whether the async clipboard API exists. Browsers hide it on pages that aren't a secure
/// context, such as plain HTTP to an address other than localhost.
fn clipboard_available() -> bool {
    js_sys::Reflect::get(&window().navigator(), &"clipboard".into())
        .is_ok_and(|c| !c.is_undefined())
}

/// Copies by selecting the text in a hidden field, for pages without the clipboard API.
fn copy_with_selection(text: &str) -> bool {
    use wasm_bindgen::JsCast;
    let Some(document) = window().document() else {
        return false;
    };
    let (Some(body), Ok(field)) = (document.body(), document.create_element("textarea")) else {
        return false;
    };
    let Ok(field) = field.dyn_into::<web_sys::HtmlTextAreaElement>() else {
        return false;
    };
    field.set_value(text);
    let _ = field.set_attribute("readonly", "");
    let _ = field.set_attribute("style", "position:fixed;top:0;left:0;opacity:0");
    if body.append_child(&field).is_err() {
        return false;
    }
    field.select();
    let copied = document
        .dyn_ref::<web_sys::HtmlDocument>()
        .is_some_and(|d| d.exec_command("copy").unwrap_or(false));
    let _ = body.remove_child(&field);
    copied
}

/// The shared context.
pub fn use_ctx() -> Ctx {
    expect_context()
}

/// Now, in seconds since the Unix epoch.
pub fn unix_now() -> u64 {
    (js_sys::Date::now() / 1000.0) as u64
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
    let address = Address::follow();
    let ctx = Ctx {
        route: address.route,
        address,
        theme: use_theme(THEME_KEY),
        info: RwSignal::new(None),
        me: RwSignal::new(Me::Unknown),
        after_sign_in: StoredValue::new(Route::default()),
        search: NodeRef::new(),
        pressed: RwSignal::new((None, 0)),
        now: RwSignal::new(unix_now()),
    };
    provide_context(ctx);
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
        Page::User(key) => view! { <UserPage key /> }.into_any(),
        Page::SignIn => view! { <SignInPage /> }.into_any(),
        Page::Me => view! { <MePage /> }.into_any(),
    };
    let hints = Signal::derive(move || ctx.route.with(keys::hints));
    let browsing = move || matches!(page.get(), Page::Browse);
    let host = ctx.host();
    let host_title = host.clone();
    view! {
        <div class="app">
            <header class="appbar">
                <a class="appbar__home" href="#/" aria-label="Tracker home">
                    <Wordmark />
                </a>
                <span class="appbar__tracker" title=host_title>
                    <span class="appbar__kind">"Tracker"</span>
                    <span class="appbar__host">{host}</span>
                </span>
                <nav class="appbar__links" aria-label="Main">
                    <a href="#/" aria-current=move || browsing().then_some("page")>"Browse"</a>
                    <Show when=move || ctx.me.with(|m| m.account().is_some())>
                        <a href="#/me" aria-current=move || (page.get() == Page::Me).then_some("page")>"My bundles"</a>
                    </Show>
                </nav>
                <div class="appbar__spacer"></div>
                <AccountButton />
                <ThemeButton theme=ctx.theme />
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
    User(tempest_protocol::PeerId),
    SignIn,
    Me,
}

fn page_of(route: &Route) -> Page {
    match route {
        Route::Browse(_) => Page::Browse,
        Route::Listing(bundle) => Page::Listing(*bundle),
        Route::Edit(bundle) => Page::Edit(*bundle),
        Route::User(key) => Page::User(*key),
        Route::SignIn => Page::SignIn,
        Route::Me => Page::Me,
    }
}

fn load_info(ctx: Ctx) {
    spawn_local(async move {
        if let Ok(info) = request::get::<TrackerInfo>("info").await {
            let _ = ctx.info.try_set(Some(info));
        }
    });
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
    page_keys(move |e, key| {
        let key = match key {
            PageKey::Back => {
                if let Some(up) = ctx.route.with_untracked(Route::up) {
                    ctx.go(up);
                }
                return;
            }
            PageKey::Key(key) => key,
        };
        let Some(command) = ctx.route.with_untracked(|r| keys::command(r, &key)) else {
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
