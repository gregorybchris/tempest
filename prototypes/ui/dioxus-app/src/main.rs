//! Tempest UI prototype in Dioxus (web renderer, built with trunk).
//!
//! The markup mirrors `server-app/src/render.rs`. All data, labels and SVG
//! paths come from `proto-core`.

mod chart;
mod chunks;
mod panels;
mod topbar;

use std::rc::Rc;

use dioxus::prelude::*;
use gloo_events::EventListener;
use gloo_timers::future::TimeoutFuture;
use proto_core::chart::Visible;
use proto_core::{Bundle, World};
use wasm_bindgen::JsCast;

const TICK_MS: u32 = 250;
const THEMES: [&str; 4] = ["green", "amber", "vector", "beige"];

fn main() {
    // Mount straight into <body>, so the DOM matches the reference.
    let body = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.body())
        .expect("document has a <body>");
    LaunchBuilder::web()
        .with_cfg(dioxus::web::Config::new().rootelement(body.into()))
        .launch(App);
}

/// App state shared through context. Every field is a `Copy` signal handle.
#[derive(Clone, Copy)]
pub struct Ui {
    pub world: Signal<World>,
    pub selected: Signal<usize>,
    pub theme: Signal<&'static str>,
    pub scanlines: Signal<bool>,
    pub visible: Signal<Visible>,
    pub zoom: Signal<Option<(u64, u64)>>,
}

impl Ui {
    /// Reads the selected bundle.
    pub fn bundle(&self) -> impl Readable<Target = Bundle> {
        let i = (self.selected)();
        self.world
            .map(move |w| &w.bundles[i.min(w.bundles.len() - 1)])
    }

    /// Selects a transfer. Selecting another bundle resets the chart zoom.
    pub fn select(mut self, i: usize) {
        if i != *self.selected.peek() {
            self.selected.set(i);
            self.zoom.set(None);
        }
    }

    pub fn toggle_series(mut self, key: &str) {
        let mut v = self.visible.write();
        match key {
            "down" => v.down = !v.down,
            _ => v.up = !v.up,
        }
    }

    fn on_key(mut self, key: &str) {
        let count = self.world.peek().bundles.len();
        let selected = *self.selected.peek();
        match key {
            "j" => self.select((selected + 1) % count),
            "k" => self.select((selected + count - 1) % count),
            "t" => {
                let next = THEMES
                    .iter()
                    .position(|t| *t == *self.theme.peek())
                    .unwrap_or(0)
                    + 1;
                self.theme.set(THEMES[next % THEMES.len()]);
            }
            "s" => self.scanlines.toggle(),
            "d" => self.toggle_series("down"),
            "u" => self.toggle_series("up"),
            "0" => self.zoom.set(None),
            _ => {}
        }
    }
}

pub fn use_ui() -> Ui {
    use_context()
}

#[component]
fn App() -> Element {
    let ui = use_context_provider(|| Ui {
        world: Signal::new(World::new(42)),
        selected: Signal::new(0),
        theme: Signal::new("green"),
        scanlines: Signal::new(false),
        visible: Signal::new(Visible::default()),
        zoom: Signal::new(None),
    });

    // Live data.
    let mut world = ui.world;
    use_future(move || async move {
        loop {
            TimeoutFuture::new(TICK_MS).await;
            world.write().tick(TICK_MS as u64);
        }
    });

    // Theme and scanlines live on <html>.
    use_effect(move || {
        let html = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.document_element())
            .expect("document has an <html>");
        let scanlines = if (ui.scanlines)() { "on" } else { "off" };
        html.set_attribute("data-theme", (ui.theme)()).ok();
        html.set_attribute("data-scanlines", scanlines).ok();
    });

    // Global keyboard shortcuts. `Callback::call` runs inside the Dioxus
    // runtime, so the listener can write signals.
    let on_key = use_callback(move |key: String| ui.on_key(&key));
    use_hook(|| {
        let window = web_sys::window().expect("window");
        Rc::new(EventListener::new(&window, "keydown", move |e| {
            let e = e.unchecked_ref::<web_sys::KeyboardEvent>();
            if !(e.ctrl_key() || e.meta_key() || e.alt_key() || in_text_input(e)) {
                on_key.call(e.key());
            }
        }))
    });

    rsx! {
        div { class: "app", id: "app",
            topbar::Topbar {}
            panels::Transfers {}
            main { class: "detail", id: "detail",
                panels::Summary {}
                chunks::Chunks {}
                chart::Rates {}
                div { class: "split",
                    panels::Peers {}
                    panels::Log {}
                }
            }
            Hints {}
        }
    }
}

fn in_text_input(e: &web_sys::KeyboardEvent) -> bool {
    let Some(el) = e
        .target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok())
    else {
        return false;
    };
    matches!(el.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT") || el.is_content_editable()
}

#[component]
fn Hints() -> Element {
    rsx! {
        footer { class: "hints",
            span { kbd { "j" } kbd { "k" } "select transfer" }
            span { kbd { "t" } "next theme" }
            span { kbd { "s" } "scanlines" }
            span { kbd { "d" } kbd { "u" } "toggle download / upload series" }
            span { kbd { "0" } "reset zoom" }
        }
    }
}

pub fn bool_attr(b: bool) -> &'static str {
    if b {
        "true"
    } else {
        "false"
    }
}
