//! The account signed in: its name, its key, signing out, and the listings it published,
//! whatever their status.

use leptos::prelude::*;
use leptos::task::spawn_local;
use tempest_tracker_api::account::{Account, AccountEdit};
use tempest_tracker_api::listing::{Listing, ListingPage, ListingStatus};
use tempest_tracker_api::text;
use tempest_ui_kit::components::*;
use tempest_ui_kit::format;

use crate::app::{Me, use_ctx};
use crate::present::{self, plural};
use crate::request::{self, method};
use crate::route::Route;
use crate::views::{Copyable, HealthBars, fetch, shown};

/// The most of the account's listings shown, which the API allows in one page.
const MAX_SHOWN: u32 = tempest_tracker_api::listing::MAX_PAGE;

/// The account page.
#[component]
pub fn MePage() -> impl IntoView {
    let ctx = use_ctx();
    move || {
        match ctx.me.get() {
        Me::Unknown => view! { <p class="page loading dim">"loading…"</p> }.into_any(),
        Me::SignedOut => view! {
            <section class="page">
                <Empty title="Not signed in" text="Sign in to see your account and the listings you published.".to_owned()>
                    <Button kind=ButtonKind::Primary on_click=move || ctx.sign_in()>"Sign in"</Button>
                </Empty>
            </section>
        }
        .into_any(),
        Me::SignedIn(account) => view! { <AccountView account /> }.into_any(),
    }
    }
}

#[component]
fn AccountView(account: Account) -> impl IntoView {
    let ctx = use_ctx();
    let toasts = use_toasts();
    let name = RwSignal::new(account.name.clone().unwrap_or_default());
    let problem = RwSignal::new(None::<String>);
    let saving = RwSignal::new(false);
    let save = move || {
        let typed = name.get_untracked();
        let edit = match typed.trim().is_empty() {
            true => AccountEdit { name: None },
            false => match text::clean_name(&typed) {
                Ok(clean) => AccountEdit { name: Some(clean) },
                Err(error) => {
                    problem.set(Some(request::sentence(&error.0)));
                    return;
                }
            },
        };
        problem.set(None);
        saving.set(true);
        spawn_local(async move {
            let answer = request::send::<Account>(method::PUT, "account", Some(&edit)).await;
            let _ = saving.try_set(false);
            match answer {
                Ok(account) => {
                    toasts.push(Tone::Up, "Saved your name");
                    ctx.me.set(Me::SignedIn(account));
                }
                Err(error) => {
                    let _ = problem.try_set(Some(error.to_string()));
                }
            }
        });
    };
    let sign_out = move || {
        spawn_local(async move {
            match request::send::<serde_json::Value>(method::DELETE, "session", None::<&()>).await {
                Ok(_) => {
                    ctx.me.set(Me::SignedOut);
                    toasts.push(Tone::Neutral, "Signed out");
                    ctx.go(Route::default());
                }
                Err(error) => toasts.push(Tone::Bad, error.to_string()),
            }
        });
    };
    let key = account.key.to_string();
    let joined = account.joined;
    view! {
        <section class="page me">
            <header class="page__head">
                <h1 class="page__title glow">{account.name.clone().unwrap_or_else(|| "Your account".to_owned())}</h1>
                <p class="dim">{move || format!("Joined {}.", present::ago(ctx.now.get(), joined))}</p>
            </header>
            <div class="me__grid">
                <Panel title="Name">
                    <form class="me__name" on:submit=move |e| {
                        e.prevent_default();
                        save();
                    }>
                        <TextField
                            label="Shown on your listings and comments"
                            value=name
                            placeholder="no name: your key shows"
                            error=problem
                        />
                        <button class="btn btn--primary" type="submit" disabled=move || saving.get()>"Save"</button>
                    </form>
                </Panel>
                <Panel title="Key">
                    <p class="note dim">"Your account is this Tempest key. Signing in from another computer needs the same key."</p>
                    <Copyable text=key what="key" />
                    <div class="me__actions">
                        <Button kind=ButtonKind::Danger small=true on_click=move || sign_out()>"Sign out"</Button>
                    </div>
                </Panel>
            </div>
            <MyListings />
        </section>
    }
}

/// The listings the account published.
#[component]
fn MyListings() -> impl IntoView {
    let (fetched, _) = fetch::<ListingPage>(move || format!("account/listings?limit={MAX_SHOWN}"));
    let list = shown(fetched, move |page: ListingPage| {
        if page.listings.is_empty() {
            return view! {
                <Empty title="Nothing published yet" text="Publish a share with: tempest share --publish <this tracker> <path>. It shows here at once, and to everyone once the tracker has its manifest.".to_owned() />
            }
            .into_any();
        }
        let more = (page.total > page.listings.len() as u64).then(|| {
            view! { <p class="note dim">{format!("Showing the newest {} of {}.", page.listings.len(), page.total)}</p> }
        });
        view! {
            <div class="table-scroll">
                <table class="table mine">
                    <thead>
                        <tr>
                            <th>"Title"</th>
                            <th>"Status"</th>
                            <th class="num">"Health"</th>
                            <th class="num">"Votes"</th>
                            <th class="num">"Downloads"</th>
                            <th class="num">"Comments"</th>
                            <th></th>
                        </tr>
                    </thead>
                    <tbody>
                        {page.listings.into_iter().map(|listing| view! { <MyRow listing /> }).collect_view()}
                    </tbody>
                </table>
            </div>
            {more}
        }
        .into_any()
    });
    let count = move || match fetched.get() {
        Some(Ok(page)) => plural(page.total, "listing"),
        _ => "Your listings".to_owned(),
    };
    view! {
        <section>
            <h2 class="section-title">{count}</h2>
            {list}
        </section>
    }
}

#[component]
fn MyRow(listing: Listing) -> impl IntoView {
    let ctx = use_ctx();
    let bundle = listing.bundle;
    let (label, tone) = match listing.status {
        ListingStatus::Listed => ("listed", Tone::Up),
        ListingStatus::Pending => ("pending", Tone::Warn),
        ListingStatus::Unpublished => ("unpublished", Tone::Dim),
    };
    let why = match listing.status {
        ListingStatus::Listed => "Shown to everyone",
        ListingStatus::Pending => {
            "Waiting for the tracker to fetch its manifest from a peer sharing it"
        }
        ListingStatus::Unpublished => "Only you see it; edit it to list it again",
    };
    let title = match listing.title.is_empty() {
        true => "untitled".to_owned(),
        false => listing.title.clone(),
    };
    view! {
        <tr class="row--link" on:click=move |e| if row_click(&e) { ctx.go(Route::Listing(bundle)) }>
            <td class="table__clip"><a href=Route::Listing(bundle).hash()>{title}</a></td>
            <td title=why><Badge tone label=label.to_owned() /></td>
            <td class="num"><HealthBars swarm=listing.swarm /></td>
            <td class="num">{present::score(listing.votes)}</td>
            <td class="num">{format::count(listing.downloads)}</td>
            <td class="num">{format::count(listing.comments)}</td>
            <td class="num">
                <a class="btn btn--small" href=Route::Edit(bundle).hash() on:click=|e| e.stop_propagation()>"edit"</a>
            </td>
        </tr>
    }
}
