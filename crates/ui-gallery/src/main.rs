//! A living catalog of Tempest's design system: every component, chart and the chunk map, on
//! made-up live data, in any of the four themes.
//!
//! `trunk serve` in this directory serves it. `?theme=amber` picks the first theme and
//! `?still` stops the clock, for screenshots.

mod demo;

use std::time::Duration;

use leptos::prelude::*;
use tempest_ui_kit::chart::{Bar, BarChart, Color, LineChart, LineData, Series, Unit, XAxis};
use tempest_ui_kit::components::*;
use tempest_ui_kit::theme::{ThemePicker, apply_look};
use tempest_ui_kit::{ChunkMap, Look, Theme, format};

use demo::{Demo, Transfer};

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}

/// The areas in the navigation; the gallery shows them all on one page.
#[derive(Clone, Copy, PartialEq)]
enum Area {
    Transfers,
    Share,
    Catalog,
    Settings,
}

/// A table column.
#[derive(Clone, Copy, PartialEq)]
enum Column {
    Name,
    Size,
    Progress,
    Down,
    Status,
}

/// The detail drawer's tabs.
#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Summary,
    Chunks,
    Rates,
}

#[component]
fn App() -> impl IntoView {
    let search = window().location().search().unwrap_or_default();
    let param = |name: &str| {
        search
            .trim_start_matches('?')
            .split('&')
            .find_map(|p| p.strip_prefix(name))
    };
    let theme = RwSignal::new(param("theme=").and_then(Theme::named).unwrap_or_default());
    let scanlines = RwSignal::new(false);
    let clean = RwSignal::new(false);
    apply_look(Signal::derive(move || Look {
        theme: theme.get(),
        scanlines: scanlines.get(),
        clean: clean.get(),
    }));

    let world = RwSignal::new(Demo::new(7));
    if param("still").is_none() {
        set_interval(
            move || world.update(|w| w.tick(250)),
            Duration::from_millis(250),
        );
    }
    let area = RwSignal::new(Area::Transfers);

    let down_history = Signal::derive(move || {
        world.with(|w| {
            w.history
                .iter()
                .rev()
                .take(60)
                .rev()
                .map(|s| s.down)
                .collect()
        })
    });
    let up_history = Signal::derive(move || {
        world.with(|w| {
            w.history
                .iter()
                .rev()
                .take(60)
                .rev()
                .map(|s| s.up)
                .collect()
        })
    });

    view! {
        <Toaster>
            <header class="appbar">
                <Wordmark />
                <Nav
                    items=vec![
                        (Area::Transfers, "Transfers"),
                        (Area::Share, "Share"),
                        (Area::Catalog, "Catalog"),
                        (Area::Settings, "Settings"),
                    ]
                    current=area
                />
                <div class="appbar__spacer"></div>
                <div class="meters">
                    <Meter
                        label="↓ download"
                        tone=Tone::Down
                        value=Signal::derive(move || world.with(|w| format::rate(w.transfers[0].down)))
                        history=down_history
                    />
                    <Meter
                        label="↑ upload"
                        tone=Tone::Up
                        value=Signal::derive(move || world.with(|w| format::rate(w.transfers[0].up + w.transfers[1].up)))
                        history=up_history
                    />
                </div>
            </header>
            <main class="gallery">
                <section class="gallery__section">
                    <h1 class="gallery__title glow">"Tempest UI kit"</h1>
                    <p class="dim">
                        "Every component of the web UIs, on made-up live data. The look evokes 1980s terminals; the layout stays modern."
                    </p>
                    <div class="gallery__row">
                        <ThemePicker theme />
                        <Toggle label="scanlines" value=scanlines />
                        <Toggle label="clean" value=clean />
                    </div>
                </section>
                <TransfersSection world />
                <ChartsSection world />
                <ControlsSection />
            </main>
            <HintBar hints=vec![
                Hint { keys: vec!["1", "4"], label: "areas" },
                Hint { keys: vec!["j", "k"], label: "move" },
                Hint { keys: vec!["enter"], label: "detail" },
                Hint { keys: vec!["/"], label: "filter" },
                Hint { keys: vec!["T"], label: "theme" },
                Hint { keys: vec!["?"], label: "help" },
            ] />
        </Toaster>
    }
}

