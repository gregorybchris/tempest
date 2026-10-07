//! The rates chart: live lines, hover crosshair and tooltip, drag to zoom.

use leptos::prelude::*;
use leptos::svg::Svg;
use proto_core::chart::{self, Layout};

use crate::state::{bool_attr, use_ui};

/// Width and height of the chart's SVG coordinate system.
const WIDTH: f64 = 960.0;
const HEIGHT: f64 = 240.0;

#[component]
pub fn Rates() -> impl IntoView {
    let ui = use_ui();
    let layout = Layout::rates(WIDTH, HEIGHT);
    let (top, bottom) = (layout.plot_top(), layout.plot_bottom());

    // History gains a sample once a second, so the chart re-renders at 1 Hz
    // even though the world ticks at 4 Hz.
    let history = Memo::new(move |_| ui.bundle(|b| b.history.iter().copied().collect::<Vec<_>>()));
    let chart = Memo::new(move |_| history.with(|h| chart::rates(h, ui.zoom.get(), layout, ui.visible.get())));

    // Pointer state, in viewBox x.
    let pointer = RwSignal::new(None::<f64>);
    let drag_from = RwSignal::new(None::<f64>);
    let hover = Memo::new(move |_| {
        let px = pointer.get()?;
        chart.with(|c| history.with(|h| chart::hover(c, h, ui.visible.get(), px)))
    });

    let svg = NodeRef::<Svg>::new();
    let to_x = move |client_x: i32| {
        let rect = svg.get_untracked()?.get_bounding_client_rect();
        Some((client_x as f64 - rect.left()) / rect.width() * WIDTH)
    };
    let on_down = move |e: web_sys::PointerEvent| {
        if e.button() != 0 {
            return;
        }
        if let Some(el) = svg.get_untracked() {
            let _ = el.set_pointer_capture(e.pointer_id());
        }
        drag_from.set(to_x(e.client_x()));
    };
    let on_move = move |e: web_sys::PointerEvent| pointer.set(to_x(e.client_x()));
    let on_up = move |e: web_sys::PointerEvent| {
        let (Some(from), Some(to)) = (drag_from.get_untracked(), to_x(e.client_x())) else {
            return;
        };
        drag_from.set(None);
        if let Some(window) = chart.with_untracked(|c| chart::zoom(c, from, to)) {
            ui.zoom.set(Some(window));
        }
    };

    let selection = move || {
        let (a, b) = (drag_from.get()?, pointer.get()?);
        let x0 = a.min(b).max(layout.plot_left());
        let x1 = a.max(b).min(layout.plot_right());
        (x1 > x0).then(|| view! { <rect class="chart__selection" x=x0 y=top width=x1 - x0 height=bottom - top /> })
    };

    let crosshair = move || {
        hover.get().map(|h| {
            let dots = h
                .points
                .iter()
                .map(|&(key, y)| view! { <circle class=format!("chart__dot chart__dot--{key}") cx=h.x cy=y r="3.5" /> })
                .collect_view();
            view! {
                <line class="chart__crosshair" x1=h.x x2=h.x y1=top y2=bottom />
                {dots}
            }
        })
    };

    let tooltip = move || {
        hover.get().map(|h| {
            let class = if h.x > WIDTH * 0.7 {
                "chart__tooltip chart__tooltip--left"
            } else {
                "chart__tooltip"
            };
            view! {
                <div class=class style=format!("left:{}%", h.x / WIDTH * 100.0)>
                    {h.lines.into_iter().map(|l| view! { <div>{l}</div> }).collect_view()}
                </div>
            }
        })
    };

    // Series paths stay in the DOM while visible; only their `d` changes.
    let series_keys = move || chart.with(|c| c.series.iter().map(|s| s.key).collect::<Vec<_>>());
    let series_path = move |key: &'static str, d: fn(&chart::SeriesPath) -> &String| {
        move || {
            chart.with(|c| {
                c.series
                    .iter()
                    .find(|s| s.key == key)
                    .map(|s| d(s).clone())
                    .unwrap_or_default()
            })
        }
    };

    let series_toggle = move |key: &'static str, label: &'static str, flip: fn(&mut chart::Visible)| {
        let on = move || ui.visible.with(|v| if key == "down" { v.down } else { v.up });
        view! {
            <button
                class=format!("series-toggle series-toggle--{key}")
                type="button"
                data-series=key
                aria-pressed=move || bool_attr(on())
                on:click=move |_| ui.visible.update(flip)
            >
                {label}
            </button>
        }
    };

    view! {
        <section class="panel panel--chart" id="rates">
            <div class="panel__head">
                <h2 class="panel-title">"rates"</h2>
                <div class="series-toggles">
                    <Show when=move || chart.with(|c| c.zoomed)>
                        <button class="chart-reset" type="button" id="chart-reset" on:click=move |_| ui.zoom.set(None)>
                            "reset zoom"
                        </button>
                    </Show>
                    {series_toggle("down", "download", |v| v.down = !v.down)}
                    {series_toggle("up", "upload", |v| v.up = !v.up)}
                </div>
            </div>
            <div class="chart">
                <svg
                    node_ref=svg
                    class="chart__svg"
                    id="chart"
                    viewBox=format!("0 0 {WIDTH} {HEIGHT}")
                    preserveAspectRatio="none"
                    on:pointerdown=on_down
                    on:pointermove=on_move
                    on:pointerup=on_up
                    on:pointercancel=move |_| drag_from.set(None)
                    on:pointerleave=move |_| pointer.set(None)
                    on:dblclick=move |_| ui.zoom.set(None)
                >
                    <g class="chart__grid">
                        {move || chart.with(|c| {
                            c.y_ticks
                                .iter()
                                .map(|t| view! { <line x1=layout.plot_left() x2=layout.plot_right() y1=t.pos y2=t.pos /> })
                                .collect_view()
                        })}
                    </g>
                    <g class="chart__axis chart__axis--y">
                        {move || chart.with(|c| {
                            c.y_ticks
                                .iter()
                                .map(|t| view! { <text x=layout.plot_left() - 10.0 y=t.pos + 3.5>{t.label.clone()}</text> })
                                .collect_view()
                        })}
                    </g>
                    <g class="chart__axis chart__axis--x">
                        {move || chart.with(|c| {
                            c.x_ticks
                                .iter()
                                .map(|t| view! { <text x=t.pos y=HEIGHT - 8.0>{t.label.clone()}</text> })
                                .collect_view()
                        })}
                    </g>
                    <For each=series_keys key=|k| *k let:key>
                        <path class=format!("chart__area chart__area--{key}") d=series_path(key, |s| &s.area) />
                    </For>
                    <For each=series_keys key=|k| *k let:key>
                        <path class=format!("chart__line chart__line--{key}") d=series_path(key, |s| &s.line) vector-effect="non-scaling-stroke" />
                    </For>
                    {crosshair}
                    {selection}
                </svg>
                {tooltip}
            </div>
            <p class="chart__hint">"drag to zoom · double-click to reset"</p>
        </section>
    }
}
