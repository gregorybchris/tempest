//! The golden baselines (D-077, D-171), and experiments checked against them.

use leptos::prelude::*;
use tempest_sim_api::PREFIX;
use tempest_sim_api::data::{BaselineCheck, BaselineInfo, BaselineReport, Method};
use tempest_ui_kit::chart::{BarChart, Unit};
use tempest_ui_kit::components::*;
use tempest_ui_kit::format;

use crate::app::use_ctx;
use crate::present::{self, plural, short};
use crate::request::load;
use crate::route::Route;
use crate::views::loaded;

/// Every golden baseline, and how each experiment of its scenario fared.
#[component]
pub fn BaselinesPage() -> impl IntoView {
    let list = load::<Vec<BaselineInfo>>(|| format!("{PREFIX}/baselines"));
    let body = loaded(list, |list: Vec<BaselineInfo>| {
        if list.is_empty() {
            return view! {
                <Empty
                    title="No golden baselines"
                    text="The results server reads them from crates/sim/baselines: run tempest-sim web from the repository's root, or point --baselines at them.".to_owned()
                />
            }
            .into_any();
        }
        list.into_iter()
            .map(|b| view! { <Golden b /> })
            .collect_view()
            .into_any()
    });
    view! {
        <section class="page">
            <header class="page__head">
                <h1 class="page__title glow">"Golden baselines"</h1>
                <p class="dim">"Each golden scenario's saved metrics. An experiment of the same scenario is checked against them on the seeds both have, as "<code>"tempest-sim baseline"</code>" checks."</p>
            </header>
            <div class="grid">{body}</div>
        </section>
    }
}

#[component]
fn Golden(b: BaselineInfo) -> impl IntoView {
    let ctx = use_ctx();
    let title = format!("{} · {}", b.scenario, plural(b.seeds.len() as u64, "seed"));
    if b.experiments.is_empty() {
        let hint = format!(
            "No experiment of this scenario yet. Record one with tempest-sim run crates/sim/scenarios/golden/{}.yaml.",
            b.scenario
        );
        return view! {
            <Panel title=title>
                <p class="dim note mono">{b.file}</p>
                <p class="dim">{hint}</p>
            </Panel>
        }
        .into_any();
    }
    let rows = b
        .experiments
        .into_iter()
        .map(|e| {
            let (label, tone) = match (&e.problem, e.regressed, e.improved) {
                (Some(_), _, _) => ("not checked".to_owned(), Tone::Dim),
                (None, 0, 0) => ("passes".to_owned(), Tone::Down),
                (None, 0, n) => (format!("{n} improved"), Tone::Hot),
                (None, n, _) => (format!("{n} regressed"), Tone::Bad),
            };
            let id = e.experiment;
            view! {
                <tr class="row--link" on:click=move |_| ctx.go(Route::Experiment(id))>
                    <td class="num dim"><a href=Route::Experiment(id).hash()>{id}</a></td>
                    <td class="nowrap">{e.created_at}</td>
                    <td class="mono dim">{e.git_commit.as_deref().map(short).unwrap_or("–").to_owned()}</td>
                    <td><Badge tone label /></td>
                    <td class="dim">{e.problem}</td>
                </tr>
            }
        })
        .collect_view();
    view! {
        <Panel title=title>
            <p class="dim note mono">{b.file}</p>
            <div class="table-scroll">
                <table class="table">
                    <thead>
                        <tr>
                            <th class="num">"#"</th>
                            <th>"Recorded"</th>
                            <th>"Commit"</th>
                            <th>"Check"</th>
                            <th></th>
                        </tr>
                    </thead>
                    <tbody>{rows}</tbody>
                </table>
            </div>
        </Panel>
    }
    .into_any()
}

