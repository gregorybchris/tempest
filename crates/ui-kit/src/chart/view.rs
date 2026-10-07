//! The chart components: line charts with hover, drag-to-zoom and series toggles, bar charts
//! with confidence intervals, and sparklines.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};

use leptos::html::Div;
use leptos::prelude::*;
use send_wrapper::SendWrapper;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use super::bars::{self, Bar};
use super::line::{self, Color, LineData, Unit};
use super::scale::Layout;
use crate::theme::bool_attr;

/// The width charts assume until they are measured.
const DEFAULT_WIDTH: f64 = 720.0;

/// A line chart. Hovering shows each series' value at the nearest sample; dragging across the
/// plot zooms to that span, and a double click or "reset zoom" zooms back out; the legend's
/// buttons hide and show series.
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

    // The pointer's x in the chart's pixels, and where a drag started.
    let pointer = RwSignal::new(None::<f64>);
    let drag_from = RwSignal::new(None::<f64>);
    let hover = Memo::new(move |_| {
        let px = pointer.get()?;
        data.with(|d| plot.with(|p| hidden.with(|h| d.hover(p, h, px))))
    });
    let to_x = move |client_x: i32| {
        let rect = container.get_untracked()?.get_bounding_client_rect();
        Some(client_x as f64 - rect.left())
    };
    let on_down = move |e: web_sys::PointerEvent| {
        if e.button() == 0 && e.pointer_type() == "mouse" {
            drag_from.set(to_x(e.client_x()));
        }
    };
    let on_up = move |e: web_sys::PointerEvent| {
        let (Some(from), Some(to)) = (drag_from.get_untracked(), to_x(e.client_x())) else {
            return;
        };
        drag_from.set(None);
        if let Some(span) = plot.with_untracked(|p| line::zoom(p, from, to)) {
            window.set(Some(span));
        }
    };

    let clip = format!("chart-clip-{}", next_id());
    let clip_url = format!("url(#{clip})");
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
        let (a, b) = (drag_from.get()?, pointer.get()?);
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
            <div class=class style=format!("left:{:.1}px", h.x)>
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
                            })
                        };
                        view! {
                            <button class="chart__toggle" type="button" style=color.style() aria-pressed=shown on:click=toggle>
                                {label}
                            </button>
                        }
                    }
                />
            </div>
            <div node_ref=container class="chart__body" style="position:relative">
                <svg
                    class="chart__svg"
                    role="img"
                    aria-label=label
                    style=format!("height:{height}px")
                    viewBox=move || format!("0 0 {:.0} {height}", width.get())
                    on:pointerdown=on_down
                    on:pointermove=move |e: web_sys::PointerEvent| pointer.set(to_x(e.client_x()))
                    on:pointerup=on_up
                    on:pointercancel=move |_| drag_from.set(None)
                    on:pointerleave=move |_| {
                        pointer.set(None);
                        drag_from.set(None);
                    }
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
            </div>
            <p class="chart__hint">"drag to zoom · double-click to reset"</p>
        </div>
    }
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

/// A bar chart with optional confidence intervals as whiskers. Hovering a bar shows its value,
/// interval and note.
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
            <div class=class style=format!("left:{:.1}px", bar.x + bar.width)>
                <div class="chart__tooltip-title">{lines.next()}</div>
                {lines.map(|l| view! { <div>{l}</div> }).collect_view()}
            </div>
        })
    };

    view! {
        <div class="chart">
            <div node_ref=container style="position:relative">
                <svg
                    class="chart__svg"
                    role="img"
                    aria-label=label
                    style=format!("height:{height}px;cursor:default")
                    viewBox=move || format!("0 0 {:.0} {height}", width.get())
                    on:pointerleave=move |_| active.set(None)
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
                                        on:pointerenter=move |_| active.set(Some(i))
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
