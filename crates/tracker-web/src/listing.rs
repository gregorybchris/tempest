//! One listing: what it holds, how to get it, its swarm's health, its votes and its comments.

use leptos::html::Textarea;
use leptos::prelude::*;
use leptos::task::spawn_local;
use tempest_protocol::BundleId;
use tempest_tracker_api::listing::{
    Comment, Listing, ListingStatus, NewComment, NewReport, Vote, VoteRequest,
};
use tempest_tracker_api::query::ListingQuery;
use tempest_tracker_api::text::{self, MAX_COMMENT, MAX_REPORT};
use tempest_ui_kit::components::*;
use tempest_ui_kit::format;

use crate::app::{Me, on_command, use_ctx};
use crate::keys::Command;
use crate::present::{self, plural};
use crate::request::{self, method};
use crate::route::Route;
use crate::views::{Copyable, Crumbs, HealthBars, TextArea, fetch, shown};

/// A listing's page.
#[component]
pub fn ListingPage(bundle: BundleId) -> impl IntoView {
    let ctx = use_ctx();
    let path = format!("listings/{bundle}");
    let (fetched, _) = fetch::<Listing>(move || {
        // Again after signing in or out, for the viewer's vote and the publisher's view.
        ctx.me.track();
        path.clone()
    });
    shown(fetched, move |listing: Listing| {
        let listing = RwSignal::new(listing);
        view! { <Detail listing /> }
    })
}

