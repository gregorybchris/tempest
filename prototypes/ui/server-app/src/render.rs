//! Renders the Tempest UI as HTML. This markup is the reference DOM that
//! every prototype matches (see SPEC.md).

use maud::{html, Markup, PreEscaped, DOCTYPE};
use proto_core::chart::{self, Layout, Visible};
use proto_core::{format, Bundle, BundleState, Cell, Path, Sample, World, GRID_COLS};

/// UI state that isn't part of the simulated client.
#[derive(Clone, Debug)]
pub struct Ui {
    pub selected: usize,
    pub theme: &'static str,
    pub scanlines: bool,
    pub visible: Visible,
    pub zoom: Option<(u64, u64)>,
}

impl Default for Ui {
    fn default() -> Self {
        Self { selected: 0, theme: "green", scanlines: false, visible: Visible::default(), zoom: None }
    }
}

pub const THEMES: [(&str, &str); 4] =
    [("green", "green"), ("amber", "amber"), ("vector", "vector"), ("beige", "beige")];

/// Width of the chart's SVG coordinate system. The SVG scales to its container.
pub const CHART_WIDTH: f64 = 960.0;
pub const CHART_HEIGHT: f64 = 240.0;

/// A complete page.
pub fn page(world: &World, ui: &Ui, head_extra: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" data-theme=(ui.theme) data-scanlines=(if ui.scanlines { "on" } else { "off" }) {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Tempest" }
                link rel="preconnect" href="https://fonts.googleapis.com";
                link rel="preconnect" href="https://fonts.gstatic.com" crossorigin;
                link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=IBM+Plex+Mono:wght@400;500;600&display=swap";
                link rel="stylesheet" href="/tempest.css";
                (head_extra)
            }
            body {
                (app(world, ui))
            }
        }
    }
}

/// The app frame: everything inside `<body>`.
pub fn app(world: &World, ui: &Ui) -> Markup {
    let bundle = &world.bundles[ui.selected.min(world.bundles.len() - 1)];
    html! {
        div.app #app {
            (topbar(world, ui))
            (transfers(world, ui))
            main.detail #detail {
                (summary(bundle))
                (chunks(bundle))
                (rates(bundle, ui))
                div.split {
                    (peers(bundle))
                    (log(bundle))
                }
            }
            (hints())
        }
    }
}

pub fn wordmark() -> Markup {
    // TEMPEST in vector strokes, each glyph on a 10 × 14 grid, 14 apart.
    const GLYPHS: [&str; 7] = [
        "M0 0H10M5 0V14",            // T
        "M10 0H0V14H10M0 7H7",       // E
        "M0 14V0L5 8L10 0V14",       // M
        "M0 14V0H10V7H0",            // P
        "M10 0H0V14H10M0 7H7",       // E
        "M10 0H0V7H10V14H0",         // S
        "M0 0H10M5 0V14",            // T
    ];
    html! {
        svg.wordmark viewBox="-1 -1 96 16" role="img" aria-label="Tempest" {
            defs {
                linearGradient #wordmark-gradient x1="0" x2="1" y1="0" y2="0" {
                    stop offset="0" stop-color="#3ef3ff" {}
                    stop offset="0.55" stop-color="#ff52d9" {}
                    stop offset="1" stop-color="#f4ff4f" {}
                }
            }
            @for (i, d) in GLYPHS.iter().enumerate() {
                path transform=(format!("translate({} 0)", i * 14)) d=(d) {}
            }
        }
    }
}

fn topbar(world: &World, ui: &Ui) -> Markup {
    let down: Vec<f64> = sum_history(world, |s| s.down_bps);
    let up: Vec<f64> = sum_history(world, |s| s.up_bps);
    html! {
        header.topbar {
            (wordmark())
            div.meters {
                div.meter.meter--down {
                    span.meter__label { "download" }
                    span.meter__value #meter-down { (format::rate(world.down_bps())) }
                    (spark(&down, "down"))
                }
                div.meter.meter--up {
                    span.meter__label { "upload" }
                    span.meter__value #meter-up { (format::rate(world.up_bps())) }
                    (spark(&up, "up"))
                }
                div.meter {
                    span.meter__label { "peers" }
                    span.meter__value #meter-peers { (world.peer_count()) }
                }
            }
            div.controls {
                div.themes role="group" aria-label="Theme" {
                    @for (key, label) in THEMES {
                        button.theme-option type="button" data-theme-option=(key) aria-pressed=(bool_attr(ui.theme == key)) { (label) }
                    }
                }
                button.toggle #scanlines type="button" aria-pressed=(bool_attr(ui.scanlines)) { "scanlines" }
            }
        }
    }
}

