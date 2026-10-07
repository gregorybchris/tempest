//! Transfers list, bundle summary, peers table and decision log.

use dioxus::prelude::*;
use proto_core::{format, Bundle, BundleState, Path};

use crate::{bool_attr, use_ui};

#[component]
pub fn Transfers() -> Element {
    let ui = use_ui();
    let world = ui.world.read();
    let selected = (ui.selected)();
    rsx! {
        aside { class: "transfers", aria_label: "Transfers",
            h2 { class: "transfers__title", "transfers" }
            ul { class: "transfer-list",
                for (i, b) in world.bundles.iter().enumerate() {
                    li {
                        button {
                            class: "transfer",
                            r#type: "button",
                            "data-index": i,
                            aria_current: bool_attr(i == selected),
                            onclick: move |_| ui.select(i),
                            span { class: "transfer__name", "{b.name}" }
                            span { class: "transfer__state {state_class(b.state)}", {b.state.label()} }
                            span { class: "transfer__bar",
                                span {
                                    class: "transfer__fill",
                                    style: "width:{b.progress * 100.0:.2}%",
                                }
                            }
                            span { class: "transfer__meta",
                                span {
                                    {format::percent(b.progress)}
                                    " of "
                                    {format::bytes(b.size_bytes)}
                                }
                                span { {transfer_rate(b)} }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn transfer_rate(b: &Bundle) -> String {
    match b.state {
        BundleState::Downloading => format!("↓ {}", format::rate(b.down_bps)),
        BundleState::Sharing => format!("↑ {}", format::rate(b.up_bps)),
        BundleState::WaitingForPeers => "0 sources".to_string(),
    }
}

fn state_class(s: BundleState) -> &'static str {
    match s {
        BundleState::Downloading => "state--downloading",
        BundleState::Sharing => "state--sharing",
        BundleState::WaitingForPeers => "state--waiting",
    }
}

#[component]
pub fn Summary() -> Element {
    let bundle = use_ui().bundle();
    let b = bundle.read();
    let eta = b
        .eta_secs()
        .map(format::duration)
        .unwrap_or_else(|| "—".into());
    rsx! {
        section { class: "summary", id: "summary",
            h1 { class: "detail__title", "{b.name}" }
            p { class: "detail__explain", {b.explanation()} }
            dl { class: "stats",
                div { dt { "progress" } dd { {format::percent(b.progress)} } }
                div { dt { "size" } dd { {format::bytes(b.size_bytes)} } }
                div { dt { "files" } dd { {format::count(b.files as u64)} } }
                div { dt { "download" } dd { {format::rate(b.down_bps)} } }
                div { dt { "upload" } dd { {format::rate(b.up_bps)} } }
                div { dt { "time left" } dd { {eta} } }
                div { dt { "ratio" } dd { "{b.ratio():.2}" } }
                div { dt { "bundle id" } dd { title: b.id.as_str(), {format::short_id(&b.id)} } }
            }
        }
    }
}

#[component]
pub fn Peers() -> Element {
    let bundle = use_ui().bundle();
    let b = bundle.read();
    rsx! {
        section { class: "panel panel--peers", id: "peers",
            div { class: "panel__head",
                h2 { class: "panel-title", "peers · {b.peers.len()}" }
            }
            if b.peers.is_empty() {
                p { class: "detail__explain", "Not connected to any peers." }
            } else {
                div { class: "table-scroll",
                    table { class: "peers",
                        thead {
                            tr {
                                th { "peer" }
                                th { "address" }
                                th { "path" }
                                th { "has" }
                                th { "wants ours" }
                                th { "we serve" }
                                th { "client" }
                                th { class: "num", "↓ from them" }
                                th { class: "num", "↑ to them" }
                            }
                        }
                        tbody {
                            for p in &b.peers {
                                tr {
                                    td { class: "peer__id", title: p.id.as_str(), {format::short_id(&p.id)} }
                                    td { class: "peer__addr", "{p.addr}" }
                                    td {
                                        class: if p.path == Path::Relayed { "path--relayed" } else { "" },
                                        {p.path.label()}
                                    }
                                    td {
                                        span { class: "peer__has", title: format::percent(p.progress),
                                            span { style: "width:{p.progress * 100.0:.1}%" }
                                        }
                                    }
                                    td { class: flag(p.wants_from_us), {yes_no(p.wants_from_us)} }
                                    td { class: flag(p.we_serve),
                                        if p.newcomer_slot {
                                            "newcomer"
                                        } else {
                                            {yes_no(p.we_serve)}
                                        }
                                    }
                                    td { class: "peer__addr", "{p.client}" }
                                    td { class: "num", {format::rate(p.down_bps)} }
                                    td { class: "num", {format::rate(p.up_bps)} }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn Log() -> Element {
    let bundle = use_ui().bundle();
    let b = bundle.read();
    rsx! {
        section { class: "panel panel--log", id: "log",
            div { class: "panel__head",
                h2 { class: "panel-title", "decisions" }
            }
            if b.decisions.is_empty() {
                p { class: "detail__explain", "Nothing to decide until a peer is found." }
            } else {
                ol { class: "log",
                    for d in b.decisions.iter().take(60) {
                        li { class: "log__entry log--{d.kind.label()}",
                            time { class: "log__time", {format::clock(d.t_ms)} }
                            span { class: "log__kind", {d.kind.label()} }
                            span { class: "log__what", "{d.what}" }
                            span { class: "log__why", "{d.why}" }
                        }
                    }
                }
            }
        }
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