#[component]
fn Detail(listing: RwSignal<Listing>) -> impl IntoView {
    let ctx = use_ctx();
    let l = listing.get_untracked();
    let bundle = l.bundle;
    let toasts = use_toasts();
    let mine = Memo::new(move |_| {
        let publisher = listing.with(|l| l.publisher.key);
        ctx.me
            .with(|m| m.account().is_some_and(|a| a.key == publisher))
    });
    let comment_box = NodeRef::<Textarea>::new();

    let vote = move |chosen: Vote| {
        if ctx.me.with_untracked(|m| m.account().is_none()) {
            toasts.push(Tone::Warn, "Sign in to vote.");
            ctx.sign_in();
            return;
        }
        // The same vote again takes it back.
        let vote = match listing.with_untracked(|l| l.my_vote) == chosen {
            true => Vote::None,
            false => chosen,
        };
        spawn_local(async move {
            let path = format!("listings/{bundle}/vote");
            match request::send::<Listing>(method::PUT, &path, Some(&VoteRequest { vote })).await {
                Ok(updated) => {
                    let _ = listing.try_set(updated);
                }
                Err(error) => toasts.push(Tone::Bad, error.to_string()),
            }
        });
    };
    on_command(ctx, move |command| match command {
        Command::VoteUp => vote(Vote::Up),
        Command::VoteDown => vote(Vote::Down),
        Command::Comment => match comment_box.get_untracked() {
            Some(textarea) => {
                let _ = textarea.focus();
            }
            None => ctx.sign_in(),
        },
        Command::Edit if mine.get_untracked() => ctx.go(Route::Edit(bundle)),
        _ => {}
    });

    let title = match l.title.is_empty() {
        true => "untitled".to_owned(),
        false => l.title.clone(),
    };
    let mut steps = vec![("listings".to_owned(), "#/".to_owned())];
    if let Some(slug) = &l.category {
        let q = ListingQuery {
            category: Some(slug.clone()),
            ..ListingQuery::default()
        };
        steps.push((ctx.category_name(slug), Route::Browse(q).hash()));
    }
    let status = match l.status {
        ListingStatus::Listed => None,
        ListingStatus::Pending => Some((
            "pending",
            "Only you see this. It shows once the tracker has fetched its manifest from a peer \
             that shares it, so keep sharing it.",
        )),
        ListingStatus::Unpublished => Some((
            "unpublished",
            "Only you see this. Edit it to list it again.",
        )),
    };
    let paragraphs = present::paragraphs(&l.description)
        .into_iter()
        .map(|p| view! { <p>{p}</p> })
        .collect_view();
    let tags = l
        .tags
        .iter()
        .map(|tag| {
            let q = ListingQuery {
                tags: vec![tag.clone()],
                ..ListingQuery::default()
            };
            view! { <a class="tag" href=Route::Browse(q).hash()>{format!("#{tag}")}</a> }
        })
        .collect_view();
    let name = l.content.as_ref().map(|c| c.name.clone());
    let link = present::link(bundle, &ctx.origin(), name.as_deref().or(Some(&l.title)));
    let command = present::get_command(&link);
    let content = l.content.clone().map(|c| {
        view! {
            <div><dt>"name"</dt><dd>{c.name}</dd></div>
            <div><dt>"size"</dt><dd>{format::bytes(c.size)}</dd></div>
            <div><dt>"files"</dt><dd>{format::count(c.files)}</dd></div>
        }
    });
    let now = ctx.now;
    let (created, updated) = (l.created, l.updated);
    let swarm = move || listing.with(|l| l.swarm);
    let my_vote = move || listing.with(|l| l.my_vote);
    let edit_button = move || {
        mine.get().then(|| {
            view! {
                <a class="btn btn--small" href=Route::Edit(bundle).hash() title="Edit (e)">"edit"</a>
            }
        })
    };
    view! {
        <article class="page listing">
            <header class="page__head">
                <Crumbs steps here=present::short_key(&bundle.to_string()) />
                <div class="listing__titlebar">
                    <h1 class="page__title glow">{title}</h1>
                    {edit_button}
                </div>
                <p class="listing__byline dim">
                    "listed by "<span class="listing__publisher">{present::person(&l.publisher)}</span>
                    " · "{move || present::ago(now.get(), created)}
                    {(updated > created).then(|| view! { " · updated "{move || present::ago(now.get(), updated)} })}
                </p>
                {status.map(|(label, text)| view! {
                    <p class="notice"><Badge tone=Tone::Warn label=label.to_owned() />" "{text}</p>
                })}
            </header>
            <div class="listing__grid">
                <div class="listing__main">
                    {match l.description.is_empty() {
                        true => view! { <p class="dim">"No description."</p> }.into_any(),
                        false => view! { <div class="prose">{paragraphs}</div> }.into_any(),
                    }}
                    {(!l.tags.is_empty()).then(|| view! { <div class="listing__tags">{tags}</div> })}
                    <Panel title="Content">
                        {content.is_none().then(|| view! {
                            <p class="note dim">"Not known until the tracker fetches its manifest."</p>
                        })}
                        <dl class="meta">
                            {content}
                            <div><dt>"bundle"</dt><dd class="mono">{bundle.to_string()}</dd></div>
                        </dl>
                    </Panel>
                    <Comments bundle listing comment_box />
                </div>
                <aside class="listing__side">
                    <Panel title="Get it">
                        <p class="note dim">"Open the link in Tempest, or run:"</p>
                        <Copyable text=command what="command" />
                        <div class="listing__get">
                            <a class="btn btn--primary" href=link.clone()>"Open in Tempest"</a>
                            <button class="btn" type="button" on:click={
                                let link = link.clone();
                                move |_| ctx.copy(link.clone(), "link")
                            }>"copy link"</button>
                        </div>
                    </Panel>
                    <Panel title="Swarm">
                        {move || {
                            let s = swarm();
                            let (words, tone) = present::health(&s);
                            view! {
                                <div class="swarm">
                                    <HealthBars swarm=s />
                                    <span class=format!("swarm__words {}", tone.class())>{words}</span>
                                </div>
                            }
                        }}
                        <Stats>
                            <Stat label="seeders" value=Signal::derive(move || format::count(swarm().complete)) />
                            <Stat label="downloading" value=Signal::derive(move || format::count(swarm().incomplete)) />
                            <Stat label="downloads" value=Signal::derive(move || format::count(listing.with(|l| l.downloads))) />
                        </Stats>
                    </Panel>
                    <Panel title="Votes">
                        <div class="votes">
                            <button
                                class="vote vote--up"
                                type="button"
                                title="Vote up (+)"
                                aria-pressed=move || if my_vote() == Vote::Up { "true" } else { "false" }
                                on:click=move |_| vote(Vote::Up)
                            >
                                "▲ "<span class="num">{move || format::count(listing.with(|l| l.votes.up))}</span>
                            </button>
                            <span class="votes__score num glow">{move || present::score(listing.with(|l| l.votes))}</span>
                            <button
                                class="vote vote--down"
                                type="button"
                                title="Vote down (-)"
                                aria-pressed=move || if my_vote() == Vote::Down { "true" } else { "false" }
                                on:click=move |_| vote(Vote::Down)
                            >
                                "▼ "<span class="num">{move || format::count(listing.with(|l| l.votes.down))}</span>
                            </button>
                        </div>
                    </Panel>
                    <ReportLink bundle />
                </aside>
            </div>
        </article>
    }
}

