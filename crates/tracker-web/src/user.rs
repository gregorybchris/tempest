//! An account's page (D-262): who it is, which is its key and a glyph made from it, what it
//! has listed, and what it has said. Every name in the website links here, because a name can
//! look like another's and a key can't.

use leptos::prelude::*;
use tempest_protocol::PeerId;
use tempest_tracker_api::account::{Count, Profile, ProfileComment};
use tempest_tracker_api::listing::{ListingPage, MAX_PAGE};
use tempest_tracker_api::query::ListingQuery;
use tempest_ui_kit::components::*;
use tempest_ui_kit::format;

use crate::app::use_ctx;
use crate::browse::ResultCard;
use crate::present;
use crate::request;
use crate::route::Route;
use crate::views::{Crumbs, Problem, fetch, shown};

/// The listings shown at first, and added by each "show more".
const LISTINGS_STEP: u32 = 10;

/// How many comments are shown.
const COMMENTS_SHOWN: u32 = 10;

/// How long a comment may be before it is cut, in characters.
const COMMENT_EXCERPT: usize = 240;

/// The account `key`'s page.
#[component]
pub fn UserPage(key: PeerId) -> impl IntoView {
    let (fetched, _) = fetch::<Profile>(move || format!("accounts/{key}"));
    move || {
        match fetched.get() {
        None => view! { <p class="page loading dim">"loading…"</p> }.into_any(),
        Some(Err(error)) if request::not_found(&error) => view! {
            <section class="page">
                <Empty title="No such account" text="This tracker has no account with that key.".to_owned()>
                    <a class="btn" href=Route::default().hash()>"Back to the listings"</a>
                </Empty>
            </section>
        }
        .into_any(),
        Some(Err(error)) => view! { <section class="page"><Problem error /></section> }.into_any(),
        Some(Ok(profile)) => view! { <Account profile /> }.into_any(),
    }
    }
}

#[component]
fn Account(profile: Profile) -> impl IntoView {
    let ctx = use_ctx();
    let key = profile.key;
    let title = profile
        .name
        .clone()
        .unwrap_or_else(|| present::short_key(&key.to_string()));
    let own = move || ctx.me.with(|me| me.account().is_some_and(|a| a.key == key));
    let joined = profile.joined;
    let banned = profile.banned;
    let steps = vec![("Listings".to_owned(), Route::default().hash())];
    view! {
        <article class="page user">
            <header class="page__head">
                <Crumbs steps here=title.clone() />
            </header>
            <section class="user__card">
                <Glyph key />
                <div class="user__id">
                    <h1 class="page__title glow">{title}</h1>
                    <p class="user__sub dim">
                        {match profile.name {
                            Some(_) => "",
                            None => "No name chosen. ",
                        }}
                        {move || format!("Joined {}.", present::ago(ctx.now.get(), joined))}
                        {move || own().then(|| view! {
                            " "<Badge tone=Tone::Down label="you" />
                            " "<a href=Route::Me.hash()>"change your name"</a>
                        })}
                        {banned.then(|| view! { " "<Badge tone=Tone::Bad label="banned" /> })}
                    </p>
                    <KeyLine key />
                </div>
            </section>
            {match banned {
                true => view! {
                    <p class="notice">"The operator banned this key. Its listings and comments are hidden."</p>
                }
                .into_any(),
                false => view! { <Details profile /> }.into_any(),
            }}
        </article>
    }
}

