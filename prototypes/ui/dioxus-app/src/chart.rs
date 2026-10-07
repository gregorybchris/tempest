//! The rates chart: hover crosshair and tooltip, drag to zoom, series toggles.

use dioxus::html::input_data::MouseButton;
use dioxus::prelude::*;
use dioxus::web::WebEventExt;
use proto_core::chart::{self, Hover, Layout, RateChart};
use proto_core::Sample;

use crate::{bool_attr, use_ui};

/// The SVG's coordinate system. The SVG scales to its container.
const CHART_WIDTH: f64 = 960.0;
const CHART_HEIGHT: f64 = 240.0;

#[component]
pub fn Rates() -> Element {
    let mut ui = use_ui();
    // History changes once per second, so the chart re-renders once per second.
    let history = use_memo(move || {
        ui.bundle()
            .read()
            .history
            .iter()
            .copied()
            .collect::<Vec<Sample>>()
    });
    let chart = use_memo(move || {
        let layout = Layout::rates(CHART_WIDTH, CHART_HEIGHT);
        chart::rates(&history.read(), (ui.zoom)(), layout, (ui.visible)())
    });

    // Pointer state, in viewBox x. Only the overlay components read these.
    let mut pointer = use_signal(|| None::<f64>);
    let mut drag = use_signal(|| None::<(f64, f64)>);
    let hover = use_memo(move || {
        let px = pointer()?;
        chart::hover(&chart.read(), &history.read(), (ui.visible)(), px)
    });

    let mut svg = use_signal(|| None::<web_sys::Element>);
    let view_x = move |e: &PointerEvent| {
        let rect = svg.peek().as_ref()?.get_bounding_client_rect();
        Some((e.client_coordinates().x - rect.left()) / rect.width() * CHART_WIDTH)
    };

    let c = chart.read();
    let l = c.layout;
    let visible = (ui.visible)();
    rsx! {
        section { class: "panel panel--chart", id: "rates",
            div { class: "panel__head",
                h2 { class: "panel-title", "rates" }
                div { class: "series-toggles",
                    if c.zoomed {
                        button {
                            class: "chart-reset",
                            r#type: "button",
                            id: "chart-reset",
                            onclick: move |_| ui.zoom.set(None),
                            "reset zoom"
                        }
                    }
                    button {
                        class: "series-toggle series-toggle--down",
                        r#type: "button",
                        "data-series": "down",
                        aria_pressed: bool_attr(visible.down),
                        onclick: move |_| ui.toggle_series("down"),
                        "download"
                    }
                    button {
                        class: "series-toggle series-toggle--up",
                        r#type: "button",
                        "data-series": "up",
                        aria_pressed: bool_attr(visible.up),
                        onclick: move |_| ui.toggle_series("up"),
                        "upload"
                    }
                }
            }
            div { class: "chart",
                svg {
                    class: "chart__svg",
                    id: "chart",
                    view_box: "0 0 {l.width} {l.height}",
                    preserve_aspect_ratio: "none",
                    onmounted: move |e| svg.set(Some(e.as_web_event())),
                    onpointerdown: move |e| {
                        let primary = e.trigger_button() == Some(MouseButton::Primary);
                        let Some(x) = view_x(&e).filter(|_| primary) else {
                            return;
                        };
                        // Keep receiving moves while dragging outside the chart.
                        if let Some(el) = svg.peek().as_ref() {
                            el.set_pointer_capture(e.as_web_event().pointer_id()).ok();
                        }
                        drag.set(Some((x, x)));
                    },
                    onpointermove: move |e| {
                        let x = view_x(&e);
                        pointer.set(x);
                        if let (Some(x), Some((start, _))) = (x, drag()) {
                            drag.set(Some((start, x)));
                        }
                    },
                    onpointerup: move |_| {
                        if let Some((a, b)) = drag.take() {
                            if let Some(window) = chart::zoom(&chart.read(), a, b) {
                                ui.zoom.set(Some(window));
                            }
                        }
                    },
                    onpointerleave: move |_| pointer.set(None),
                    ondoubleclick: move |_| ui.zoom.set(None),
                    g { class: "chart__grid",
                        for t in &c.y_ticks {
                            line {
                                x1: l.plot_left(),
                                x2: l.plot_right(),
                                y1: t.pos,
                                y2: t.pos,
                            }
                        }
                    }
                    g { class: "chart__axis chart__axis--y",
                        for t in &c.y_ticks {
                            text { x: l.plot_left() - 10.0, y: t.pos + 3.5, "{t.label}" }
                        }
                    }
                    g { class: "chart__axis chart__axis--x",
                        for t in &c.x_ticks {
                            text { x: t.pos, y: l.height - 8.0, "{t.label}" }
                        }
                    }
                    for s in &c.series {
                        path { class: "chart__area chart__area--{s.key}", d: s.area.as_str() }
                    }
                    for s in &c.series {
                        path {
                            class: "chart__line chart__line--{s.key}",
                            d: s.line.as_str(),
                            "vector-effect": "non-scaling-stroke",
                        }
                    }
                    Marks { chart, hover, drag }
                }
                Tooltip { hover }
            }
            p { class: "chart__hint", "drag to zoom · double-click to reset" }
        }
    }
}

/// Crosshair, dots and the drag selection, drawn over the lines.
#[component]
fn Marks(
    chart: Memo<RateChart>,
    hover: Memo<Option<Hover>>,
    drag: ReadSignal<Option<(f64, f64)>>,
) -> Element {
    let l = chart.read().layout;
    let (top, bottom) = (l.plot_top(), l.plot_bottom());
    let selection = drag().map(|(a, b)| {
        let x0 = a.min(b).max(l.plot_left());
        let x1 = a.max(b).min(l.plot_right());
        (x0, (x1 - x0).max(0.0))
    });
    rsx! {
        if let Some(h) = hover() {
            line { class: "chart__crosshair", x1: h.x, x2: h.x, y1: top, y2: bottom }
            for (key, y) in h.points {
                circle { class: "chart__dot chart__dot--{key}", cx: h.x, cy: y, r: "3.5" }
            }
        }
        if let Some((x, width)) = selection {
            rect { class: "chart__selection", x, y: top, width, height: bottom - top }
        }
    }
}

#[component]
fn Tooltip(hover: Memo<Option<Hover>>) -> Element {
    let Some(h) = hover() else {
        return rsx! {};
    };
    let side = if h.x > CHART_WIDTH * 0.7 {
        " chart__tooltip--left"
    } else {
        ""
    };
    rsx! {
        div {
            class: "chart__tooltip{side}",
            style: "left:{h.x / CHART_WIDTH * 100.0:.3}%",
            for line in h.lines {
                div { {line} }
            }
        }
    }
}
