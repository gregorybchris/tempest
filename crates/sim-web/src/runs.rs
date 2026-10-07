//! The runs, filtered, sorted and paged, and one run: its numbers, its charts, its peers and
//! its traffic.

use leptos::prelude::*;
use tempest_sim_api::PREFIX;
use tempest_sim_api::data::{
    ClassSummary, ExperimentInfo, MetricUnit, PeerInfo, RunDetail, RunInfo, RunList, RunSeries,
    TrafficInfo,
};
use tempest_sim_api::query::{Named, Outcome, RunQuery, RunSort};
use tempest_ui_kit::chart::{Bar, BarChart, Color, LineChart, Unit};
use tempest_ui_kit::components::*;
use tempest_ui_kit::format;

use crate::app::{debounce, synced, use_ctx};
use crate::present::{self, maybe, plural, short};
use crate::request::load;
use crate::route::Route;
use crate::views::{Crumbs, Fact, Settings, loaded};

/// Every run.
#[component]
pub fn RunsPage() -> impl IntoView {
    let ctx = use_ctx();
    let from = Memo::new(move |_| match ctx.route.get() {
        Route::Runs(query) => query,
        _ => RunQuery::default(),
    });
    let query = synced(from, move |q| ctx.replace(Route::Runs(q)));
    view! {
        <section class="page">
            <header class="page__head">
                <h1 class="page__title glow">"Runs"</h1>
                <p class="dim">"Every run of every experiment: one variant with one seed, and its headline metrics."</p>
            </header>
            <RunTable query fixed_experiment=false />
        </section>
    }
}

/// A run's outcome as a name for the picker.
fn outcome_label(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Any => "any outcome",
        Outcome::Complete => "all completed",
        Outcome::Incomplete => "some didn't complete",
    }
}