/// The figures, the listings, and what the account says and publishes in.
#[component]
fn Details(profile: Profile) -> impl IntoView {
    let key = profile.key;
    let stats = profile.stats;
    let shown_count = RwSignal::new(LISTINGS_STEP);
    let (listings, _) = fetch::<ListingPage>(move || {
        format!("accounts/{key}/listings?limit={}", shown_count.get())
    });
    // Whether it shares anything now is known once all of its listings are in hand.
    let sharing = Memo::new(move |_| match listings.get() {
        Some(Ok(page)) if page.listings.len() as u64 == page.total => Some((
            page.listings
                .iter()
                .filter(|l| l.swarm.complete > 0)
                .count() as u64,
            page.total,
        )),
        _ => None,
    });
    let score = tempest_tracker_api::listing::VoteCounts {
        up: stats.votes_up,
        down: stats.votes_down,
    };
    view! {
        <dl class="stats user__stats">
            <div title="Listings that show">
                <dt>"listings"</dt>
                <dd class="num">{format::count(stats.listings)}</dd>
            </div>
            <div title="Completed downloads of its listings">
                <dt>"downloads"</dt>
                <dd class="num">{format::count(stats.downloads)}</dd>
            </div>
            <div title=format!("{} up, {} down", stats.votes_up, stats.votes_down)>
                <dt>"votes"</dt>
                <dd class="num">{present::score(score)}</dd>
            </div>
            <div title="Comments on listings that show">
                <dt>"comments"</dt>
                <dd class="num">{format::count(stats.comments)}</dd>
            </div>
            {move || sharing.get().map(|(now, of)| view! {
                <div title="Listings with at least one peer that has everything">
                    <dt>"sharing now"</dt>
                    <dd class="num" class:tone--up=(now > 0)>{format!("{now} of {of}")}</dd>
                </div>
            })}
        </dl>
        <div class="user__grid">
            <section class="user__main">
                <h2 class="section-title">"Listings"</h2>
                {shown(listings, move |page: ListingPage| {
                    if page.listings.is_empty() {
                        return view! { <Empty title="Nothing listed" text="Nothing from this account shows right now.".to_owned() /> }.into_any();
                    }
                    let more = (page.listings.len() as u64) < page.total;
                    let selected = Selector::new(|| None::<usize>);
                    let rows = page
                        .listings
                        .into_iter()
                        .enumerate()
                        .map(|(index, listing)| {
                            let selected = selected.clone();
                            view! { <ResultCard index listing words=String::new() selected byline=false /> }
                        })
                        .collect_view();
                    view! {
                        <ol class="results">{rows}</ol>
                        {more.then(|| view! {
                            <div class="user__more">
                                <Button on_click=move || shown_count.update(|n| *n = (*n + LISTINGS_STEP).min(MAX_PAGE))>
                                    "show more"
                                </Button>
                            </div>
                        })}
                    }
                    .into_any()
                })}
            </section>
            <aside class="user__side">
                <Publishes categories=profile.categories tags=profile.tags />
                <Said key total=stats.comments />
            </aside>
        </div>
    }
}

/// The categories and tags an account publishes in, each a link to browse them.
#[component]
fn Publishes(categories: Vec<Count>, tags: Vec<Count>) -> impl IntoView {
    let ctx = use_ctx();
    if categories.is_empty() && tags.is_empty() {
        return ().into_any();
    }
    let (has_categories, has_tags) = (!categories.is_empty(), !tags.is_empty());
    let categories = categories
        .into_iter()
        .map(|c| {
            let query = ListingQuery {
                category: Some(c.name.clone()),
                ..ListingQuery::default()
            };
            view! {
                <a class="user__chip user__chip--category" href=Route::Browse(query).hash()>
                    {ctx.category_name(&c.name)}<span class="dim">{c.count}</span>
                </a>
            }
        })
        .collect_view();
    let tags = tags
        .into_iter()
        .map(|t| {
            let query = ListingQuery {
                tags: vec![t.name.clone()],
                ..ListingQuery::default()
            };
            view! {
                <a class="user__chip" href=Route::Browse(query).hash()>
                    {format!("#{}", t.name)}<span class="dim">{t.count}</span>
                </a>
            }
        })
        .collect_view();
    view! {
        <Panel title="Publishes in">
            {has_categories.then(|| view! {
                <h3 class="user__label">"categories"</h3>
                <div class="user__chips">{categories}</div>
            })}
            {has_tags.then(|| view! {
                <h3 class="user__label">"tags"</h3>
                <div class="user__chips">{tags}</div>
            })}
        </Panel>
    }
    .into_any()
}

/// The newest comments an account wrote, each with the listing it is on.
#[component]
fn Said(key: PeerId, total: u64) -> impl IntoView {
    let ctx = use_ctx();
    let (fetched, _) = fetch::<Vec<ProfileComment>>(move || {
        format!("accounts/{key}/comments?limit={COMMENTS_SHOWN}")
    });
    let title = move || format!("Comments ({})", format::count(total));
    view! {
        <Panel title=Signal::derive(title)>
            {shown(fetched, move |comments: Vec<ProfileComment>| {
                if comments.is_empty() {
                    return view! { <p class="dim">"No comments yet."</p> }.into_any();
                }
                let cut = (comments.len() as u64) < total;
                view! {
                    <ol class="said">
                        {comments
                            .into_iter()
                            .map(|c| {
                                let created = c.comment.created;
                                view! {
                                    <li class="said__item">
                                        <div class="said__head">
                                            <a href=Route::Listing(c.listing.bundle).hash()>{c.listing.title}</a>
                                            <span class="dim">{move || present::ago(ctx.now.get(), created)}</span>
                                        </div>
                                        <p class="said__body">{present::excerpt(&c.comment.body, COMMENT_EXCERPT)}</p>
                                    </li>
                                }
                            })
                            .collect_view()}
                    </ol>
                    {cut.then(|| view! {
                        <p class="note dim">{format!("The newest {COMMENTS_SHOWN} of {total}.")}</p>
                    })}
                }
                .into_any()
            })}
        </Panel>
    }
}