/// A link that opens a form to report the listing to the operator.
#[component]
fn ReportLink(bundle: BundleId) -> impl IntoView {
    let ctx = use_ctx();
    let toasts = use_toasts();
    let open = RwSignal::new(false);
    let reason = RwSignal::new(String::new());
    let problem = RwSignal::new(None::<String>);
    let sending = RwSignal::new(false);
    // An error is about what was typed, so typing again clears it.
    Effect::new(move |_| {
        reason.track();
        problem.set(None);
    });
    let start = move || {
        if ctx.me.with_untracked(|m| m.account().is_none()) {
            toasts.push(Tone::Warn, "Sign in to report a listing.");
            ctx.sign_in();
            return;
        }
        problem.set(None);
        open.set(true);
    };
    let send = move || {
        let cleaned = match text::clean_report(&reason.get_untracked()) {
            Ok(reason) => reason,
            Err(error) => {
                problem.set(Some(request::sentence(&error.0)));
                return;
            }
        };
        problem.set(None);
        sending.set(true);
        spawn_local(async move {
            let path = format!("listings/{bundle}/reports");
            let body = NewReport { reason: cleaned };
            let answer = request::send::<serde_json::Value>(method::POST, &path, Some(&body)).await;
            let _ = sending.try_set(false);
            match answer {
                Ok(_) => {
                    let _ = open.try_set(false);
                    let _ = reason.try_set(String::new());
                    let _ = problem.try_set(None);
                    toasts.push(Tone::Neutral, "Reported. The operator will see it.");
                }
                Err(error) => {
                    let _ = problem.try_set(Some(error.to_string()));
                }
            }
        });
    };
    view! {
        <p class="listing__report">
            <button class="linkish" type="button" on:click=move |_| start()>"Report this listing"</button>
        </p>
        <Dialog
            open
            on_close=move || open.set(false)
            title="Report this listing"
            actions=ViewFn::from(move || view! {
                <Button kind=ButtonKind::Ghost on_click=move || open.set(false)>"Cancel"</Button>
                <Button
                    kind=ButtonKind::Primary
                    disabled=sending
                    on_click=move || send()
                >"Report"</Button>
            })
        >
            <p class="note dim">
                "Tell the operator what is wrong with it. They decide what to do; nothing happens automatically."
            </p>
            <TextArea label="Reason" value=reason max=MAX_REPORT rows=4 error=problem />
        </Dialog>
    }
}