/// The last two minutes of total rate, for the top-bar sparklines.
fn sum_history(world: &World, f: impl Fn(&Sample) -> f64) -> Vec<f64> {
    let len = world.bundles.iter().map(|b| b.history.len()).max().unwrap_or(0);
    let from = len.saturating_sub(120);
    (from..len)
        .map(|i| world.bundles.iter().filter_map(|b| b.history.get(i)).map(&f).sum())
        .collect()
}

fn spark(values: &[f64], key: &str) -> Markup {
    let (line, area) = chart::sparkline(values, 84.0, 26.0);
    html! {
        svg class=(format!("spark spark--{key}")) viewBox="0 0 84 26" preserveAspectRatio="none" aria-hidden="true" {
            path.spark__area d=(area) {}
            path.spark__line d=(line) {}
        }
    }
}

fn transfers(world: &World, ui: &Ui) -> Markup {
    html! {
        aside.transfers aria-label="Transfers" {
            h2.transfers__title { "transfers" }
            ul.transfer-list {
                @for (i, b) in world.bundles.iter().enumerate() {
                    li {
                        button.transfer type="button" data-index=(i) aria-current=(bool_attr(i == ui.selected)) {
                            span.transfer__name { (b.name) }
                            span class=(format!("transfer__state {}", state_class(b.state))) { (b.state.label()) }
                            span.transfer__bar { span.transfer__fill style=(format!("width:{:.2}%", b.progress * 100.0)) {} }
                            span.transfer__meta {
                                span { (format::percent(b.progress)) " of " (format::bytes(b.size_bytes)) }
                                span { (transfer_rate(b)) }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn transfer_rate(b: &Bundle) -> String {
    match b.state {
        BundleState::Downloading => format!("↓ {}", format::rate(b.down_bps)),
        BundleState::Sharing => format!("↑ {}", format::rate(b.up_bps)),
        BundleState::WaitingForPeers => "0 sources".to_string(),
    }
}

pub fn state_class(s: BundleState) -> &'static str {
    match s {
        BundleState::Downloading => "state--downloading",
        BundleState::Sharing => "state--sharing",
        BundleState::WaitingForPeers => "state--waiting",
    }
}

pub fn summary(b: &Bundle) -> Markup {
    html! {
        section.summary #summary {
            h1.detail__title { (b.name) }
            p.detail__explain { (b.explanation()) }
            dl.stats {
                div { dt { "progress" } dd { (format::percent(b.progress)) } }
                div { dt { "size" } dd { (format::bytes(b.size_bytes)) } }
                div { dt { "files" } dd { (format::count(b.files as u64)) } }
                div { dt { "download" } dd { (format::rate(b.down_bps)) } }
                div { dt { "upload" } dd { (format::rate(b.up_bps)) } }
                div { dt { "time left" } dd { (b.eta_secs().map(format::duration).unwrap_or_else(|| "—".into())) } }
                div { dt { "ratio" } dd { (format!("{:.2}", b.ratio())) } }
                div { dt { "bundle id" } dd title=(b.id) { (format::short_id(&b.id)) } }
            }
        }
    }
}

/// CSS class and inline style for one chunk-map cell.
pub fn cell_attrs(cell: &Cell) -> (&'static str, String) {
    if cell.in_flight {
        ("cell cell--flight", String::new())
    } else if cell.have >= 1.0 {
        ("cell cell--have", String::new())
    } else if cell.have > 0.0 {
        ("cell cell--partial", format!("--p:{:.2}", cell.have))
    } else if cell.availability == 0 {
        ("cell cell--gone", String::new())
    } else {
        ("cell cell--missing", format!("--a:{:.2}", cell.availability as f32 / 12.0))
    }
}

pub fn chunks(b: &Bundle) -> Markup {
    html! {
        section.panel.panel--chunks #chunks {
            div.panel__head {
                h2.panel-title { "chunks" }
                span.chunkmap__scale { (format::count(b.chunk_count())) " chunks · " (format::count(b.chunks_per_cell())) " per cell" }
            }
            div.chunkmap style=(format!("--cols:{GRID_COLS}")) role="img" aria-label=(format!("Chunk map: {} complete", format::percent(b.progress))) {
                @for (i, cell) in b.cells.iter().enumerate() {
                    @let (class, style) = cell_attrs(cell);
                    span class=(class) style=(style) data-cell=(i) {}
                }
            }
            div.chunkmap__foot {
                div.key {
                    span.key__item { span.cell.cell--have {} "have" }
                    span.key__item { span.cell.cell--flight {} "downloading" }
                    span.key__item { span.cell.cell--missing style="--a:1" {} "missing, common" }
                    span.key__item { span.cell.cell--missing style="--a:0.15" {} "missing, rare" }
                    span.key__item { span.cell.cell--gone {} "no source" }
                }
                span.chunkmap__readout #readout { "hover a cell for details" }
            }
        }
    }
}

pub fn rates(b: &Bundle, ui: &Ui) -> Markup {
    let history: Vec<Sample> = b.history.iter().copied().collect();
    let c = chart::rates(&history, ui.zoom, Layout::rates(CHART_WIDTH, CHART_HEIGHT), ui.visible);
    let l = c.layout;
    html! {
        section.panel.panel--chart #rates {
            div.panel__head {
                h2.panel-title { "rates" }
                div.series-toggles {
                    @if c.zoomed {
                        button.chart-reset type="button" #chart-reset { "reset zoom" }
                    }
                    button.series-toggle.series-toggle--down type="button" data-series="down" aria-pressed=(bool_attr(ui.visible.down)) { "download" }
                    button.series-toggle.series-toggle--up type="button" data-series="up" aria-pressed=(bool_attr(ui.visible.up)) { "upload" }
                }
            }
            div.chart {
                svg.chart__svg #chart viewBox=(format!("0 0 {} {}", l.width, l.height)) preserveAspectRatio="none" {
                    g.chart__grid {
                        @for t in &c.y_ticks {
                            line x1=(l.plot_left()) x2=(l.plot_right()) y1=(t.pos) y2=(t.pos) {}
                        }
                    }
                    g.chart__axis.chart__axis--y {
                        @for t in &c.y_ticks {
                            text x=(l.plot_left() - 10.0) y=(t.pos + 3.5) { (t.label) }
                        }
                    }
                    g.chart__axis.chart__axis--x {
                        @for t in &c.x_ticks {
                            text x=(t.pos) y=(l.height - 8.0) { (t.label) }
                        }
                    }
                    @for s in &c.series {
                        path class=(format!("chart__area chart__area--{}", s.key)) d=(s.area) {}
                    }
                    @for s in &c.series {
                        path class=(format!("chart__line chart__line--{}", s.key)) d=(s.line) vector-effect="non-scaling-stroke" {}
                    }
                }
            }
            p.chart__hint { "drag to zoom · double-click to reset" }
        }
    }
}

pub fn peers(b: &Bundle) -> Markup {
    html! {
        section.panel.panel--peers #peers {
            div.panel__head { h2.panel-title { "peers · " (b.peers.len()) } }
            @if b.peers.is_empty() {
                p.detail__explain { "Not connected to any peers." }
            } @else {
                div.table-scroll {
                    table.peers {
                        thead { tr {
                            th { "peer" } th { "address" } th { "path" } th { "has" }
                            th { "wants ours" } th { "we serve" } th { "client" } th.num { "↓ from them" } th.num { "↑ to them" }
                        } }
                        tbody {
                            @for p in &b.peers {
                                tr {
                                    td.peer__id title=(p.id) { (format::short_id(&p.id)) }
                                    td.peer__addr { (p.addr) }
                                    td class=(if p.path == Path::Relayed { "path--relayed" } else { "" }) { (p.path.label()) }
                                    td { span.peer__has title=(format::percent(p.progress)) { span style=(format!("width:{:.1}%", p.progress * 100.0)) {} } }
                                    td class=(flag(p.wants_from_us)) { (yes_no(p.wants_from_us)) }
                                    td class=(flag(p.we_serve)) { (if p.newcomer_slot { "newcomer" } else { yes_no(p.we_serve) }) }
                                    td.peer__addr { (p.client) }
                                    td.num { (format::rate(p.down_bps)) }
                                    td.num { (format::rate(p.up_bps)) }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

pub fn log(b: &Bundle) -> Markup {
    html! {
        section.panel.panel--log #log {
            div.panel__head { h2.panel-title { "decisions" } }
            @if b.decisions.is_empty() {
                p.detail__explain { "Nothing to decide until a peer is found." }
            } @else {
                ol.log {
                    @for d in b.decisions.iter().take(60) {
                        li class=(format!("log__entry log--{}", d.kind.label())) {
                            time.log__time { (format::clock(d.t_ms)) }
                            span.log__kind { (d.kind.label()) }
                            span.log__what { (d.what) }
                            span.log__why { (d.why) }
                        }
                    }
                }
            }
        }
    }
}

fn hints() -> Markup {
    html! {
        footer.hints {
            span { kbd { "j" } kbd { "k" } "select transfer" }
            span { kbd { "t" } "next theme" }
            span { kbd { "s" } "scanlines" }
            span { kbd { "d" } kbd { "u" } "toggle download / upload series" }
            span { kbd { "0" } "reset zoom" }
        }
    }
}

fn flag(on: bool) -> &'static str {
    if on { "flag--on" } else { "flag--off" }
}

fn yes_no(on: bool) -> &'static str {
    if on { "yes" } else { "no" }
}

fn bool_attr(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}

#[allow(dead_code)]
fn raw(s: &str) -> PreEscaped<&str> {
    PreEscaped(s)
}
