//! The chart components: line charts with a readout, zoom and series toggles, bar charts with
//! confidence intervals, and sparklines. Both charts take the keyboard and touch as well as the
//! mouse.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};

use leptos::ev;
use leptos::html::Div;
use leptos::prelude::*;
use send_wrapper::SendWrapper;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use super::bars::{self, Bar};
use super::line::{self, Color, Hover, LineData, Unit};
use super::navigate::{Step, step};
use super::scale::Layout;
use crate::theme::bool_attr;

/// The width charts assume until they are measured.
const DEFAULT_WIDTH: f64 = 720.0;

/// How far a touch may move, in pixels, and still be a tap.
const TAP_SLOP: f64 = 8.0;

/// Where a line chart's readout is.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Readout {
    /// Under the mouse, at this pixel x.
    Pointer(f64),
    /// On the sample at this x value, chosen by a key or a tap.
    Sample(f64),
}

/// The fingers on a line chart, and the gesture they make.
#[derive(Clone, Debug, Default)]
struct Touches {
    /// Each finger's pointer ID and pixel x.
    fingers: Vec<(i32, f64)>,
    /// Where a lone finger came down.
    start: Option<f64>,
    /// Whether the lone finger has moved too far to be a tap, or a pinch has begun.
    moved: bool,
    /// For a pinch, each finger's pointer ID and the x value it came down on.
    pinch: Option<[(i32, f64); 2]>,
}

impl Touches {
    fn at(&self, id: i32) -> Option<f64> {
        self.fingers.iter().find(|f| f.0 == id).map(|f| f.1)
    }
}