/// A status word's accent.
fn tone(status: &str) -> Tone {
    match status {
        "downloading" => Tone::Down,
        "sharing" => Tone::Up,
        "stalled" => Tone::Warn,
        "errored" => Tone::Bad,
        "checking" | "moving" => Tone::Hot,
        _ => Tone::Dim,
    }
}

#[component]
fn TransfersSection(world: RwSignal<Demo>) -> impl IntoView {
    let sort = RwSignal::new(Sort {
        key: Column::Progress,
        descending: true,
    });
    let query = RwSignal::new(String::new());
    let selected = RwSignal::new(None::<usize>);
    let tab = RwSignal::new(Tab::Summary);

    // Rows key on their position in the made-up list; the view sorts indexes, not copies.
    let rows = move || {
        let q = query.get().to_lowercase();
        let s = sort.get();
        world.with(|w| {
            let mut rows: Vec<usize> = (0..w.transfers.len())
                .filter(|&i| w.transfers[i].name.contains(&q))
                .collect();
            rows.sort_by(|&a, &b| {
                let (a, b) = (&w.transfers[a], &w.transfers[b]);
                let order = match s.key {
                    Column::Name => a.name.cmp(b.name),
                    Column::Size => a.size.cmp(&b.size),
                    Column::Progress => a.progress.total_cmp(&b.progress),
                    Column::Down => a.down.total_cmp(&b.down),
                    Column::Status => a.status.cmp(b.status),
                };
                if s.descending { order.reverse() } else { order }
            });
            rows
        })
    };
    let field =
        move |i: usize, f: fn(&Transfer) -> String| move || world.with(|w| f(&w.transfers[i]));
    let title = Signal::derive(move || {
        selected
            .get()
            .map(|i| world.with(|w| w.transfers[i].name.to_owned()))
            .unwrap_or_default()
    });

    view! {
        <section class="gallery__section">
            <Panel title="transfers" actions=move || view! { <SearchBox value=query placeholder="filter by name…" /> }>
                <div class="table-scroll">
                    <table class="table">
                        <thead>
                            <tr>
                                <SortHeader label="Name" key=Column::Name sort />
                                <SortHeader label="Size" key=Column::Size sort numeric=true />
                                <SortHeader label="Progress" key=Column::Progress sort numeric=true />
                                <SortHeader label="↓" key=Column::Down sort numeric=true />
                                <SortHeader label="Status" key=Column::Status sort />
                                <th>"Tags"</th>
                            </tr>
                        </thead>
                        <tbody>
                            <For each=rows key=|i| *i let:i>
                                <tr
                                    aria-selected=move || tempest_ui_kit::theme::bool_attr(selected.get() == Some(i))
                                    on:click=move |_| selected.set(Some(i))
                                    style="cursor:pointer"
                                >
                                    <td class="table__clip">{field(i, |t| t.name.to_owned())}</td>
                                    <td class="num">{field(i, |t| format::bytes(t.size))}</td>
                                    <td style="min-width:150px">
                                        <ProgressBar
                                            fraction=Signal::derive(move || world.with(|w| w.transfers[i].progress))
                                            tone=Signal::derive(move || world.with(|w| tone(w.transfers[i].status)))
                                        />
                                    </td>
                                    <td class="num">{field(i, |t| format::rate(t.down))}</td>
                                    <td>
                                        <Badge
                                            tone=Signal::derive(move || world.with(|w| tone(w.transfers[i].status)))
                                            label=Signal::derive(move || world.with(|w| w.transfers[i].status.to_owned()))
                                        />
                                    </td>
                                    <td>
                                        {world.with_untracked(|w| w.transfers[i].tags.iter().map(|t| view! { <Chip label=*t /> }).collect_view())}
                                    </td>
                                </tr>
                            </For>
                        </tbody>
                    </table>
                </div>
                <Show when=move || rows().is_empty()>
                    <Empty title="Nothing matches" text="No transfer's name contains that. Clear the filter to see them all.">
                        <Button kind=ButtonKind::Ghost on_click=move || query.set(String::new())>"clear filter"</Button>
                    </Empty>
                </Show>
            </Panel>
            <Drawer open=Signal::derive(move || selected.get().is_some()) on_close=move || selected.set(None) title>
                <div class="gallery__stack">
                    <p class="dim">"Downloading from 7 of 12 peers, 2 through relays. About 3 minutes left."</p>
                    <Tabs
                        label="Detail"
                        tabs=vec![(Tab::Summary, "Summary"), (Tab::Chunks, "Chunks"), (Tab::Rates, "Rates")]
                        current=tab
                    />
                    {move || match tab.get() {
                        Tab::Summary => view! {
                            <Stats>
                                <Stat label="progress" value=Signal::derive(move || world.with(|w| format::percent(w.transfers[0].progress)))/>
                                <Stat label="size" value=Signal::derive(move || world.with(|w| format::bytes(w.transfers[0].size)))/>
                                <Stat label="download" value=Signal::derive(move || world.with(|w| format::rate(w.transfers[0].down)))/>
                                <Stat label="ratio" value="0.42"/>
                            </Stats>
                        }.into_any(),
                        Tab::Chunks => view! {
                            <ChunkMap cells=Signal::derive(move || world.with(|w| w.cells.clone())) running=true columns=48 />
                        }.into_any(),
                        Tab::Rates => view! { <LineChart data=rates(world) label="Download and upload rates" height=200.0 /> }.into_any(),
                    }}
                </div>
            </Drawer>
        </section>
    }
}

