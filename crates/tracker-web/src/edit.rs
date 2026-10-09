//! The publisher's form for a listing: its title, description, category and tags, and
//! unpublishing it or listing it again.

use leptos::prelude::*;
use leptos::task::spawn_local;
use tempest_protocol::BundleId;
use tempest_tracker_api::listing::{Listing, ListingStatus};
use tempest_tracker_api::text::{MAX_DESCRIPTION, MAX_TITLE};
use tempest_ui_kit::components::*;

use crate::app::{Me, use_ctx};
use crate::present::{self, EditForm, Field};
use crate::request::{self, method};
use crate::route::Route;
use crate::views::{Crumbs, TextArea, fetch, shown};

/// The form's page.
#[component]
pub fn EditPage(bundle: BundleId) -> impl IntoView {
    let ctx = use_ctx();
    let (fetched, _) = fetch::<Listing>(move || {
        ctx.me.track();
        format!("listings/{bundle}")
    });
    shown(fetched, move |listing: Listing| {
        let mine = ctx
            .me
            .with(|m| m.account().is_some_and(|a| a.key == listing.publisher.key));
        match (ctx.me.get(), mine) {
            (_, true) => view! { <Form listing /> }.into_any(),
            (Me::SignedIn(_), false) => view! {
                <section class="page">
                    <Empty title="Not yours" text="Only its publisher can edit a listing.".to_owned() />
                </section>
            }
            .into_any(),
            _ => view! {
                <section class="page">
                    <Empty title="Sign in first" text="Sign in with the key that published this listing to edit it.".to_owned()>
                        <Button kind=ButtonKind::Primary on_click=move || ctx.sign_in()>"Sign in"</Button>
                    </Empty>
                </section>
            }
            .into_any(),
        }
    })
}