/// The runs `query` selects, with a toolbar that edits it.
#[component]
pub fn RunTable(
    /// The filters, sort and page.
    query: RwSignal<RunQuery>,
    /// Whether the experiment is fixed, as on an experiment's page.
    fixed_experiment: bool,
) -> impl IntoView {
    let ctx = use_ctx();
    let edit = move |f: &dyn Fn(&mut RunQuery)| {
        query.update(|q| {
            f(q);
            q.offset = 0;
        })
    };
    // The toolbar's fields edit the query, and follow it when it changes otherwise.
    let search = synced(
        Memo::new(move |_| query.with(|q| q.search.clone())),
        move |text| debounce(move || edit(&|q| q.search = text.clone())),
    );
    let experiment = synced(
        Memo::new(move |_| query.with(|q| q.experiment.map(|e| e.to_string()).unwrap_or_default())),
        move |text: String| {
            let chosen = text.parse().ok();
            edit(&|q| {
                q.experiment = chosen;
                q.variant = None;
            })
        },
    );
    let outcome = synced(
        Memo::new(move |_| query.with(|q| q.outcome.key().to_owned())),
        move |key: String| {
            let chosen = Outcome::named(&key).unwrap_or_default();
            edit(&|q| q.outcome = chosen)
        },
    );
    let sort = synced(
        Memo::new(move |_| {
            query.with(|q| Sort {
                key: q.sort,
                descending: q.descending,
            })
        }),
        move |s: Sort<RunSort>| {
            edit(&|q| {
                q.sort = s.key;
                q.descending = s.descending;
            })
        },
    );

    let runs = load::<RunList>(move || format!("{PREFIX}/runs?{}", query.get().to_query()));
    let experiments = load::<Vec<ExperimentInfo>>(move || format!("{PREFIX}/experiments"));
    let experiment_options = Signal::derive(move || {
        let mut options = vec![(String::new(), "every experiment".to_owned())];
        if let Some(Ok(list)) = experiments.get() {
            options.extend(
                list.into_iter()
                    .map(|e| (e.id.to_string(), format!("#{} {}", e.id, e.name))),
            );
        }
        options
    });
    let outcome_options = Signal::stored(
        Outcome::ALL
            .iter()
            .map(|o| (o.key().to_owned(), outcome_label(*o).to_owned()))
            .collect::<Vec<_>>(),
    );

    // `[` and `]` page through.
    let total = Memo::new(move |_| match runs.get() {
        Some(Ok(list)) => list.total,
        _ => 0,
    });
    let turn = move |step: i64| {
        let (offset, limit) = query.with_untracked(|q| (q.offset as i64, i64::from(q.limit)));
        let next = offset + step * limit;
        if next >= 0 && (next as u64) < total.get_untracked() {
            query.update(|q| q.offset = next as u64);
        }
    };
    Effect::new(move |previous: Option<u32>| {
        let (direction, count) = ctx.page_step.get();
        if previous.is_some_and(|p| p != count) {
            turn(i64::from(direction));
        }
        count
    });

    let table = loaded(runs, move |list: RunList| {
        if list.runs.is_empty() {
            return view! {
                <Empty title="No runs" text="No run matches these filters.".to_owned() />
            }
            .into_any();
        }
        let first = query.with_untracked(|q| q.offset) + 1;
        let last = first + list.runs.len() as u64 - 1;
        let range = format!(
            "{}–{} of {}",
            format::count(first),
            format::count(last),
            plural(list.total, "run")
        );
        let more = last < list.total;
        let rows = list.runs;
        view! {
            <div class="table-scroll">
                <table class="table runs">
                    <thead>
                        <tr>
                            <SortHeader label="#" key=RunSort::Id sort numeric=true />
                            {(!fixed_experiment).then(|| view! { <SortHeader label="Experiment" key=RunSort::Experiment sort /> })}
                            <SortHeader label="Variant" key=RunSort::Variant sort />
                            <SortHeader label="Seed" key=RunSort::Seed sort numeric=true />
                            <SortHeader label="Completed" key=RunSort::Completed sort numeric=true />
                            <SortHeader label="p50" key=RunSort::CompletionP50 sort numeric=true />
                            <SortHeader label="p90" key=RunSort::CompletionP90 sort numeric=true />
                            <SortHeader label="1st byte" key=RunSort::FirstByteP50 sort numeric=true />
                            <SortHeader label="Util." key=RunSort::Utilization sort numeric=true />
                            <SortHeader label="Fair" key=RunSort::Fairness sort numeric=true />
                            <SortHeader label="Rarest" key=RunSort::Rarest sort numeric=true />
                            <SortHeader label="Overhead" key=RunSort::Overhead sort numeric=true />
                        </tr>
                    </thead>
                    <tbody>
                        {rows.into_iter().map(|run| view! { <RunRow run fixed_experiment /> }).collect_view()}
                    </tbody>
                </table>
            </div>
            <div class="pager">
                <span class="dim">{range}</span>
                <Button small=true disabled=Signal::stored(first == 1) on_click=move || turn(-1)>"← previous"</Button>
                <Button small=true disabled=Signal::stored(!more) on_click=move || turn(1)>"next →"</Button>
            </div>
        }
        .into_any()
    });
    view! {
        <div>
        <div class="toolbar">
            <SearchBox value=search placeholder="search experiment or variant…" node_ref=ctx.search />
            {(!fixed_experiment).then(|| view! { <SelectField label="experiment" options=experiment_options value=experiment /> })}
            <SelectField label="outcome" options=outcome_options value=outcome />
        </div>
        {table}
        </div>
    }
}