/// The key in groups of eight characters, to read and compare, with a button that copies it.
#[component]
fn KeyLine(key: PeerId) -> impl IntoView {
    let ctx = use_ctx();
    let hex = key.to_string();
    let groups = key_groups(&hex)
        .into_iter()
        .map(|group| view! { <span class="user__group">{group}</span> })
        .collect_view();
    view! {
        <div class="user__key">
            <code class="user__hex" title="This account's whole key">{groups}</code>
            <button
                class="btn btn--small"
                type="button"
                aria-label="Copy the key"
                on:click=move |_| ctx.copy(hex.clone(), "key")
            >
                "copy"
            </button>
        </div>
        <p class="note dim">"A name can look like another. A key can't."</p>
    }
}

/// The key's hexadecimal text in groups of eight characters.
fn key_groups(hex: &str) -> Vec<String> {
    hex.as_bytes()
        .chunks(8)
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect()
}

/// The glyph's cells: 8 rows of 8, mirrored left to right, from the first four bytes of the key.
/// Cell `(row, column)` is on if the bit `row * 4 + min(column, 7 - column)` of those bytes is.
fn glyph_cells(key: &[u8; 32]) -> [[bool; 8]; 8] {
    let bits = u32::from_be_bytes([key[0], key[1], key[2], key[3]]);
    let mut cells = [[false; 8]; 8];
    for (row, line) in cells.iter_mut().enumerate() {
        for (column, cell) in line.iter_mut().enumerate() {
            let half = column.min(7 - column);
            *cell = bits >> (row * 4 + half) & 1 == 1;
        }
    }
    cells
}

/// The glyph's hue, in degrees, from the next two bytes of the key.
fn glyph_hue(key: &[u8; 32]) -> u16 {
    u16::from_be_bytes([key[4], key[5]]) % 360
}

/// An SVG path that fills the glyph's cells, in a box 8 wide and 8 high.
fn glyph_path(key: &[u8; 32]) -> String {
    let mut path = String::new();
    for (row, line) in glyph_cells(key).iter().enumerate() {
        for (column, on) in line.iter().enumerate() {
            if *on {
                path.push_str(&format!("M{column} {row}h1v1h-1z"));
            }
        }
    }
    path
}

/// A small picture made from a key, the same for the same key everywhere: a tell at a glance
/// that two accounts with one name are not the same account.
#[component]
fn Glyph(key: PeerId) -> impl IntoView {
    let bytes = *key.as_bytes();
    let path = glyph_path(&bytes);
    let style = format!("--glyph-hue: {}", glyph_hue(&bytes));
    view! {
        <svg
            class="glyph"
            style=style
            viewBox="0 0 8 8"
            shape-rendering="crispEdges"
            role="img"
            aria-label="A picture made from this account's key"
        >
            <path d=path />
        </svg>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_glyph_is_mirrored_and_depends_on_its_key() {
        let key = [0xa5; 32];
        for line in glyph_cells(&key) {
            for column in 0..4 {
                assert_eq!(line[column], line[7 - column]);
            }
        }
        let mut other = key;
        other[0] ^= 1;
        assert_ne!(glyph_cells(&key), glyph_cells(&other));
        assert_eq!(glyph_cells(&key), glyph_cells(&key));
        assert!(glyph_hue(&key) < 360);
    }

    #[test]
    fn a_glyph_path_has_a_square_per_cell() {
        let mut key = [0; 32];
        assert_eq!(glyph_path(&key), "");
        key[3] = 1;
        // Bit 0 is row 0, column 0 and its mirror, column 7.
        assert_eq!(glyph_path(&key), "M0 0h1v1h-1zM7 0h1v1h-1z");
    }

    #[test]
    fn keys_are_read_in_eights() {
        let hex = "0123456789abcdef".repeat(4);
        let groups = key_groups(&hex);
        assert_eq!(groups.len(), 8);
        assert_eq!(groups[0], "01234567");
    }
}