/// A line chart. Hovering shows each series' value at the nearest sample; dragging across the
/// plot zooms to that span, and a double click or "reset zoom" zooms back out; the legend's
/// buttons hide and show series.
///
/// Focused, the arrow keys step the readout from sample to sample (ten with `shift`), `home`
/// and `end` jump to either end, `+` and `-` zoom about the readout, `0` shows everything and
/// `esc` hides the readout; each value is announced. On a touch screen a tap shows the readout,
/// two fingers pinch to zoom, and one finger pans a zoomed chart.
#[component]
pub fn LineChart(
    /// The series and units, which may change at any time.
    #[prop(into)]
    data: Signal<LineData>,
    /// Its height in pixels; the width fills the container.
    #[prop(default = 240.0)]
    height: f64,
    /// What the chart shows, for screen readers.
    #[prop(into)]
    label: String,
) -> impl IntoView {
    let container = NodeRef::<Div>::new();
    let width = use_width(container);
    let window = RwSignal::new(None::<(f64, f64)>);
    let hidden = RwSignal::new(BTreeSet::<String>::new());
    let plot = Memo::new(move |_| {
        let layout = Layout::new(width.get(), height);
        data.with(|d| hidden.with(|h| d.plot(layout, window.get(), h)))
    });

    let readout = RwSignal::new(None::<Readout>);
    // Where a mouse drag started, in pixels.
    let drag_from = RwSignal::new(None::<f64>);
    let touches = StoredValue::new(Touches::default());
    let hover_at = move |px: f64| data.with(|d| plot.with(|p| hidden.with(|h| d.hover(p, h, px))));
    let hover = Memo::new(move |_| match readout.get()? {
        Readout::Pointer(px) => hover_at(px),
        Readout::Sample(x) => hover_at(plot.with(|p| p.x.map(x))),
    });
    let to_x = move |client_x: i32| {
        let rect = container.get_untracked()?.get_bounding_client_rect();
        Some(client_x as f64 - rect.left())
    };

    let on_down = move |e: web_sys::PointerEvent| {
        let Some(px) = to_x(e.client_x()) else {
            return;
        };
        if e.pointer_type() != "touch" {
            if e.button() == 0 {
                drag_from.set(Some(px));
            }
            return;
        }
        let id = e.pointer_id();
        touches.update_value(|t| {
            t.fingers.push((id, px));
            match t.fingers[..] {
                [_] => {
                    t.start = Some(px);
                    t.moved = false;
                }
                [(a, pa), (b, pb)] => {
                    let x = |px| plot.with_untracked(|p| p.x.invert(px));
                    t.pinch = Some([(a, x(pa)), (b, x(pb))]);
                    t.moved = true;
                }
                _ => {}
            }
        });
    };
    let on_move = move |e: web_sys::PointerEvent| {
        let Some(px) = to_x(e.client_x()) else {
            return;
        };
        if e.pointer_type() != "touch" {
            readout.set(Some(Readout::Pointer(px)));
            return;
        }
        let id = e.pointer_id();
        let next = touches.try_update_value(|t| {
            let last = t.at(id)?;
            if let Some(finger) = t.fingers.iter_mut().find(|f| f.0 == id) {
                finger.1 = px;
            }
            if let Some([(a, xa), (b, xb)]) = t.pinch {
                let fingers = (t.at(a)?, t.at(b)?);
                return Some(plot.with_untracked(|p| p.pinched((xa, xb), fingers)));
            }
            t.moved |= t.start.is_some_and(|start| (px - start).abs() > TAP_SLOP);
            let zoomed = plot.with_untracked(|p| p.zoomed);
            (t.moved && zoomed).then(|| plot.with_untracked(|p| p.panned(px - last)))
        });
        if let Some(Some(next)) = next {
            window.set(next);
        }
    };
    let on_up = move |e: web_sys::PointerEvent| {
        let px = to_x(e.client_x());
        if e.pointer_type() != "touch" {
            let (Some(from), Some(to)) = (drag_from.get_untracked(), px) else {
                return;
            };
            drag_from.set(None);
            if let Some(span) = plot.with_untracked(|p| line::zoom(p, from, to)) {
                window.set(Some(span));
            }
            return;
        }
        let id = e.pointer_id();
        let tapped = touches.try_update_value(|t| {
            let tapped = t.fingers.len() == 1 && !t.moved;
            t.fingers.retain(|f| f.0 != id);
            if t.fingers.len() < 2 {
                t.pinch = None;
            }
            if t.fingers.is_empty() {
                t.start = None;
            }
            tapped
        });
        if tapped == Some(true)
            && let Some(h) = px.and_then(|px| untrack(|| hover_at(px)))
        {
            readout.set(Some(Readout::Sample(h.at)));
        }
    };
    let on_cancel = move |_| {
        drag_from.set(None);
        touches.set_value(Touches::default());
    };
    // A finger lifting also leaves, and its readout stays.
    let on_leave = move |e: web_sys::PointerEvent| {
        if e.pointer_type() != "touch" {
            readout.set(None);
            drag_from.set(None);
        }
    };

    let on_key = move |e: ev::KeyboardEvent| {
        if e.ctrl_key() || e.meta_key() || e.alt_key() {
            return;
        }
        let p = plot.get_untracked();
        let current = hover.get_untracked().map(|h| h.at);
        let by = if e.shift_key() { 10 } else { 1 };
        let to = match e.key().as_str() {
            "ArrowRight" => Some(Step::By(by)),
            "ArrowLeft" => Some(Step::By(-by)),
            "Home" => Some(Step::First),
            "End" => Some(Step::Last),
            _ => None,
        };
        if let Some(to) = to {
            let samples = data.with_untracked(|d| hidden.with_untracked(|h| d.samples(h)));
            if let Some(x) = step(&samples, current, to) {
                if x < p.window.0 || x > p.window.1 {
                    window.set(p.showing(x));
                }
                readout.set(Some(Readout::Sample(x)));
            }
        } else {
            let center = current.unwrap_or((p.window.0 + p.window.1) / 2.0);
            match e.key().as_str() {
                "+" | "=" => window.set(p.zoomed_by(center, 0.5)),
                "-" | "_" => window.set(p.zoomed_by(center, 2.0)),
                "0" => window.set(None),
                "Escape" if readout.get_untracked().is_some() => readout.set(None),
                _ => return,
            }
        }
        e.prevent_default();
    };
    // What a key or a tap moved the readout to, for screen readers.
    let announced = move || {
        matches!(readout.get(), Some(Readout::Sample(_)))
            .then(|| hover.get().map(|h| announcement(&h)))
            .flatten()
    };

    let clip = format!("chart-clip-{}", next_id());
    let clip_url = format!("url(#{clip})");
    let hint = format!("chart-hint-{}", next_id());
    let legend = Memo::new(move |_| {
        data.with(|d| {
            d.series
                .iter()
                .map(|s| (s.key.clone(), s.label.clone(), s.color))
                .collect::<Vec<_>>()
        })
    });
    let shown_keys =
        move || plot.with(|p| p.paths.iter().map(|s| s.key.clone()).collect::<Vec<_>>());
    // Each series' paths stay in the DOM while it shows; only their data changes.
    let path = move |key: String, part: fn(&line::SeriesPaths) -> String| {
        move || {
            plot.with(|p| {
                p.paths
                    .iter()
                    .find(|s| s.key == key)
                    .map(part)
                    .unwrap_or_default()
            })
        }
    };

    let selection = move || {
        let (Some(a), Some(Readout::Pointer(b))) = (drag_from.get(), readout.get()) else {
            return None;
        };
        let p = plot.get();
        let x0 = a.min(b).max(p.layout.plot_left());
        let x1 = a.max(b).min(p.layout.plot_right());
        (x1 > x0).then(|| {
            let (top, bottom) = (p.layout.plot_top(), p.layout.plot_bottom());
            view! { <rect class="chart__selection" x=x0 y=top width=x1 - x0 height=bottom - top /> }
        })
    };
    let crosshair = move || {
        let h = hover.get()?;
        let (top, bottom) = plot.with(|p| (p.layout.plot_top(), p.layout.plot_bottom()));
        let dots = h
            .rows
            .iter()
            .map(|row| view! { <circle class="chart__dot" style=row.color.style() cx=h.x cy=row.y r=3.5 /> })
            .collect_view();
        Some(view! {
            <line class="chart__crosshair" x1=h.x x2=h.x y1=top y2=bottom />
            {dots}
        })
    };
    let tooltip = move || {
        let h = hover.get()?;
        let w = width.get();
        let class = if h.x > w * 0.65 {
            "chart__tooltip chart__tooltip--left"
        } else {
            "chart__tooltip"
        };
        Some(view! {
            <div class=class style=format!("left:{:.1}px", h.x) aria-hidden="true">
                <div class="chart__tooltip-title">{h.title}</div>
                {h
                    .rows
                    .into_iter()
                    .map(|row| {
                        view! {
                            <div class="chart__tooltip-row" style=row.color.style()>
                                <i class="chart__swatch"></i>
                                <span>{row.label}</span>
                                <span>{row.value}</span>
                            </div>
                        }
                    })
                    .collect_view()}
            </div>
        })
    };

    view! {
        <div class="chart">
            <div class="chart__head">
                <Show when=move || plot.with(|p| p.zoomed)>
                    <button class="chart__reset" type="button" on:click=move |_| window.set(None)>
                        "reset zoom"
                    </button>
                </Show>
                <For
                    each=move || legend.get()
                    key=|s| s.clone()
                    children=move |(key, label, color)| {
                        let shown = {
                            let key = key.clone();
                            move || bool_attr(!hidden.with(|h| h.contains(&key)))
                        };
                        let toggle = move |_| {
                            hidden.update(|h| {
                                if !h.remove(&key) {
                                    h.insert(key.clone());
                                }
                            });
                        };
                        view! {
                            <button class="chart__toggle" type="button" style=color.style() aria-pressed=shown on:click=toggle>
                                {label}
                            </button>
                        }
                    }
                />
            </div>
            <div
                node_ref=container
                class="chart__body"
                style="position:relative"
                tabindex="0"
                role="group"
                aria-roledescription="chart"
                aria-label=label
                aria-describedby=hint.clone()
                aria-keyshortcuts="ArrowLeft ArrowRight Home End + - 0"
                on:keydown=on_key
                on:focusout=move |_| {
                    if matches!(readout.get_untracked(), Some(Readout::Sample(_))) {
                        readout.set(None);
                    }
                }
            >
                <svg
                    class="chart__svg"
                    aria-hidden="true"
                    style=format!("height:{height}px")
                    viewBox=move || format!("0 0 {:.0} {height}", width.get())
                    on:pointerdown=on_down
                    on:pointermove=on_move
                    on:pointerup=on_up
                    on:pointercancel=on_cancel
                    on:pointerleave=on_leave
                    on:dblclick=move |_| window.set(None)
                >
                    <defs>
                        <clipPath id=clip.clone()>
                            <rect
                                x=move || plot.with(|p| p.layout.plot_left())
                                y=move || plot.with(|p| p.layout.plot_top() - 4.0)
                                width=move || plot.with(|p| p.layout.plot_right() - p.layout.plot_left())
                                height=move || plot.with(|p| p.layout.plot_bottom() - p.layout.plot_top() + 8.0)
                            ></rect>
                        </clipPath>
                    </defs>
                    <g class="chart__grid">
                        {move || plot.with(|p| {
                            let (x1, x2) = (p.layout.plot_left(), p.layout.plot_right());
                            p.y_ticks
                                .iter()
                                .map(|t| view! { <line x1=x1 x2=x2 y1=t.pos.round() + 0.5 y2=t.pos.round() + 0.5 /> })
                                .collect_view()
                        })}
                    </g>
                    <g class="chart__axis chart__axis--y">
                        {move || plot.with(|p| {
                            let x = p.layout.plot_left() - 8.0;
                            p.y_ticks
                                .iter()
                                .map(|t| view! { <text x=x y=t.pos + 3.5>{t.label.clone()}</text> })
                                .collect_view()
                        })}
                    </g>
                    <g class="chart__axis chart__axis--x">
                        {move || plot.with(|p| {
                            let y = p.layout.height - 8.0;
                            p.x_ticks
                                .iter()
                                .map(|t| view! { <text x=t.pos y=y>{t.label.clone()}</text> })
                                .collect_view()
                        })}
                    </g>
                    <g clip-path=clip_url>
                        <For each=shown_keys key=|k| k.clone() let:key>
                            <g style=path(key.clone(), |s| s.color.style())>
                                <path class="chart__band" d=path(key.clone(), |s| s.band.clone()) />
                                <path class="chart__area" d=path(key.clone(), |s| s.area.clone()) />
                                <path
                                    class=path(key.clone(), line_class)
                                    d=path(key.clone(), |s| s.line.clone())
                                />
                            </g>
                        </For>
                    </g>
                    {crosshair}
                    {selection}
                </svg>
                {tooltip}
                <p class="sr-only" aria-live="polite">{announced}</p>
            </div>
            <p class="chart__hint" id=hint>
                <span class="chart__hint-pointer">"drag to zoom · double-click to reset"</span>
                <span class="chart__hint-touch">"tap for values · pinch to zoom · drag to pan"</span>
                <span class="chart__hint-keys">"← → values · + − zoom · 0 shows all"</span>
            </p>
        </div>
    }
}