#[component]
fn RunRow(run: RunInfo, fixed_experiment: bool) -> impl IntoView {
    let ctx = use_ctx();
    let id = run.id;
    let seconds = |v| maybe(MetricUnit::Seconds, v);
    let fraction = |v| maybe(MetricUnit::Fraction, v);
    let tone = if run.completed == run.downloaders {
        Tone::Down
    } else {
        Tone::Warn
    };
    view! {
        <tr class="row--link" on:click=move |_| ctx.go(Route::Run(id))>
            <td class="num dim"><a href=Route::Run(id).hash()>{id}</a></td>
            {(!fixed_experiment).then(|| view! { <td class="table__clip">{run.experiment_name.clone()}<span class="dim">" #"{run.experiment}</span></td> })}
            <td class="table__clip" title=run.variant_name.clone()>{present::variant_values(&run.variant_name)}</td>
            <td class="num">{run.seed}</td>
            <td class=format!("num col-done {}", tone.class())>{format!("{}/{}", run.completed, run.downloaders)}</td>
            <td class="num">{seconds(run.completion_p50_s)}</td>
            <td class="num">{seconds(run.completion_p90_s)}</td>
            <td class="num">{seconds(run.first_byte_p50_s)}</td>
            <td class="num">{fraction(run.upload_utilization)}</td>
            <td class="num">{run.fairness.map_or_else(|| "–".to_owned(), |f| format!("{f:.3}"))}</td>
            <td class="num">{maybe(MetricUnit::Number, run.rarest_copies)}</td>
            <td class="num">{format::bytes(run.overhead_bytes)}</td>
        </tr>
    }
}

/// The tabs of a run's page.
#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Charts,
    Peers,
    Traffic,
}

/// One run.
#[component]
pub fn RunPage(id: i64) -> impl IntoView {
    let detail = load::<RunDetail>(move || format!("{PREFIX}/runs/{id}"));
    loaded(detail, |d: RunDetail| view! { <Run d /> })
}

#[component]
fn Run(d: RunDetail) -> impl IntoView {
    let r = d.run;
    let id = r.id;
    let tab = RwSignal::new(Tab::Charts);
    let series = load::<RunSeries>(move || format!("{PREFIX}/runs/{id}/series"));
    let peers = load::<Vec<PeerInfo>>(move || format!("{PREFIX}/runs/{id}/peers"));
    let seconds = |v| maybe(MetricUnit::Seconds, v);
    let logged = d.logged;
    let classes = d.classes;
    let traffic = d.traffic;
    let steps = vec![
        ("experiments".to_owned(), "#/experiments".to_owned()),
        (
            format!("{} #{}", r.experiment_name, r.experiment),
            Route::Experiment(r.experiment).hash(),
        ),
    ];
    view! {
        <section class="page">
            <header class="page__head">
                <Crumbs steps here=format!("run #{id}") />
                <h1 class="page__title glow">{format!("seed {}", r.seed)}</h1>
                <p><Settings settings=d.vary /></p>
            </header>
            <Stats>
                <Fact label="completed" value=format!("{} of {}", r.completed, r.downloaders) />
                <Fact label="completion p50" value=seconds(r.completion_p50_s) />
                <Fact label="p90" value=seconds(r.completion_p90_s) />
                <Fact label="p99" value=seconds(r.completion_p99_s) />
                <Fact label="first byte p50" value=seconds(r.first_byte_p50_s) />
                <Fact label="first peer p50" value=seconds(r.first_peer_p50_s) />
                <Fact label="upload utilization" value=maybe(MetricUnit::Fraction, r.upload_utilization) />
                <Fact label="fairness" value=r.fairness.map_or_else(|| "–".to_owned(), |f| format!("{f:.3}")) />
                <Fact label="rarest copies" value=maybe(MetricUnit::Number, r.rarest_copies) />
                <Fact label="payload" value=format::bytes(r.payload_bytes) />
                <Fact label="overhead" value=format::bytes(r.overhead_bytes) />
                <Fact label="duplicate" value=format::bytes(r.duplicate_bytes) />
                <Fact label="simulated" value=present::value(MetricUnit::Seconds, r.end_s) />
                <Fact label="events" value=format::count(r.events) />
                <Fact label="log digest" value=short(&r.log_sha256).to_owned() />
            </Stats>
            <Tabs label="Run" tabs=vec![(Tab::Charts, "Charts"), (Tab::Peers, "Peers"), (Tab::Traffic, "Traffic")] current=tab />
            {move || match tab.get() {
                Tab::Charts => view! { <RunCharts series peers logged /> }.into_any(),
                Tab::Peers => view! { <RunPeers classes=classes.clone() peers /> }.into_any(),
                Tab::Traffic => view! { <RunTraffic traffic=traffic.clone() /> }.into_any(),
            }}
        </section>
    }
}

