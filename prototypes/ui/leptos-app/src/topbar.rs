//! The top bar: wordmark, live meters, theme and scanline controls.

use leptos::prelude::*;
use proto_core::{chart, format, Sample, World};

use crate::state::{bool_attr, use_ui, THEMES};

#[component]
pub fn Topbar() -> impl IntoView {
    let ui = use_ui();
    let w = move |f: fn(&World) -> String| move || ui.world.with(f);

    view! {
        <header class="topbar">
            <Wordmark />
            <div class="meters">
                <div class="meter meter--down">
                    <span class="meter__label">"download"</span>
                    <span class="meter__value" id="meter-down">{w(|w| format::rate(w.down_bps()))}</span>
                    <Spark key="down" value=|s| s.down_bps />
                </div>
                <div class="meter meter--up">
                    <span class="meter__label">"upload"</span>
                    <span class="meter__value" id="meter-up">{w(|w| format::rate(w.up_bps()))}</span>
                    <Spark key="up" value=|s| s.up_bps />
                </div>
                <div class="meter">
                    <span class="meter__label">"peers"</span>
                    <span class="meter__value" id="meter-peers">{w(|w| w.peer_count().to_string())}</span>
                </div>
            </div>
            <div class="controls">
                <div class="themes" role="group" aria-label="Theme">
                    {THEMES
                        .map(|key| view! {
                            <button
                                class="theme-option"
                                type="button"
                                data-theme-option=key
                                aria-pressed=move || bool_attr(ui.theme.get() == key)
                                on:click=move |_| ui.theme.set(key)
                            >
                                {key}
                            </button>
                        })}
                </div>
                <button
                    class="toggle"
                    id="scanlines"
                    type="button"
                    aria-pressed=move || bool_attr(ui.scanlines.get())
                    on:click=move |_| ui.scanlines.update(|s| *s = !*s)
                >
                    "scanlines"
                </button>
            </div>
        </header>
    }
}

/// A sparkline of the last two minutes of total rate across bundles.
#[component]
fn Spark(key: &'static str, value: fn(&Sample) -> f64) -> impl IntoView {
    let ui = use_ui();
    let paths = Memo::new(move |_| ui.world.with(|w| chart::sparkline(&sum_history(w, value), 84.0, 26.0)));

    view! {
        <svg class=format!("spark spark--{key}") viewBox="0 0 84 26" preserveAspectRatio="none" aria-hidden="true">
            <path class="spark__area" d=move || paths.with(|p| p.1.clone()) />
            <path class="spark__line" d=move || paths.with(|p| p.0.clone()) />
        </svg>
    }
}

fn sum_history(world: &World, value: fn(&Sample) -> f64) -> Vec<f64> {
    let len = world.bundles.iter().map(|b| b.history.len()).max().unwrap_or(0);
    (len.saturating_sub(120)..len)
        .map(|i| world.bundles.iter().filter_map(|b| b.history.get(i)).map(value).sum())
        .collect()
}

#[component]
fn Wordmark() -> impl IntoView {
    // TEMPEST in vector strokes, each glyph on a 10 × 14 grid, 14 apart.
    const GLYPHS: [&str; 7] = [
        "M0 0H10M5 0V14",
        "M10 0H0V14H10M0 7H7",
        "M0 14V0L5 8L10 0V14",
        "M0 14V0H10V7H0",
        "M10 0H0V14H10M0 7H7",
        "M10 0H0V7H10V14H0",
        "M0 0H10M5 0V14",
    ];
    view! {
        <svg class="wordmark" viewBox="-1 -1 96 16" role="img" aria-label="Tempest">
            // Number literals keep this subtree off Leptos's static-template
            // path, which wraps static SVG fragments in a stray <g>.
            <defs>
                <linearGradient id="wordmark-gradient" x1=0 x2=1 y1=0 y2=0>
                    <stop offset=0 stop-color="#3ef3ff"></stop>
                    <stop offset=0.55 stop-color="#ff52d9"></stop>
                    <stop offset=1 stop-color="#f4ff4f"></stop>
                </linearGradient>
            </defs>
            {GLYPHS
                .iter()
                .enumerate()
                .map(|(i, d)| view! { <path transform=format!("translate({} 0)", i * 14) d=*d /> })
                .collect_view()}
        </svg>
    }
}
