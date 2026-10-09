//! The benchmark of a 1,536-cell live view that guards the Leptos guidelines (specs/interfaces.md).
//!
//! It mounts the chunk map in a real browser and times updates that each change an eighth of
//! its cells, from the change until the browser is free again, so the time covers the effects
//! and the DOM writes. In a debug build, one memo per cell over a shared signal, the pattern the
//! guidelines forbid, took about 55 ms an update here; the chunk map's diffing took about 2.
//!
//! Run it with `wasm-bindgen-test-runner` as the wasm32 runner and a chromedriver; CI's `web`
//! job shows how.

use leptos::prelude::*;
use tempest_ui_kit::{ChunkCell, ChunkMap};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::{console_log, wasm_bindgen_test};

const CELLS: usize = 1536;
const UPDATES: usize = 60;
/// The most an update may take on average, in milliseconds: a frame at 60 Hz. Debug builds,
/// which CI runs, are several times slower than release ones.
const BUDGET_MS: f64 = 16.7;

#[wasm_bindgen_test]
async fn chunk_map_updates_1536_cells_within_a_frame() {
    let missing = ChunkCell {
        missing: 4,
        availability: 3,
        ..ChunkCell::default()
    };
    let cells = RwSignal::new(vec![missing; CELLS]);
    let host: web_sys::HtmlElement = document().create_element("div").unwrap().unchecked_into();
    document().body().unwrap().append_child(&host).unwrap();
    let _mounted = mount_to(
        host.clone(),
        move || view! { <ChunkMap cells running=true /> },
    );
    let idle = Idle::new();
    idle.wait().await;
    assert_eq!(count(&host, ".chunkmap .cell"), CELLS);

    let performance = window().performance().unwrap();
    let mut slowest = 0.0_f64;
    let start = performance.now();
    for round in 0..UPDATES {
        let before = performance.now();
        // Every eighth cell, offset by the round, flips between stored and missing.
        cells.update(|cells| {
            for cell in cells.iter_mut().skip(round % 8).step_by(8) {
                *cell = if cell.have > 0 {
                    missing
                } else {
                    ChunkCell {
                        have: 4,
                        ..ChunkCell::default()
                    }
                };
            }
        });
        idle.wait().await;
        slowest = slowest.max(performance.now() - before);
    }
    let mean = (performance.now() - start) / UPDATES as f64;
    console_log!(
        "chunk map, {CELLS} cells: {mean:.2} ms an update on average, {slowest:.2} ms at most"
    );

    let stored = cells.with_untracked(|c| c.iter().filter(|c| c.have > 0).count());
    assert_eq!(
        count(&host, ".chunkmap .cell--have"),
        stored,
        "the DOM matches the data"
    );
    assert!(
        mean < BUDGET_MS,
        "{mean:.2} ms an update, over the {BUDGET_MS} ms budget"
    );
}

/// The number of elements under `host` matching `selector`.
fn count(host: &web_sys::HtmlElement, selector: &str) -> usize {
    host.query_selector_all(selector).unwrap().length() as usize
}

/// Waits until the browser has run everything queued, effects included, by posting a message
/// to itself: a task that, unlike a zero timeout, isn't throttled when chained.
struct Idle {
    channel: web_sys::MessageChannel,
}

impl Idle {
    fn new() -> Self {
        Self {
            channel: web_sys::MessageChannel::new().unwrap(),
        }
    }

    async fn wait(&self) {
        let promise = js_sys::Promise::new(&mut |resolve, _| {
            self.channel.port1().set_onmessage(Some(&resolve));
            self.channel
                .port2()
                .post_message(&wasm_bindgen::JsValue::NULL)
                .unwrap();
        });
        JsFuture::from(promise).await.unwrap();
    }
}