/// The comments, oldest first, and the form for a new one.
#[component]
fn Comments(
    bundle: BundleId,
    listing: RwSignal<Listing>,
    comment_box: NodeRef<Textarea>,
) -> impl IntoView {
    let ctx = use_ctx();
    let toasts = use_toasts();
    let path = format!("listings/{bundle}/comments");
    let (fetched, _) = fetch::<Vec<Comment>>(move || {
        ctx.me.track();
        path.clone()
    });
    let draft = RwSignal::new(String::new());
    let problem = RwSignal::new(None::<String>);
    let posting = RwSignal::new(false);
    let post = move || {
        let body = match text::clean_comment(&draft.get_untracked()) {
            Ok(body) => body,
            Err(error) => {
                problem.set(Some(request::sentence(&error.0)));
                return;
            }
        };
        problem.set(None);
        posting.set(true);
        spawn_local(async move {
            let path = format!("listings/{bundle}/comments");
            let answer =
                request::send::<Comment>(method::POST, &path, Some(&NewComment { body })).await;
            let _ = posting.try_set(false);
            match answer {
                Ok(comment) => {
                    fetched.update(|f| {
                        if let Some(Ok(list)) = f {
                            list.push(comment);
                        }
                    });
                    listing.update(|l| l.comments += 1);
                    draft.set(String::new());
                }
                Err(error) => {
                    let _ = problem.try_set(Some(error.to_string()));
                }
            }
        });
    };
    let remove = move |id: u64| {
        spawn_local(async move {
            let path = format!("comments/{id}");
            match request::send::<serde_json::Value>(method::DELETE, &path, None::<&()>).await {
                Ok(_) => {
                    fetched.update(|f| {
                        if let Some(Ok(list)) = f {
                            list.retain(|c| c.id != id);
                        }
                    });
                    listing.update(|l| l.comments = l.comments.saturating_sub(1));
                }
                Err(error) => toasts.push(Tone::Bad, error.to_string()),
            }
        });
    };
    let count = move || listing.with(|l| l.comments);
    let list = shown(fetched, move |comments: Vec<Comment>| {
        if comments.is_empty() {
            return view! { <p class="dim">"No comments yet."</p> }.into_any();
        }
        let me = ctx.me.with_untracked(|m| m.account().map(|a| a.key));
        view! {
            <ol class="comments">
                {comments
                    .into_iter()
                    .map(|c| {
                        let id = c.id;
                        let own = me == Some(c.author.key);
                        let created = c.created;
                        view! {
                            <li class="comment">
                                <div class="comment__head">
                                    <span class="comment__author" class:comment__author--me=own>{present::person(&c.author)}</span>
                                    <span class="dim">{move || present::ago(ctx.now.get(), created)}</span>
                                    {own.then(|| view! {
                                        <button class="btn btn--ghost btn--small comment__delete" type="button" on:click=move |_| remove(id)>
                                            "delete"
                                        </button>
                                    })}
                                </div>
                                <div class="comment__body">
                                    {present::paragraphs(&c.body).into_iter().map(|p| view! { <p>{p}</p> }).collect_view()}
                                </div>
                            </li>
                        }
                    })
                    .collect_view()}
            </ol>
        }
        .into_any()
    });
    let form = move || {
        match ctx.me.get() {
        Me::SignedIn(_) => view! {
            <form class="comment-form" on:submit=move |e| {
                e.prevent_default();
                post();
            }>
                <TextArea
                    label="Add a comment"
                    value=draft
                    max=MAX_COMMENT
                    rows=3
                    error=problem
                    node_ref=comment_box
                />
                <div class="comment-form__actions">
                    <button class="btn btn--primary" type="submit" disabled=move || posting.get()>"Post"</button>
                </div>
            </form>
        }
        .into_any(),
        Me::SignedOut => view! {
            <p class="dim">
                <button class="linkish" type="button" on:click=move |_| ctx.sign_in()>"Sign in"</button>
                " to comment and vote."
            </p>
        }
        .into_any(),
        Me::Unknown => ().into_any(),
    }
    };
    view! {
        <section class="listing__comments">
            <h2 class="section-title">{move || plural(count(), "comment")}</h2>
            {list}
            {form}
        </section>
    }
}
