//! The app's frame: the wordmark, the areas' navigation, live meters and the key hint bar.

use leptos::prelude::*;

use super::Tone;
use crate::chart::{Color, Sparkline};

/// TEMPEST in vector strokes.
#[component]
pub fn Wordmark() -> impl IntoView {
    // Each glyph is on a 10 by 14 grid, 14 apart.
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
            // Numeric attributes keep this subtree off Leptos's static-template path, which
            // wraps static SVG in a stray <g> and breaks the gradient and path layout.
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

/// The app's areas as tabs in the app bar, or a bottom bar on phones. Each shows its number,
/// the key that opens it.
#[component]
pub fn Nav<T>(
    /// Each area and its name.
    items: Vec<(T, &'static str)>,
    /// The area shown.
    current: RwSignal<T>,
) -> impl IntoView
where
    T: Copy + PartialEq + Send + Sync + 'static,
{
    view! {
        <nav class="nav" aria-label="Areas">
            {items
                .into_iter()
                .enumerate()
                .map(|(i, (item, label))| {
                    view! {
                        <button
                            class="nav__item"
                            type="button"
                            aria-current=move || (current.get() == item).then_some("page")
                            on:click=move |_| current.set(item)
                        >
                            <span class="nav__key">{i + 1}</span>
                            {label}
                        </button>
                    }
                })
                .collect_view()}
        </nav>
    }
}

/// A live number with a label, and optionally a sparkline of its recent values.
#[component]
pub fn Meter(
    /// What it measures.
    label: &'static str,
    /// The value, formatted.
    #[prop(into)]
    value: Signal<String>,
    /// Its accent.
    #[prop(default = Tone::Neutral)]
    tone: Tone,
    /// Recent values, oldest first, for a sparkline.
    #[prop(optional, into)]
    history: Option<Signal<Vec<f64>>>,
) -> impl IntoView {
    let color = match tone {
        Tone::Up => Color::Up,
        Tone::Hot => Color::Hot,
        Tone::Warn => Color::Warn,
        Tone::Bad => Color::Bad,
        Tone::Down => Color::Down,
        Tone::Neutral | Tone::Dim => Color::Fg,
    };
    view! {
        <div class=format!("meter {}", tone.class())>
            <span class="meter__label">{label}</span>
            <span class="meter__value">{value}</span>
            {history.map(|values| view! { <Sparkline values color /> })}
        </div>
    }
}

/// A key hint: keys and what they do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hint {
    /// The keys, each shown boxed.
    pub keys: Vec<&'static str>,
    /// What they do.
    pub label: &'static str,
}

/// The bar of key hints along the bottom, listing the keys that work in the focused view.
#[component]
pub fn HintBar(
    /// The hints.
    #[prop(into)]
    hints: Signal<Vec<Hint>>,
) -> impl IntoView {
    view! {
        <footer class="hints">
            {move || {
                hints
                    .get()
                    .into_iter()
                    .map(|hint| {
                        view! {
                            <span>
                                {hint.keys.into_iter().map(|k| view! { <kbd>{k}</kbd> }).collect_view()}
                                {hint.label}
                            </span>
                        }
                    })
                    .collect_view()
            }}
        </footer>
    }
}
