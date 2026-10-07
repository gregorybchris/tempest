//! The chunk map: 1,536 cells and a hover readout.

use dioxus::prelude::*;
use dioxus::web::WebEventExt;
use proto_core::{format, Bundle, Cell, GRID_COLS};

use crate::use_ui;

#[component]
pub fn Chunks() -> Element {
    let ui = use_ui();
    // Written here, read only by `Readout`, so hovering doesn't re-render the map.
    let mut hovered = use_signal(|| None::<usize>);
    let bundle = ui.bundle();
    let b = bundle.read();
    rsx! {
        section { class: "panel panel--chunks", id: "chunks",
            div { class: "panel__head",
                h2 { class: "panel-title", "chunks" }
                span { class: "chunkmap__scale",
                    {format::count(b.chunk_count())}
                    " chunks · "
                    {format::count(b.chunks_per_cell())}
                    " per cell"
                }
            }
            div {
                class: "chunkmap",
                style: "--cols:{GRID_COLS}",
                role: "img",
                aria_label: "Chunk map: {format::percent(b.progress)} complete",
                // One delegated listener instead of 1,536 per-cell handlers.
                onmouseover: move |e| {
                    if let Some(i) = cell_index(&e) {
                        hovered.set(Some(i));
                    }
                },
                onmouseleave: move |_| hovered.set(None),
                for (i, (class, style)) in b.cells.iter().map(cell_attrs).enumerate() {
                    span { class, style, "data-cell": i }
                }
            }
            div { class: "chunkmap__foot",
                div { class: "key",
                    span { class: "key__item", span { class: "cell cell--have" } "have" }
                    span { class: "key__item", span { class: "cell cell--flight" } "downloading" }
                    span { class: "key__item",
                        span { class: "cell cell--missing", style: "--a:1" }
                        "missing, common"
                    }
                    span { class: "key__item",
                        span { class: "cell cell--missing", style: "--a:0.15" }
                        "missing, rare"
                    }
                    span { class: "key__item", span { class: "cell cell--gone" } "no source" }
                }
                Readout { hovered }
            }
        }
    }
}

#[component]
fn Readout(hovered: ReadSignal<Option<usize>>) -> Element {
    let ui = use_ui();
    let bundle = ui.bundle();
    let text = match hovered() {
        Some(i) => describe(&bundle.read(), i),
        None => "hover a cell for details".to_string(),
    };
    rsx! {
        span { class: "chunkmap__readout", id: "readout", {text} }
    }
}

/// The `data-cell` index of the element under the pointer.
fn cell_index(e: &MouseEvent) -> Option<usize> {
    let target: web_sys::Element =
        wasm_bindgen::JsCast::dyn_into(e.as_web_event().target()?).ok()?;
    target.get_attribute("data-cell")?.parse().ok()
}

fn describe(b: &Bundle, i: usize) -> String {
    let cell = b.cells[i];
    let (start, end) = b.cell_chunks(i);
    let state = if cell.in_flight {
        "downloading"
    } else if cell.have >= 1.0 {
        "have"
    } else {
        "missing"
    };
    let sources = match cell.availability {
        0 => "no connected peer has them".to_string(),
        n => format!("{n} of {} peers have them", b.peers.len()),
    };
    format!(
        "chunks {}–{} · {state} · {sources}",
        format::count(start),
        format::count(end - 1)
    )
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