#[component]
fn RunCharts(
    series: crate::request::Loaded<RunSeries>,
    peers: crate::request::Loaded<Vec<PeerInfo>>,
    logged: bool,
) -> impl IntoView {
    let charts = loaded(series, move |s: RunSeries| {
        let swarm = present::swarm_chart(&s);
        let completions = present::completions_chart(&s);
        let throughput = present::throughput_chart(&s);
        view! {
            <Panel title="downloaders completed">
                <LineChart data=Signal::stored(completions) label="Fraction of downloaders completed over time" />
            </Panel>
            <Panel title="swarm · peers and chunk copies">
                {match s.availability.len() < 2 {
                    true => view! { <Empty title="Too few samples" text="This run ended before it could sample its chunks' availability twice; a shorter sample_interval in the scenario gives more.".to_owned() /> }.into_any(),
                    false => view! { <LineChart data=Signal::stored(swarm) label="Peers present and chunk copies over time" /> }.into_any(),
                }}
            </Panel>
            <Panel title="data sent per second">
                {match logged {
                    true => view! { <LineChart data=Signal::stored(throughput) label="Data sent per second over time" /> }.into_any(),
                    false => view! { <Empty title="No event log" text="This experiment was recorded with --no-events, so its runs kept no event log to measure throughput from.".to_owned() /> }.into_any(),
                }}
            </Panel>
        }
    });
    let cdf = loaded(peers, |p: Vec<PeerInfo>| {
        view! {
            <Panel title="time to complete, from arrival">
                <LineChart data=Signal::stored(present::completion_cdf(&p)) label="Fraction of downloaders completed within each time of arriving" />
            </Panel>
        }
    });
    view! { <div class="grid">{charts}{cdf}</div> }
}

/// Columns of the peers table.
#[derive(Clone, Copy, PartialEq)]
enum PeerColumn {
    Peer,
    Class,
    Arrived,
    FirstByte,
    Took,
    Left,
    Downloaded,
    Uploaded,
}

fn took(p: &PeerInfo) -> Option<f64> {
    Some(p.completed_s? - p.arrived_s?)
}

