//! Browsing: the search, the category and tag facets, the sorts, the results and the pager.
//! Everything the page shows follows the address, so every search has a link.

use leptos::prelude::*;
use tempest_tracker_api::listing::{FacetCount, Listing, ListingPage};
use tempest_tracker_api::query::{ListingQuery, Sort};
use tempest_ui_kit::components::*;
use tempest_ui_kit::format;
use tempest_ui_kit::shell::{debounce, synced};
use wasm_bindgen::JsCast;

use crate::app::{on_command, use_ctx};
use crate::keys::Command;
use crate::present::{self, PageButton, plural};
use crate::route::Route;
use crate::views::{HealthBars, Problem, fetch};

/// The listings, searched, filtered, sorted and paged.
#[component]
pub fn BrowsePage() -> impl IntoView {
    let ctx = use_ctx();
    let query = Memo::new(move |_| match ctx.route.get() {
        Route::Browse(query) => query,
        _ => ListingQuery::default(),
    });
    // Changes start again from the first page, and replace the history entry so that back
    // leaves the search rather than undoing it a letter at a time.
    let edit = move |f: &dyn Fn(&mut ListingQuery)| {
        let mut next = query.get_untracked();
        f(&mut next);
        next.offset = 0;
        ctx.replace(Route::Browse(next));
    };
    let text = synced(
        Memo::new(move |_| query.with(|q| q.q.clone())),
        move |typed: String| debounce(move || edit(&|q| q.q = typed.clone())),
    );
    let sort = synced(Memo::new(move |_| query.with(|q| q.sort)), move |chosen| {
        edit(&|q| q.sort = chosen);
    });
    let (page, loading) =
        fetch::<ListingPage>(move || format!("listings?{}", query.get().to_query()));

    // The cursor for `j`, `k` and `enter`, back at the top whenever the results change.
    let cursor = RwSignal::new(None::<usize>);
    Effect::new(move |_| {
        page.track();
        cursor.set(None);
    });
    let shown = move || match page.get() {
        Some(Ok(page)) => page.listings.iter().map(|l| l.bundle).collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    let total = move || match page.get() {
        Some(Ok(page)) => page.total,
        _ => 0,
    };
    let turn = move |step: i64| {
        let (offset, limit) = query.with_untracked(|q| (q.offset as i64, i64::from(q.limit)));
        let next = offset + step * limit;
        if next >= 0 && (next as u64) < total() {
            let mut q = query.get_untracked();
            q.offset = next as u64;
            ctx.go(Route::Browse(q));
        }
    };
    on_command(ctx, move |command| {
        let count = shown().len();
        match command {
            Command::Next if count > 0 => {
                cursor.update(|c| *c = Some(c.map_or(0, |i| (i + 1).min(count - 1))));
                reveal(cursor.get_untracked());
            }
            Command::Previous if count > 0 => {
                cursor.update(|c| *c = Some(c.map_or(0, |i| i.saturating_sub(1))));
                reveal(cursor.get_untracked());
            }
            Command::Open => {
                if let Some(bundle) = cursor.get_untracked().and_then(|i| shown().get(i).copied()) {
                    ctx.go(Route::Listing(bundle));
                }
            }
            Command::NextPage => turn(1),
            Command::PreviousPage => turn(-1),
            Command::NextSort => {
                let i = Sort::ALL
                    .iter()
                    .position(|s| *s == sort.get_untracked())
                    .unwrap_or(0);
                sort.set(Sort::ALL[(i + 1) % Sort::ALL.len()]);
            }
            _ => {}
        }
    });

    let sorts: Vec<(Sort, &'static str)> = Sort::ALL
        .iter()
        .map(|s| (*s, present::sort_label(*s)))
        .collect();
    let summary = move || match page.get() {
        Some(Ok(page)) => {
            let offset = query.with(|q| q.offset);
            present::range(offset, page.listings.len(), page.total)
        }
        _ => String::new(),
    };
    view! {
        <section class="browse">
            <div class="browse__search">
                <label class="bigsearch">
                    <span class="bigsearch__prompt" aria-hidden="true">">"</span>
                    <input
                        node_ref=ctx.search
                        class="bigsearch__input"
                        type="search"
                        placeholder="search bundles…"
                        aria-label="Search listings"
                        spellcheck="false"
                        autocomplete="off"
                        prop:value=move || text.get()
                        on:input=move |e| text.set(event_target_value(&e))
                        on:keydown=move |e| {
                            // Down from the search moves into the results.
                            if e.key() == "ArrowDown" && !shown().is_empty() {
                                e.prevent_default();
                                if let Some(input) = ctx.search.get_untracked() {
                                    let _ = input.blur();
                                }
                                cursor.set(Some(0));
                                reveal(Some(0));
                            }
                        }
                    />
                    <span class="bigsearch__busy" class:bigsearch__busy--on=move || loading.get() aria-hidden="true"></span>
                    <kbd class="bigsearch__key" aria-hidden="true">"/"</kbd>
                </label>
                <div class="browse__bar">
                    <span class="browse__count dim" aria-live="polite">{summary}</span>
                    <Segmented label="Sort by" options=sorts value=sort />
                </div>
            </div>
            <div class="browse__body">
                <Facets query page />
                <div class="browse__results" class:browse__results--stale=move || loading.get()>
                    <ActiveFilters query />
                    {move || match page.get() {
                        None => view! { <p class="loading dim">"loading…"</p> }.into_any(),
                        Some(Err(error)) => view! { <Problem error /> }.into_any(),
                        Some(Ok(found)) => view! { <Results found query cursor turn /> }.into_any(),
                    }}
                </div>
            </div>
        </section>
    }
}

/// Scrolls the result under the cursor into view.
fn reveal(index: Option<usize>) {
    let Some(index) = index else { return };
    request_animation_frame(move || {
        let found = document()
            .get_element_by_id(&format!("result-{index}"))
            .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok());
        if let Some(element) = found {
            let options = web_sys::ScrollIntoViewOptions::new();
            options.set_block(web_sys::ScrollLogicalPosition::Nearest);
            element.scroll_into_view_with_scroll_into_view_options(&options);
        }
    });
}

/// The categories and tags among the matches, each a link that narrows the search.
#[component]
fn Facets(query: Memo<ListingQuery>, page: crate::views::Fetched<ListingPage>) -> impl IntoView {
    let ctx = use_ctx();
    // The last facets stay while a new answer is on its way, so the column doesn't jump.
    let facets = Memo::new(
        move |previous: Option<&tempest_tracker_api::listing::Facets>| match page.get() {
            Some(Ok(page)) => page.facets,
            _ => previous.cloned().unwrap_or_default(),
        },
    );
    let with = move |f: &dyn Fn(&mut ListingQuery)| {
        let mut q = query.get_untracked();
        f(&mut q);
        q.offset = 0;
        Route::Browse(q).hash()
    };
    let categories = move || {
        let chosen = query.with(|q| q.category.clone());
        let counts = facets.with(|f| f.categories.clone());
        let all = with(&|q| q.category = None);
        let mut rows = vec![
            view! {
                <li>
                    <a class="facet" href=all aria-current=chosen.is_none().then_some("true")>
                        <span class="facet__name">"Everything"</span>
                    </a>
                </li>
            }
            .into_any(),
        ];
        // The categories with matches, and the one chosen even if it has none.
        let mut shown: Vec<FacetCount> = counts;
        if let Some(slug) = &chosen
            && !shown.iter().any(|c| &c.value == slug)
        {
            shown.push(FacetCount {
                value: slug.clone(),
                count: 0,
            });
        }
        rows.extend(shown.into_iter().map(|facet| {
            let slug = facet.value.clone();
            let on = chosen.as_deref() == Some(slug.as_str());
            let href = with(&|q| q.category = (!on).then(|| slug.clone()));
            view! {
                <li>
                    <a class="facet" href=href aria-current=on.then_some("true")>
                        <span class="facet__name">{ctx.category_name(&facet.value)}</span>
                        <span class="facet__count num">{format::count(facet.count)}</span>
                    </a>
                </li>
            }
            .into_any()
        }));
        rows
    };
    let tags =
        move || {
            let chosen = query.with(|q| q.tags.clone());
            facets.with(|f| f.tags.clone()).into_iter().map(|facet| {
            let tag = facet.value.clone();
            let on = chosen.contains(&tag);
            let href = with(&|q| match on {
                true => q.tags.retain(|t| *t != tag),
                false => q.tags.push(tag.clone()),
            });
            view! {
                <a class="tagfacet" href=href aria-pressed=if on { "true" } else { "false" }>
                    {facet.value}
                    <span class="tagfacet__count num">{format::count(facet.count)}</span>
                </a>
            }
        }).collect_view()
        };
    let filtered = move || query.with(|q| q.category.is_some() || !q.tags.is_empty());
    // Open beside the results; folded above them on a phone, where the CSS shows the toggle.
    let wide = window()
        .inner_width()
        .ok()
        .and_then(|w| w.as_f64())
        .is_some_and(|w| w >= 760.0);
    view! {
        <details class="facets" open=wide>
            <summary class="facets__toggle">
                "Filters"
                <Show when=filtered>
                    <span class="facets__on">" · on"</span>
                </Show>
            </summary>
            <div class="facets__body">
                <h2 class="facets__title">"Categories"</h2>
                <ul class="facets__list">{categories}</ul>
                <Show when=move || facets.with(|f| !f.tags.is_empty())>
                    <h2 class="facets__title">"Tags"</h2>
                    <div class="facets__tags">{tags}</div>
                </Show>
            </div>
        </details>
    }
}

/// The category and tags chosen, each removable, and a way to clear them all.
#[component]
fn ActiveFilters(query: Memo<ListingQuery>) -> impl IntoView {
    let ctx = use_ctx();
    let edit = move |f: Box<dyn Fn(&mut ListingQuery)>| {
        let mut q = query.get_untracked();
        f(&mut q);
        q.offset = 0;
        ctx.go(Route::Browse(q));
    };
    move || {
        let q = query.get();
        if q.category.is_none() && q.tags.is_empty() {
            return ().into_any();
        }
        let category = q.category.clone().map(|slug| {
            let label = format!("in {}", ctx.category_name(&slug));
            view! { <Chip label on_remove=move || edit(Box::new(|q| q.category = None)) /> }
        });
        let tags = q
            .tags
            .iter()
            .map(|tag| {
                let gone = tag.clone();
                view! {
                    <Chip
                        label=format!("#{tag}")
                        on_remove=move || {
                            let gone = gone.clone();
                            edit(Box::new(move |q| q.tags.retain(|t| *t != gone)));
                        }
                    />
                }
            })
            .collect_view();
        view! {
            <div class="filters" aria-label="Filters">
                {category}
                {tags}
                <button
                    class="btn btn--ghost btn--small"
                    type="button"
                    on:click=move |_| edit(Box::new(|q| {
                        q.category = None;
                        q.tags.clear();
                    }))
                >
                    "clear filters"
                </button>
            </div>
        }
        .into_any()
    }
}

/// A page of results and its pager.
#[component]
fn Results(
    found: ListingPage,
    query: Memo<ListingQuery>,
    cursor: RwSignal<Option<usize>>,
    turn: impl Fn(i64) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let q = query.get_untracked();
    if found.listings.is_empty() {
        let text = match (q.q.is_empty(), q.category.is_some() || !q.tags.is_empty()) {
            (true, false) => {
                "Nothing is listed here yet. Sharers list their bundles with tempest share --publish."
            }
            (_, true) => "Nothing matches with these filters. Try clearing some.",
            (false, false) => "Nothing matches. Check the spelling, or try fewer or other words.",
        };
        return view! { <Empty title="No listings" text=text.to_owned() /> }.into_any();
    }
    let selected = Selector::new(move || cursor.get());
    let words = q.q.clone();
    let rows = found
        .listings
        .into_iter()
        .enumerate()
        .map(|(i, listing)| {
            let selected = selected.clone();
            view! { <ResultCard index=i listing words=words.clone() selected /> }
        })
        .collect_view();
    let buttons = present::pages(q.offset, q.limit, found.total);
    let limit = u64::from(q.limit);
    let current = q.offset / limit.max(1);
    let pager = (!buttons.is_empty()).then(|| {
        let numbers = buttons
            .into_iter()
            .map(|button| match button {
                PageButton::Gap => view! { <span class="pager__gap dim">"…"</span> }.into_any(),
                PageButton::Page(n) => {
                    let mut target = q.clone();
                    target.offset = n * limit;
                    view! {
                        <a
                            class="pager__page num"
                            href=Route::Browse(target).hash()
                            aria-current=(n == current).then_some("page")
                        >
                            {n + 1}
                        </a>
                    }
                    .into_any()
                }
            })
            .collect_view();
        let last = found.total.div_ceil(limit.max(1)).saturating_sub(1);
        view! {
            <nav class="pager" aria-label="Pages">
                <Button small=true title="Previous page ([)" disabled=Signal::stored(current == 0) on_click=move || turn(-1)>"←"</Button>
                {numbers}
                <Button small=true title="Next page (])" disabled=Signal::stored(current >= last) on_click=move || turn(1)>"→"</Button>
            </nav>
        }
    });
    view! {
        <ol class="results">{rows}</ol>
        {pager}
    }
    .into_any()
}

/// One listing among the results.
#[component]
fn ResultCard(
    index: usize,
    listing: Listing,
    words: String,
    selected: Selector<Option<usize>>,
) -> impl IntoView {
    let ctx = use_ctx();
    let href = Route::Listing(listing.bundle).hash();
    let title = match listing.title.is_empty() {
        true => "untitled".to_owned(),
        false => listing.title.clone(),
    };
    let title = present::highlight(&title, &words)
        .into_iter()
        .map(|(run, mark)| match mark {
            true => view! { <mark>{run}</mark> }.into_any(),
            false => run.into_any(),
        })
        .collect_view();
    let excerpt = present::excerpt(&listing.description, 220);
    let category = listing.category.as_deref().map(|slug| {
        let q = ListingQuery {
            category: Some(slug.to_owned()),
            ..ListingQuery::default()
        };
        view! { <a class="result__category" href=Route::Browse(q).hash()>{ctx.category_name(slug)}</a> }
    });
    let content = listing
        .content
        .as_ref()
        .map(|c| format!("{} · {}", format::bytes(c.size), plural(c.files, "file")));
    let tags = listing
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
    let swarm = listing.swarm;
    let now = ctx.now;
    let created = listing.created;
    let bundle = listing.bundle;
    view! {
        <li
            id=format!("result-{index}")
            class="result"
            class:result--cursor=move || selected.selected(&Some(index))
            on:click=move |e| {
                // A click anywhere on the card opens it, except on its own links.
                let on_link = e
                    .target()
                    .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                    .and_then(|t| t.closest("a").ok().flatten())
                    .is_some();
                if !on_link {
                    ctx.go(Route::Listing(bundle));
                }
            }
        >
            <div class="result__main">
                <a class="result__title" href=href>{title}</a>
                <div class="result__meta dim">
                    {category}
                    {content.map(|c| view! { <span>{c}</span> })}
                    <span>"by "{present::person(&listing.publisher)}</span>
                    <span>{move || present::ago(now.get(), created)}</span>
                </div>
                {(!excerpt.is_empty()).then(|| view! { <p class="result__excerpt">{excerpt}</p> })}
                {(!listing.tags.is_empty()).then(|| view! { <div class="result__tags">{tags}</div> })}
            </div>
            <dl class="result__stats">
                <div>
                    <dt>"health"</dt>
                    <dd><HealthBars swarm /></dd>
                </div>
                <div title="Peers with the whole bundle">
                    <dt>"sharing"</dt>
                    <dd class="num tone--up">{format::count(swarm.complete)}</dd>
                </div>
                <div title="Peers still downloading it">
                    <dt>"downloading"</dt>
                    <dd class="num tone--down">{format::count(swarm.incomplete)}</dd>
                </div>
                <div title="Votes up less votes down">
                    <dt>"votes"</dt>
                    <dd class="num">{present::score(listing.votes)}</dd>
                </div>
                <div title="Completed downloads">
                    <dt>"downloads"</dt>
                    <dd class="num">{format::count(listing.downloads)}</dd>
                </div>
            </dl>
        </li>
    }
}
