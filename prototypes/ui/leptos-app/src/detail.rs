//! The selected bundle's summary, chunk map, peers and decision log.

use leptos::prelude::*;
use proto_core::{format, Bundle, Cell, Decision, Path, Peer, GRID_COLS};

use crate::state::use_ui;

#[component]
pub fn Summary() -> impl IntoView {
    let ui = use_ui();
    let b = move |f: fn(&Bundle) -> String| move || ui.bundle(f);

    view! {
        <section class="summary" id="summary">
            <h1 class="detail__title">{b(|b| b.name.clone())}</h1>
            <p class="detail__explain">{b(Bundle::explanation)}</p>
            <dl class="stats">
                <div><dt>"progress"</dt><dd>{b(|b| format::percent(b.progress))}</dd></div>
                <div><dt>"size"</dt><dd>{b(|b| format::bytes(b.size_bytes))}</dd></div>
                <div><dt>"files"</dt><dd>{b(|b| format::count(b.files as u64))}</dd></div>
                <div><dt>"download"</dt><dd>{b(|b| format::rate(b.down_bps))}</dd></div>
                <div><dt>"upload"</dt><dd>{b(|b| format::rate(b.up_bps))}</dd></div>
                <div><dt>"time left"</dt><dd>{b(|b| b.eta_secs().map(format::duration).unwrap_or_else(|| "—".into()))}</dd></div>
                <div><dt>"ratio"</dt><dd>{b(|b| format!("{:.2}", b.ratio()))}</dd></div>
                <div><dt>"bundle id"</dt><dd title=b(|b| b.id.clone())>{b(|b| format::short_id(&b.id).to_string())}</dd></div>
            </dl>
        </section>
    }
}

#[component]
pub fn Chunks() -> impl IntoView {
    let ui = use_ui();
    let hovered = RwSignal::new(None::<usize>);

    // The chunk map is the hot path. A memo per cell would put 1,536
    // subscribers on the world signal, and re-subscribing them each tick is
    // quadratic. Instead, one effect diffs the selected bundle's cells into
    // a signal per cell, so a tick touches only the cells that changed.
    let cells: Vec<_> = untrack(|| ui.bundle(|b| b.cells.iter().map(|c| RwSignal::new(cell_attrs(c))).collect()));
    let targets = cells.clone();
    Effect::new(move |_| {
        ui.bundle(|b| {
            for (cell, attrs) in targets.iter().zip(b.cells.iter().map(cell_attrs)) {
                if cell.with_untracked(|old| *old != attrs) {
                    cell.set(attrs);
                }
            }
        })
    });
    let cells = cells
        .into_iter()
        .enumerate()
        .map(|(i, attrs)| {
            view! { <span class=move || attrs.with(|a| a.0) style=move || attrs.with(|a| a.1.clone()) data-cell=i></span> }
        })
        .collect_view();

    let readout = move || match hovered.get() {
        Some(i) => ui.bundle(|b| readout(b, i)),
        None => "hover a cell for details".to_string(),
    };

    // One delegated listener for all 1,536 cells.
    let on_over = move |e: web_sys::MouseEvent| {
        let cell = event_target::<web_sys::Element>(&e)
            .get_attribute("data-cell")
            .and_then(|s| s.parse().ok());
        if cell.is_some() {
            hovered.set(cell);
        }
    };

    view! {
        <section class="panel panel--chunks" id="chunks">
            <div class="panel__head">
                <h2 class="panel-title">"chunks"</h2>
                <span class="chunkmap__scale">
                    {move || ui.bundle(|b| format!("{} chunks · {} per cell", format::count(b.chunk_count()), format::count(b.chunks_per_cell())))}
                </span>
            </div>
            <div
                class="chunkmap"
                style=format!("--cols:{GRID_COLS}")
                role="img"
                aria-label=move || ui.bundle(|b| format!("Chunk map: {} complete", format::percent(b.progress)))
                on:mouseover=on_over
                on:mouseleave=move |_| hovered.set(None)
            >
                {cells}
            </div>
            <div class="chunkmap__foot">
                <div class="key">
                    <span class="key__item"><span class="cell cell--have"></span>"have"</span>
                    <span class="key__item"><span class="cell cell--flight"></span>"downloading"</span>
                    <span class="key__item"><span class="cell cell--missing" style="--a:1"></span>"missing, common"</span>
                    <span class="key__item"><span class="cell cell--missing" style="--a:0.15"></span>"missing, rare"</span>
                    <span class="key__item"><span class="cell cell--gone"></span>"no source"</span>
                </div>
                <span class="chunkmap__readout" id="readout">{readout}</span>
            </div>
        </section>
    }
}

/// CSS class and inline style for one chunk-map cell.
fn cell_attrs(cell: &Cell) -> (&'static str, String) {
    if cell.in_flight {
        ("cell cell--flight", String::new())
    } else if cell.have >= 1.0 {
        ("cell cell--have", String::new())
    } else if cell.have > 0.0 {
        ("cell cell--partial", format!("--p:{:.2}", cell.have))
    } else if cell.availability == 0 {
        ("cell cell--gone", String::new())
    } else {
        (
            "cell cell--missing",
            format!("--a:{:.2}", cell.availability as f32 / 12.0),
        )
    }
}

fn readout(b: &Bundle, i: usize) -> String {
    let cell = &b.cells[i];
    let (start, end) = b.cell_chunks(i);
    let state = if cell.in_flight {
        "downloading"
    } else if cell.have >= 1.0 {
        "have"
    } else {
        "missing"
    };
    let peers = match cell.availability {
        0 => "no connected peer has them".to_string(),
        a => format!("{a} of {} peers have them", b.peers.len()),
    };
    format!(
        "chunks {}–{} · {state} · {peers}",
        format::count(start),
        format::count(end.saturating_sub(1))
    )
}

