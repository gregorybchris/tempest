//! The top bar: wordmark, rate meters with sparklines, theme controls.

use dioxus::prelude::*;
use proto_core::{chart, format, Sample, World};

use crate::{bool_attr, use_ui, THEMES};

#[component]
pub fn Topbar() -> Element {
    let mut ui = use_ui();
    let world = ui.world.read();
    let theme = (ui.theme)();
    let scanlines = (ui.scanlines)();
    rsx! {
        header { class: "topbar",
            Wordmark {}
            div { class: "meters",
                div { class: "meter meter--down",
                    span { class: "meter__label", "download" }
                    span { class: "meter__value", id: "meter-down", {format::rate(world.down_bps())} }
                    Spark { values: sum_history(&world, |s| s.down_bps), series: "down" }
                }
                div { class: "meter meter--up",
                    span { class: "meter__label", "upload" }
                    span { class: "meter__value", id: "meter-up", {format::rate(world.up_bps())} }
                    Spark { values: sum_history(&world, |s| s.up_bps), series: "up" }
                }
                div { class: "meter",
                    span { class: "meter__label", "peers" }
                    span { class: "meter__value", id: "meter-peers", "{world.peer_count()}" }
                }
            }
            div { class: "controls",
                div { class: "themes", role: "group", aria_label: "Theme",
                    for key in THEMES {
                        button {
                            class: "theme-option",
                            r#type: "button",
                            "data-theme-option": key,
                            aria_pressed: bool_attr(theme == key),
                            onclick: move |_| ui.theme.set(key),
                            {key}
                        }
                    }
                }
                button {
                    class: "toggle",
                    id: "scanlines",
                    r#type: "button",
                    aria_pressed: bool_attr(scanlines),
                    onclick: move |_| ui.scanlines.toggle(),
                    "scanlines"
                }
            }
        }
    }
}

/// TEMPEST in vector strokes, each glyph on a 10 × 14 grid, 14 apart.
#[component]
fn Wordmark() -> Element {
    const GLYPHS: [&str; 7] = [
        "M0 0H10M5 0V14",      // T
        "M10 0H0V14H10M0 7H7", // E
        "M0 14V0L5 8L10 0V14", // M
        "M0 14V0H10V7H0",      // P
        "M10 0H0V14H10M0 7H7", // E
        "M10 0H0V7H10V14H0",   // S
        "M0 0H10M5 0V14",      // T
    ];
    rsx! {
        svg { class: "wordmark", view_box: "-1 -1 96 16", role: "img", "aria-label": "Tempest",
            defs {
                linearGradient { id: "wordmark-gradient", x1: "0", x2: "1", y1: "0", y2: "0",
                    stop { offset: "0", "stop-color": "#3ef3ff" }
                    stop { offset: "0.55", "stop-color": "#ff52d9" }
                    stop { offset: "1", "stop-color": "#f4ff4f" }
                }
            }
            for (i, d) in GLYPHS.iter().enumerate() {
                path { transform: "translate({i * 14} 0)", d: *d }
            }
        }
    }
}

/// The last two minutes of total rate, for the sparklines.
fn sum_history(world: &World, f: impl Fn(&Sample) -> f64) -> Vec<f64> {
    let len = world
        .bundles
        .iter()
        .map(|b| b.history.len())
        .max()
        .unwrap_or(0);
    (len.saturating_sub(120)..len)
        .map(|i| {
            world
                .bundles
                .iter()
                .filter_map(|b| b.history.get(i))
                .map(&f)
                .sum()
        })
        .collect()
}

#[component]
fn Spark(values: Vec<f64>, series: &'static str) -> Element {
    let (line, area) = chart::sparkline(&values, 84.0, 26.0);
    rsx! {
        svg {
            class: "spark spark--{series}",
            view_box: "0 0 84 26",
            preserve_aspect_ratio: "none",
            "aria-hidden": "true",
            path { class: "spark__area", d: area }
            path { class: "spark__line", d: line }
        }
    }
}