/// Experiment `id` checked against its golden baseline.
#[component]
pub fn BaselinePanel(id: i64) -> impl IntoView {
    let report = load::<BaselineReport>(move || format!("{PREFIX}/experiments/{id}/baseline"));
    let body = loaded(report, |r: BaselineReport| {
        let confidence = r.confidence;
        let file = r.file;
        let variants = r
            .variants
            .into_iter()
            .map(|v| view! { <Checked v confidence /> })
            .collect_view();
        view! {
            <p class="dim note mono">{file}</p>
            {variants}
        }
    });
    view! { <Panel title="golden baseline">{body}</Panel> }
}

#[component]
fn Checked(v: BaselineCheck, confidence: f64) -> impl IntoView {
    if v.seeds.is_empty() {
        let text = format!(
            "The baseline saved none of this variant's seeds ({}), so there is nothing to compare.",
            v.missing
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
        return view! { <Empty title="Not checked" text /> }.into_any();
    }
    let regressed = v.checks.iter().filter(|c| c.regressed).count() as u64;
    let improved = v.checks.iter().filter(|c| c.improved).count() as u64;
    let how = match v.method {
        Method::Mean => "by their means, as a quick check does".to_owned(),
        Method::Paired => format!(
            "seed by seed, with {:.0}% intervals, as a long check does",
            confidence * 100.0
        ),
    };
    let mut summary = format!(
        "Checked on {} {how}: ",
        plural(v.seeds.len() as u64, "seed")
    );
    summary.push_str(&match (regressed, improved) {
        (0, 0) => "every metric is within its tolerance.".to_owned(),
        (0, n) => format!(
            "{} improved beyond tolerance, so the baseline is out of date.",
            plural(n, "metric")
        ),
        (n, _) => format!("{} regressed.", plural(n, "metric")),
    });
    if !v.missing.is_empty() {
        summary.push_str(&format!(
            " The baseline lacks {}, left out.",
            plural(v.missing.len() as u64, "seed")
        ));
    }
    let tone = match (regressed, improved) {
        (0, 0) => Tone::Down,
        (0, _) => Tone::Hot,
        _ => Tone::Bad,
    };
    let bars = present::baseline_bars(&v);
    let rows = v
        .checks
        .iter()
        .map(|c| {
            let (status, tone) = present::check_status(c);
            let value = |v: Option<f64>| v.map_or_else(|| "–".to_owned(), format::compact);
            let change = c.change.map_or_else(
                || "–".to_owned(),
                |e| {
                    format!(
                        "{} ({} to {})",
                        signed(e.median),
                        signed(e.low),
                        signed(e.high)
                    )
                },
            );
            view! {
                <tr>
                    <th scope="row">{c.metric.clone()}</th>
                    <td class="num">{value(c.baseline)}</td>
                    <td class="num">{value(c.current)}</td>
                    <td class="num dim">{value(c.limit)}</td>
                    <td class="num">{change}</td>
                    <td><Badge tone label=status.to_owned() /></td>
                </tr>
            }
        })
        .collect_view();
    view! {
        <div class="checked">
            <h3 class="checked__title">{v.variant_name}</h3>
            <p class=format!("checked__summary {}", tone.class())>{summary}</p>
            <div class="grid grid--wide">
                <div>
                    <p class="dim note">"Change from the baseline, relative to it"</p>
                    <BarChart bars=Signal::stored(bars) unit=Unit::Fraction height=220.0 label="Each metric's change from its baseline" />
                </div>
                <div class="table-scroll">
                    <table class="table">
                        <thead>
                            <tr>
                                <th>"Metric"</th>
                                <th class="num">"Baseline"</th>
                                <th class="num">"Now"</th>
                                <th class="num">"Limit"</th>
                                <th class="num">"Change"</th>
                                <th></th>
                            </tr>
                        </thead>
                        <tbody>{rows}</tbody>
                    </table>
                </div>
            </div>
        </div>
    }
    .into_any()
}

fn signed(v: f64) -> String {
    let sign = if v < 0.0 { "−" } else { "+" };
    format!("{sign}{}", format::compact(v.abs()))
}