/// A readout as one sentence: the sample, then each series' value.
fn announcement(hover: &Hover) -> String {
    let values: Vec<String> = hover
        .rows
        .iter()
        .map(|row| format!("{} {}", row.label, row.value))
        .collect();
    format!("{}: {}", hover.title, values.join(", "))
}

/// A series line's classes.
fn line_class(series: &line::SeriesPaths) -> String {
    let mut class = "chart__line".to_owned();
    if series.glow {
        class.push_str(" chart__line--glow");
    }
    if series.dashed {
        class.push_str(" chart__line--dashed");
    }
    class
}

/// A bar chart with optional confidence intervals as whiskers. Hovering or tapping a bar shows
/// its value, interval and note; focused, the arrow keys, `home` and `end` move between bars
/// and announce each.
#[component]
pub fn BarChart(
    /// The bars.
    #[prop(into)]
    bars: Signal<Vec<Bar>>,
    /// What the values mean.
    unit: Unit,
    /// Its height in pixels; the width fills the container.
    #[prop(default = 220.0)]
    height: f64,
    /// What the chart shows, for screen readers.
    #[prop(into)]
    label: String,
) -> impl IntoView {
    let container = NodeRef::<Div>::new();
    let width = use_width(container);
    let plot =
        Memo::new(move |_| bars.with(|b| bars::plot(b, unit, Layout::new(width.get(), height))));
    let active = RwSignal::new(None::<usize>);
    // Whether the keyboard chose the bar, so it is announced.
    let keyed = RwSignal::new(false);

    let on_key = move |e: ev::KeyboardEvent| {
        if e.ctrl_key() || e.meta_key() || e.alt_key() {
            return;
        }
        let Some(last) = plot.with_untracked(|p| p.bars.len()).checked_sub(1) else {
            return;
        };
        let now = active.get_untracked();
        let next = match (e.key().as_str(), now) {
            ("ArrowRight", None) | ("Home", _) => 0,
            ("ArrowLeft", None) | ("End", _) => last,
            ("ArrowRight", Some(i)) => (i + 1).min(last),
            ("ArrowLeft", Some(i)) => i.saturating_sub(1),
            ("Escape", Some(_)) => {
                active.set(None);
                e.prevent_default();
                return;
            }
            _ => return,
        };
        e.prevent_default();
        keyed.set(true);
        active.set(Some(next));
    };
    let announced = move || {
        let i = active.get().filter(|_| keyed.get())?;
        plot.with(|p| p.bars.get(i).map(|bar| bar.tooltip.join(", ")))
    };

    let tooltip = move || {
        let i = active.get()?;
        let bar = plot.with(|p| p.bars.get(i).cloned())?;
        let class = if bar.center() > width.get() * 0.65 {
            "chart__tooltip chart__tooltip--left"
        } else {
            "chart__tooltip"
        };
        let mut lines = bar.tooltip.into_iter();
        Some(view! {
            <div class=class style=format!("left:{:.1}px", bar.x + bar.width) aria-hidden="true">
                <div class="chart__tooltip-title">{lines.next()}</div>
                {lines.map(|l| view! { <div>{l}</div> }).collect_view()}
            </div>
        })
    };

    view! {
        <div class="chart">
            <div
                node_ref=container
                class="chart__body"
                style="position:relative"
                tabindex="0"
                role="group"
                aria-roledescription="chart"
                aria-label=label
                on:keydown=on_key
                on:focusout=move |_| {
                    if keyed.get_untracked() {
                        keyed.set(false);
                        active.set(None);
                    }
                }
            >
                <svg
                    class="chart__svg"
                    aria-hidden="true"
                    style=format!("height:{height}px;cursor:default")
                    viewBox=move || format!("0 0 {:.0} {height}", width.get())
                    on:pointerleave=move |e: web_sys::PointerEvent| {
                        // A finger lifting also leaves, and its bar stays shown.
                        if e.pointer_type() != "touch" {
                            active.set(None);
                        }
                    }
                >
                    {move || plot.with(|p| {
                        let (x1, x2) = (p.layout.plot_left(), p.layout.plot_right());
                        let label_x = p.layout.plot_left() - 8.0;
                        let label_y = p.layout.height - 8.0;
                        let grid = p
                            .y_ticks
                            .iter()
                            .map(|t| {
                                let y = t.pos.round() + 0.5;
                                view! {
                                    <line x1=x1 x2=x2 y1=y y2=y />
                                }
                            })
                            .collect_view();
                        let y_labels = p
                            .y_ticks
                            .iter()
                            .map(|t| view! { <text x=label_x y=t.pos + 3.5>{t.label.clone()}</text> })
                            .collect_view();
                        let shapes = p
                            .bars
                            .iter()
                            .enumerate()
                            .map(|(i, bar)| {
                                let whisker = bar.whisker.map(|(lo, hi)| {
                                    let (c, cap) = (bar.center(), (bar.width * 0.3).min(8.0));
                                    view! {
                                        <line class="chart__whisker" x1=c x2=c y1=lo y2=hi />
                                        <line class="chart__whisker" x1=c - cap x2=c + cap y1=lo y2=lo />
                                        <line class="chart__whisker" x1=c - cap x2=c + cap y1=hi y2=hi />
                                    }
                                });
                                view! {
                                    <g
                                        class="chart__bar-group"
                                        style=bar.color.style()
                                        data-active=move || (active.get() == Some(i)).then_some("")
                                        on:pointerenter=move |_| {
                                            keyed.set(false);
                                            active.set(Some(i));
                                        }
                                    >
                                        <rect class="chart__bar" x=bar.x y=bar.top width=bar.width height=bar.height.max(1.0) />
                                        {whisker}
                                        <text class="chart__bar-label" x=bar.center() y=label_y>{bar.label.clone()}</text>
                                    </g>
                                }
                            })
                            .collect_view();
                        view! {
                            <g class="chart__grid">{grid}</g>
                            <g class="chart__axis chart__axis--y">{y_labels}</g>
                            <g class="chart__axis chart__axis--x">{shapes}</g>
                        }
                    })}
                </svg>
                {tooltip}
                <p class="sr-only" aria-live="polite">{announced}</p>
            </div>
        </div>
    }
}