#[component]
fn Form(listing: Listing) -> impl IntoView {
    let ctx = use_ctx();
    let toasts = use_toasts();
    let bundle = listing.bundle;
    let title = RwSignal::new(listing.title.clone());
    let description = RwSignal::new(listing.description.clone());
    let category = RwSignal::new(listing.category.clone().unwrap_or_default());
    let tags = RwSignal::new(listing.tags.join(", "));
    let problem = RwSignal::new(None::<(Field, String)>);
    let saving = RwSignal::new(false);
    let confirm = RwSignal::new(false);
    let form = move || EditForm {
        title: title.get(),
        description: description.get(),
        category: category.get(),
        tags: tags.get(),
    };
    let error_in = move |field: Field| {
        Signal::derive(move || {
            problem.with(|p| {
                p.as_ref()
                    .filter(|(f, _)| *f == field)
                    .map(|(_, e)| e.clone())
            })
        })
    };
    let unpublished = listing.status == ListingStatus::Unpublished;

    let save = move || {
        let edit = match form().edit() {
            Ok(edit) => edit,
            Err(problem_found) => {
                problem.set(Some(problem_found));
                return;
            }
        };
        problem.set(None);
        saving.set(true);
        spawn_local(async move {
            let path = format!("listings/{bundle}");
            let answer = request::send::<Listing>(method::PUT, &path, Some(&edit)).await;
            let _ = saving.try_set(false);
            match answer {
                Ok(saved) => {
                    let said = match (unpublished, saved.status) {
                        (true, _) => "Listed it again",
                        (_, ListingStatus::Pending) => {
                            "Saved; it shows once the tracker has its manifest"
                        }
                        _ => "Saved",
                    };
                    toasts.push(Tone::Up, said);
                    ctx.go(Route::Listing(bundle));
                }
                Err(error) => toasts.push(Tone::Bad, error.to_string()),
            }
        });
    };
    let unpublish = move || {
        confirm.set(false);
        spawn_local(async move {
            let path = format!("listings/{bundle}");
            match request::send::<serde_json::Value>(method::DELETE, &path, None::<&()>).await {
                Ok(_) => {
                    toasts.push(Tone::Neutral, "Unpublished. Edit it to list it again.");
                    ctx.go(Route::Me);
                }
                Err(error) => toasts.push(Tone::Bad, error.to_string()),
            }
        });
    };

    let categories = Signal::derive(move || {
        let mut options = vec![(String::new(), "none".to_owned())];
        ctx.info.with(|info| {
            if let Some(info) = info {
                options.extend(
                    info.categories
                        .iter()
                        .map(|c| (c.slug.clone(), c.name.clone())),
                );
            }
        });
        options
    });
    // The tags as they will be saved, as chips under the field.
    let tag_preview = move || match present::parse_tags(&tags.get()) {
        Ok(normal) => normal
            .into_iter()
            .map(|t| view! { <span class="tag">{format!("#{t}")}</span> })
            .collect_view()
            .into_any(),
        Err(_) => ().into_any(),
    };
    let placeholder = listing
        .content
        .as_ref()
        .map(|c| c.name.clone())
        .unwrap_or_default();
    let shown_title = match listing.title.is_empty() {
        true => "untitled".to_owned(),
        false => listing.title.clone(),
    };
    let steps = vec![
        ("listings".to_owned(), "#/".to_owned()),
        (shown_title, Route::Listing(bundle).hash()),
    ];
    view! {
        <section class="page edit">
            <header class="page__head">
                <Crumbs steps here="edit" />
                <h1 class="page__title glow">"Edit the listing"</h1>
                {unpublished.then(|| view! {
                    <p class="notice"><Badge tone=Tone::Warn label="unpublished".to_owned() />" Saving lists it again."</p>
                })}
            </header>
            <form class="edit__form" on:submit=move |e| {
                e.prevent_default();
                save();
            }>
                <label class="field">
                    <span class="field__label">"Title"</span>
                    <input
                        class="input"
                        type="text"
                        maxlength=MAX_TITLE
                        placeholder=placeholder
                        aria-invalid=move || if error_in(Field::Title).get().is_some() { "true" } else { "false" }
                        prop:value=move || title.get()
                        on:input=move |e| title.set(event_target_value(&e))
                    />
                    <span class="field__help">"Empty shows the content's name."</span>
                    {move || error_in(Field::Title).get().map(|e| view! { <span class="field__error">{e}</span> })}
                </label>
                <TextArea
                    label="Description"
                    value=description
                    max=MAX_DESCRIPTION
                    rows=8
                    placeholder="What it is, where it comes from, its license, how to use it."
                    error=error_in(Field::Description)
                />
                <CategoryField label="Category" options=categories value=category placeholder="a new category" />
                <label class="field">
                    <span class="field__label">"Tags"</span>
                    <input
                        class="input"
                        type="text"
                        spellcheck="false"
                        placeholder="weather, csv, open-data"
                        aria-invalid=move || if error_in(Field::Tags).get().is_some() { "true" } else { "false" }
                        prop:value=move || tags.get()
                        on:input=move |e| tags.set(event_target_value(&e))
                    />
                    <span class="field__help">"Separate tags with commas."</span>
                    {move || error_in(Field::Tags).get().map(|e| view! { <span class="field__error">{e}</span> })}
                    <span class="edit__tags">{tag_preview}</span>
                </label>
                <div class="edit__actions">
                    <button class="btn btn--primary" type="submit" disabled=move || saving.get()>
                        {if unpublished { "Save and list again" } else { "Save" }}
                    </button>
                    <a class="btn btn--ghost" href=Route::Listing(bundle).hash()>"Cancel"</a>
                    <span class="appbar__spacer"></span>
                    {(!unpublished).then(|| view! {
                        <Button kind=ButtonKind::Danger on_click=move || confirm.set(true)>"Unpublish"</Button>
                    })}
                </div>
            </form>
            <Dialog
                open=confirm
                on_close=move || confirm.set(false)
                title="Unpublish this listing?"
                actions=ViewFn::from(move || view! {
                    <Button kind=ButtonKind::Ghost on_click=move || confirm.set(false)>"Keep it"</Button>
                    <Button kind=ButtonKind::Danger on_click=move || unpublish()>"Unpublish"</Button>
                })
            >
                <p>"It stops showing on this tracker, and announces won't list it again. You can list it again by editing it."</p>
            </Dialog>
        </section>
    }
}
