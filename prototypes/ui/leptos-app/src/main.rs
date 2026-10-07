//! Tempest UI prototype in Leptos (client-side rendered WASM).
//!
//! The markup mirrors `server-app/src/render.rs`, the reference DOM.

mod chart;
mod detail;
mod state;
mod topbar;

use std::time::Duration;

use leptos::ev;
use leptos::prelude::*;
use leptos::wasm_bindgen::JsCast;
use proto_core::World;

use state::{Ui, THEMES};

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(App);
}

#[component]
fn App() -> impl IntoView {
    let ui = Ui::new(World::new(42));
    provide_context(ui);

    set_interval(move || ui.world.update(|w| w.tick(250)), Duration::from_millis(250));

    // Theme and scanlines live on <html>.
    Effect::new(move |_| {
        let root = document().document_element().expect("<html>");
        let _ = root.set_attribute("data-theme", ui.theme.get());
        let _ = root.set_attribute("data-scanlines", if ui.scanlines.get() { "on" } else { "off" });
    });

    let _ = window_event_listener(ev::keydown, move |e| {
        if e.ctrl_key() || e.meta_key() || e.alt_key() || in_text_input(&e) {
            return;
        }
        match e.key().as_str() {
            "j" => ui.step_selection(1),
            "k" => ui.step_selection(-1),
            "t" => ui.theme.update(|t| {
                let i = THEMES.iter().position(|k| k == t).unwrap_or(0);
                *t = THEMES[(i + 1) % THEMES.len()];
            }),
            "s" => ui.scanlines.update(|s| *s = !*s),
            "d" => ui.visible.update(|v| v.down = !v.down),
            "u" => ui.visible.update(|v| v.up = !v.up),
            "0" => ui.zoom.set(None),
            _ => return,
        }
        e.prevent_default();
    });

    view! {
        <div class="app" id="app">
            <topbar::Topbar />
            <Transfers />
            <main class="detail" id="detail">
                <detail::Summary />
                <detail::Chunks />
                <chart::Rates />
                <div class="split">
                    <detail::Peers />
                    <detail::Log />
                </div>
            </main>
            <Hints />
        </div>
    }
}

fn in_text_input(e: &web_sys::KeyboardEvent) -> bool {
    let Some(el) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok()) else {
        return false;
    };
    matches!(el.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT") || el.is_content_editable()
}

#[component]
fn Transfers() -> impl IntoView {
    let ui = state::use_ui();
    let count = ui.world.with_untracked(|w| w.bundles.len());
    let rows = (0..count)
        .map(|i| {
            let b = move |f: fn(&proto_core::Bundle) -> String| move || ui.world.with(|w| f(&w.bundles[i]));
            view! {
                <li>
                    <button
                        class="transfer"
                        type="button"
                        data-index=i
                        aria-current=move || state::bool_attr(ui.selected.get() == i)
                        on:click=move |_| ui.select(i)
                    >
                        <span class="transfer__name">{b(|b| b.name.clone())}</span>
                        <span class=b(|b| format!("transfer__state {}", state::state_class(b.state)))>
                            {b(|b| b.state.label().to_string())}
                        </span>
                        <span class="transfer__bar">
                            <span class="transfer__fill" style=b(|b| format!("width:{:.2}%", b.progress * 100.0))></span>
                        </span>
                        <span class="transfer__meta">
                            <span>{b(|b| format!("{} of {}", proto_core::format::percent(b.progress), proto_core::format::bytes(b.size_bytes)))}</span>
                            <span>{b(state::transfer_rate)}</span>
                        </span>
                    </button>
                </li>
            }
        })
        .collect_view();

    view! {
        <aside class="transfers" aria-label="Transfers">
            <h2 class="transfers__title">"transfers"</h2>
            <ul class="transfer-list">{rows}</ul>
        </aside>
    }
}

#[component]
fn Hints() -> impl IntoView {
    view! {
        <footer class="hints">
            <span><kbd>"j"</kbd><kbd>"k"</kbd>"select transfer"</span>
            <span><kbd>"t"</kbd>"next theme"</span>
            <span><kbd>"s"</kbd>"scanlines"</span>
            <span><kbd>"d"</kbd><kbd>"u"</kbd>"toggle download / upload series"</span>
            <span><kbd>"0"</kbd>"reset zoom"</span>
        </footer>
    }
}