/// A small line of recent values, such as a rate over the last two minutes.
#[component]
pub fn Sparkline(
    /// The values, oldest first.
    #[prop(into)]
    values: Signal<Vec<f64>>,
    /// Its color.
    #[prop(default = Color::Down)]
    color: Color,
) -> impl IntoView {
    const WIDTH: f64 = 72.0;
    const HEIGHT: f64 = 24.0;
    let paths = Memo::new(move |_| values.with(|v| line::sparkline(v, WIDTH, HEIGHT)));
    view! {
        <svg
            class="spark"
            style=format!("--tone:{}", color.css())
            viewBox=format!("0 0 {WIDTH} {HEIGHT}")
            preserveAspectRatio="none"
            aria-hidden="true"
        >
            <path class="spark__area" d=move || paths.with(|p| p.1.clone()) />
            <path class="spark__line" d=move || paths.with(|p| p.0.clone()) />
        </svg>
    }
}

/// The width of `node` in pixels, kept up to date as it resizes.
pub fn use_width(node: NodeRef<Div>) -> RwSignal<f64> {
    let width = RwSignal::new(DEFAULT_WIDTH);
    node.on_load(move |element| {
        let measured = element.clone();
        let measure = move || {
            let w = measured.get_bounding_client_rect().width();
            if w > 0.0 && (w - width.get_untracked()).abs() >= 0.5 {
                width.set(w);
            }
        };
        measure();
        let callback = Closure::<dyn FnMut()>::new(measure);
        let Ok(observer) = web_sys::ResizeObserver::new(callback.as_ref().unchecked_ref()) else {
            return;
        };
        observer.observe(&element);
        let observer = SendWrapper::new((observer, callback));
        on_cleanup(move || observer.0.disconnect());
    });
    width
}

/// A number unique on the page, for SVG ids.
fn next_id() -> usize {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}