#[component]
pub fn Peers() -> impl IntoView {
    let ui = use_ui();
    let ids = move || ui.bundle(|b| b.peers.iter().map(|p| p.id.clone()).collect::<Vec<_>>());

    view! {
        <section class="panel panel--peers" id="peers">
            <div class="panel__head">
                <h2 class="panel-title">"peers · " {move || ui.bundle(|b| b.peers.len())}</h2>
            </div>
            <Show
                when=move || ui.bundle(|b| !b.peers.is_empty())
                fallback=|| view! { <p class="detail__explain">"Not connected to any peers."</p> }
            >
                <div class="table-scroll">
                    <table class="peers">
                        <thead>
                            <tr>
                                <th>"peer"</th><th>"address"</th><th>"path"</th><th>"has"</th>
                                <th>"wants ours"</th><th>"we serve"</th><th>"client"</th>
                                <th class="num">"↓ from them"</th><th class="num">"↑ to them"</th>
                            </tr>
                        </thead>
                        <tbody>
                            <For each=ids key=|id| id.clone() let:id>
                                <PeerRow id />
                            </For>
                        </tbody>
                    </table>
                </div>
            </Show>
        </section>
    }
}

/// A peer's table cells, as display strings. Rows key on the peer id, and
/// each cell updates only when its own text changes.
#[derive(Clone, Default, PartialEq)]
struct PeerCells {
    addr: String,
    path: &'static str,
    path_class: &'static str,
    has: f64,
    wants: &'static str,
    wants_class: &'static str,
    serve: &'static str,
    serve_class: &'static str,
    client: String,
    down: String,
    up: String,
}

impl From<&Peer> for PeerCells {
    fn from(p: &Peer) -> Self {
        Self {
            addr: p.addr.clone(),
            path: p.path.label(),
            path_class: if p.path == Path::Relayed { "path--relayed" } else { "" },
            has: p.progress,
            wants: yes_no(p.wants_from_us),
            wants_class: flag(p.wants_from_us),
            serve: if p.newcomer_slot {
                "newcomer"
            } else {
                yes_no(p.we_serve)
            },
            serve_class: flag(p.we_serve),
            client: p.client.clone(),
            down: format::rate(p.down_bps),
            up: format::rate(p.up_bps),
        }
    }
}

#[component]
fn PeerRow(id: String) -> impl IntoView {
    let ui = use_ui();
    let peer = {
        let id = id.clone();
        Memo::new(move |_| {
            ui.bundle(|b| {
                b.peers
                    .iter()
                    .find(|p| p.id == id)
                    .map(PeerCells::from)
                    .unwrap_or_default()
            })
        })
    };
    let get = move |f: fn(&PeerCells) -> String| move || peer.with(f);

    view! {
        <tr>
            <td class="peer__id" title=id.clone()>{format::short_id(&id).to_string()}</td>
            <td class="peer__addr">{get(|p| p.addr.clone())}</td>
            <td class=move || peer.with(|p| p.path_class)>{move || peer.with(|p| p.path)}</td>
            <td>
                <span class="peer__has" title=move || peer.with(|p| format::percent(p.has))>
                    <span style=get(|p| format!("width:{:.1}%", p.has * 100.0))></span>
                </span>
            </td>
            <td class=move || peer.with(|p| p.wants_class)>{move || peer.with(|p| p.wants)}</td>
            <td class=move || peer.with(|p| p.serve_class)>{move || peer.with(|p| p.serve)}</td>
            <td class="peer__addr">{get(|p| p.client.clone())}</td>
            <td class="num">{get(|p| p.down.clone())}</td>
            <td class="num">{get(|p| p.up.clone())}</td>
        </tr>
    }
}

fn flag(on: bool) -> &'static str {
    if on {
        "flag--on"
    } else {
        "flag--off"
    }
}

fn yes_no(on: bool) -> &'static str {
    if on {
        "yes"
    } else {
        "no"
    }
}

#[component]
pub fn Log() -> impl IntoView {
    let ui = use_ui();
    // Entries are immutable once logged, so key each by bundle, time, and rank
    // among entries logged at the same time. New entries insert at the top
    // without re-rendering the rest.
    let entries = move || {
        let bundle = ui.selected.get();
        ui.bundle(|b| {
            let mut rank = 0;
            let mut prev = None;
            b.decisions
                .iter()
                .take(60)
                .map(|d| {
                    rank = if prev == Some(d.t_ms) { rank + 1 } else { 0 };
                    prev = Some(d.t_ms);
                    ((bundle, d.t_ms, rank), d.clone())
                })
                .collect::<Vec<_>>()
        })
    };

    view! {
        <section class="panel panel--log" id="log">
            <div class="panel__head"><h2 class="panel-title">"decisions"</h2></div>
            <Show
                when=move || ui.bundle(|b| !b.decisions.is_empty())
                fallback=|| view! { <p class="detail__explain">"Nothing to decide until a peer is found."</p> }
            >
                <ol class="log">
                    <For each=entries key=|(key, _)| *key children=|(_, d)| view! { <LogEntry d /> } />
                </ol>
            </Show>
        </section>
    }
}

#[component]
fn LogEntry(d: Decision) -> impl IntoView {
    let kind = d.kind.label();
    view! {
        <li class=format!("log__entry log--{kind}")>
            <time class="log__time">{format::clock(d.t_ms)}</time>
            <span class="log__kind">{kind}</span>
            <span class="log__what">{d.what}</span>
            <span class="log__why">{d.why}</span>
        </li>
    }
}