/// The first transfer's rates as a chart.
fn rates(world: RwSignal<Demo>) -> Signal<LineData> {
    Signal::derive(move || {
        world.with(|w| {
            let down = w.history.iter().map(|s| (s.t as f64, s.down)).collect();
            let up = w.history.iter().map(|s| (s.t as f64, s.up)).collect();
            LineData {
                series: vec![
                    Series::new("down", "download", Color::Down, down).filled(),
                    Series::new("up", "upload", Color::Up, up).dashed(),
                ],
                x: XAxis::Ago,
                y: Unit::Rate,
                y_floor: 1e6,
            }
        })
    })
}

/// A simulation-style comparison: completion over time for three strategies, each a mean with
/// a 95% band.
fn strategies() -> LineData {
    // Points, and their bands.
    type Curve = (Vec<(f64, f64)>, Vec<(f64, f64)>);
    let curve = |rate: f64, spread: f64| -> Curve {
        (0..=60)
            .map(|i| {
                let t = i as f64 * 10.0;
                let y = 1.0 - (-t / rate).exp();
                let w = spread * y * (1.0 - y) * 2.0;
                ((t, y), ((y - w).max(0.0), (y + w).min(1.0)))
            })
            .unzip()
    };
    let series = [
        ("rarest", "rarest first", 120.0, 0.08),
        ("random", "random", 170.0, 0.12),
        ("sequential", "sequential", 260.0, 0.06),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (key, label, rate, spread))| {
        let (points, band) = curve(rate, spread);
        Series::new(key, label, Color::Series(i), points).with_band(band)
    })
    .collect();
    LineData {
        series,
        x: XAxis::Value(Unit::Seconds),
        y: Unit::Fraction,
        y_floor: 1.0,
    }
}