#[component]
fn RunPeers(
    classes: Vec<ClassSummary>,
    peers: crate::request::Loaded<Vec<PeerInfo>>,
) -> impl IntoView {
    let sort = RwSignal::new(Sort {
        key: PeerColumn::Peer,
        descending: false,
    });
    let classes = classes
        .into_iter()
        .map(|c| {
            let completion = match c.starts.as_str() {
                "complete" => "started complete".to_owned(),
                _ => format!(
                    "{} of {} · p50 {}",
                    c.completed,
                    c.peers,
                    maybe(MetricUnit::Seconds, c.completion_p50_s)
                ),
            };
            view! {
                <tr>
                    <td>{c.class}</td>
                    <td class="dim">{c.behavior}</td>
                    <td class="num">{c.peers}</td>
                    <td>{completion}</td>
                    <td class="num">{format::bytes(c.downloaded)}</td>
                    <td class="num">{format::bytes(c.uploaded)}</td>
                </tr>
            }
        })
        .collect_view();
    let table = loaded(peers, move |mut list: Vec<PeerInfo>| {
        let s = sort.get();
        let time = |v: Option<f64>| v.unwrap_or(f64::INFINITY);
        list.sort_by(|a, b| {
            let order = match s.key {
                PeerColumn::Peer => a.peer.cmp(&b.peer),
                PeerColumn::Class => a.class.cmp(&b.class),
                PeerColumn::Arrived => time(a.arrived_s).total_cmp(&time(b.arrived_s)),
                PeerColumn::FirstByte => time(a.first_byte_s).total_cmp(&time(b.first_byte_s)),
                PeerColumn::Took => time(took(a)).total_cmp(&time(took(b))),
                PeerColumn::Left => time(a.left_s).total_cmp(&time(b.left_s)),
                PeerColumn::Downloaded => a.downloaded.cmp(&b.downloaded),
                PeerColumn::Uploaded => a.uploaded.cmp(&b.uploaded),
            };
            if s.descending { order.reverse() } else { order }
        });
        let seconds = |v| maybe(MetricUnit::Seconds, v);
        view! {
            <div class="table-scroll">
                <table class="table">
                    <thead>
                        <tr>
                            <SortHeader label="Peer" key=PeerColumn::Peer sort numeric=true />
                            <SortHeader label="Class" key=PeerColumn::Class sort />
                            <SortHeader label="Arrived" key=PeerColumn::Arrived sort numeric=true />
                            <SortHeader label="1st byte" key=PeerColumn::FirstByte sort numeric=true />
                            <SortHeader label="Took" key=PeerColumn::Took sort numeric=true />
                            <SortHeader label="Left" key=PeerColumn::Left sort numeric=true />
                            <SortHeader label="↓" key=PeerColumn::Downloaded sort numeric=true />
                            <SortHeader label="↑" key=PeerColumn::Uploaded sort numeric=true />
                        </tr>
                    </thead>
                    <tbody>
                        {list
                            .into_iter()
                            .map(|p| {
                                let took = match (p.starts.as_str(), took(&p)) {
                                    ("complete", _) => "seed".to_owned(),
                                    (_, Some(t)) => seconds(Some(t)),
                                    (_, None) => "didn't".to_owned(),
                                };
                                view! {
                                    <tr>
                                        <td class="num dim">{p.peer}</td>
                                        <td>{p.class}<span class="dim">{(p.behavior != "normal").then(|| format!(" · {}", p.behavior))}</span></td>
                                        <td class="num">{seconds(p.arrived_s)}</td>
                                        <td class="num">{seconds(p.first_byte_s)}</td>
                                        <td class="num">{took}</td>
                                        <td class="num">{seconds(p.left_s)}</td>
                                        <td class="num">{format::bytes(p.downloaded)}</td>
                                        <td class="num">{format::bytes(p.uploaded)}</td>
                                    </tr>
                                }
                            })
                            .collect_view()}
                    </tbody>
                </table>
            </div>
        }
    });
    view! {
        <Panel title="by class">
            <div class="table-scroll">
                <table class="table">
                    <thead>
                        <tr>
                            <th>"Class"</th>
                            <th>"Behavior"</th>
                            <th class="num">"Peers"</th>
                            <th>"Completed"</th>
                            <th class="num">"↓"</th>
                            <th class="num">"↑"</th>
                        </tr>
                    </thead>
                    <tbody>{classes}</tbody>
                </table>
            </div>
        </Panel>
        <Panel title="every peer">{table}</Panel>
    }
}

#[component]
fn RunTraffic(traffic: Vec<TrafficInfo>) -> impl IntoView {
    let bars: Vec<Bar> = traffic
        .iter()
        .take(10)
        .enumerate()
        .map(|(i, t)| Bar {
            label: t.message.clone(),
            value: t.bytes as f64,
            interval: None,
            note: Some(format!(
                "{} sent · {} overhead",
                plural(t.count, "message"),
                format::bytes(t.overhead_bytes)
            )),
            color: Color::Series(i),
        })
        .collect();
    let rows = traffic
        .into_iter()
        .map(|t| {
            view! {
                <tr>
                    <td>{t.message}</td>
                    <td class="num">{format::count(t.count)}</td>
                    <td class="num">{format::bytes(t.bytes)}</td>
                    <td class="num">{format::bytes(t.overhead_bytes)}</td>
                </tr>
            }
        })
        .collect_view();
    view! {
        <div class="grid">
            <Panel title="bytes by message">
                <BarChart bars=Signal::stored(bars) unit=Unit::Bytes label="Bytes sent by message type" />
            </Panel>
            <Panel title="messages">
                <div class="table-scroll">
                    <table class="table">
                        <thead>
                            <tr>
                                <th>"Message"</th>
                                <th class="num">"Sent"</th>
                                <th class="num">"Bytes"</th>
                                <th class="num">"Overhead"</th>
                            </tr>
                        </thead>
                        <tbody>{rows}</tbody>
                    </table>
                </div>
            </Panel>
        </div>
    }
}