#[component]
fn ChartsSection(world: RwSignal<Demo>) -> impl IntoView {
    let bars = vec![
        Bar {
            label: "rarest first".into(),
            value: 412.0,
            interval: Some((391.0, 433.0)),
            note: Some("20 seeds".into()),
            color: Color::Series(0),
        },
        Bar {
            label: "random".into(),
            value: 538.0,
            interval: Some((502.0, 574.0)),
            note: Some("20 seeds".into()),
            color: Color::Series(1),
        },
        Bar {
            label: "sequential".into(),
            value: 761.0,
            interval: Some((690.0, 832.0)),
            note: Some("20 seeds".into()),
            color: Color::Series(2),
        },
        Bar {
            label: "tit-for-tat".into(),
            value: 455.0,
            interval: Some((440.0, 470.0)),
            note: Some("20 seeds".into()),
            color: Color::Series(3),
        },
    ];
    view! {
        <section class="gallery__section gallery__grid">
            <Panel title="chunks · 1,536 cells, live">
                <ChunkMap cells=Signal::derive(move || world.with(|w| w.cells.clone())) running=true />
            </Panel>
            <Panel title="rates">
                <LineChart data=rates(world) label="Download and upload rates" />
            </Panel>
            <Panel title="completion by strategy · mean and 95% interval">
                <LineChart data=strategies() label="Completion over time by strategy" />
            </Panel>
            <Panel title="time to finish · mean and 95% interval">
                <BarChart bars unit=Unit::Seconds label="Time to finish by strategy" />
            </Panel>
        </section>
    }
}

#[component]
fn ControlsSection() -> impl IntoView {
    let toasts = use_toasts();
    let name = RwSignal::new("ensemble-2026.tar".to_owned());
    let link = RwSignal::new(String::new());
    let category = RwSignal::new("audio".to_owned());
    let limit = RwSignal::new(2_u8);
    let dialog = RwSignal::new(false);
    let link_error = Signal::derive(move || {
        link.with(|l| {
            (!l.is_empty() && !l.starts_with("tempest:"))
                .then(|| "Links start with tempest:".to_owned())
        })
    });
    view! {
        <section class="gallery__section gallery__grid">
            <Panel title="fields">
                <div class="gallery__stack">
                    <TextField label="Name" value=name />
                    <TextField label="Link" value=link placeholder="tempest:…" help="Paste a link to add a transfer." error=link_error />
                    <SelectField
                        label="Category"
                        value=category
                        options=vec![("audio".to_owned(), "Audio".to_owned()), ("video".to_owned(), "Video".to_owned()), ("datasets".to_owned(), "Datasets".to_owned())]
                    />
                    <Segmented label="Active downloads" options=vec![(1, "1"), (2, "2"), (4, "4"), (8, "8")] value=limit />
                </div>
            </Panel>
            <Panel title="actions">
                <div class="gallery__stack">
                    <div class="gallery__row">
                        <Button kind=ButtonKind::Primary on_click=move || toasts.push(Tone::Down, "Added ensemble-2026.tar")>"+ Add"</Button>
                        <Button on_click=move || toasts.push(Tone::Neutral, "Copied the link")>"Copy link"</Button>
                        <Button kind=ButtonKind::Danger on_click=move || dialog.set(true)>"Remove…"</Button>
                        <Button kind=ButtonKind::Ghost on_click=move || toasts.push(Tone::Bad, "Couldn't reach the engine. Is it running?")>"Fail"</Button>
                        <Button small=true disabled=true on_click=|| ()>"Disabled"</Button>
                    </div>
                    <div class="gallery__row">
                        <Badge tone=Tone::Down label="downloading" />
                        <Badge tone=Tone::Up label="sharing" />
                        <Badge tone=Tone::Warn label="stalled" />
                        <Badge tone=Tone::Bad label="errored" />
                        <Badge tone=Tone::Hot label="checking" />
                        <Badge tone=Tone::Dim label="stopped" />
                    </div>
                    <div class="gallery__row">
                        <Chip label="audio" on_remove=move || toasts.push(Tone::Neutral, "Removed tag audio") />
                        <Chip label="archive" />
                    </div>
                    <Empty title="No transfers yet" text="Paste a tempest: link or drop a file to share it." />
                </div>
            </Panel>
            <Dialog
                open=dialog
                on_close=move || dialog.set(false)
                title="Remove ensemble-2026.tar?"
                actions=move || view! {
                    <Button kind=ButtonKind::Ghost on_click=move || dialog.set(false)>"Cancel"</Button>
                    <Button kind=ButtonKind::Danger on_click=move || { dialog.set(false); toasts.push(Tone::Neutral, "Removed, files kept"); }>"Remove"</Button>
                }
            >
                <p class="dim">"It stops sharing. The downloaded files stay on disk unless you delete them too."</p>
            </Dialog>
        </section>
    }
}
