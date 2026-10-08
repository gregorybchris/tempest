//! The experiments, and one experiment: its variants compared, its golden baseline and its
//! runs.

use leptos::prelude::*;
use tempest_sim_api::PREFIX;
use tempest_sim_api::data::{ExperimentDetail, ExperimentInfo};
use tempest_sim_api::query::{
    CompareQuery, ExperimentQuery, ExperimentSort, MAX_COMPARED, RunQuery, VariantRef,
};
use tempest_ui_kit::components::*;
use tempest_ui_kit::format;

use crate::app::{debounce, synced, use_ctx};
use crate::baselines::BaselinePanel;
use crate::compare::ComparisonView;
use crate::present::{plural, short};
use crate::request::load;
use crate::route::Route;
use crate::runs::RunTable;
use crate::views::{Crumbs, Fact, Settings, loaded};

/// The experiments, searched and sorted.
#[component]
pub fn ExperimentsPage() -> impl IntoView {
    let ctx = use_ctx();
    let query = Memo::new(move |_| match ctx.route.get() {
        Route::Experiments(query) => query,
        _ => ExperimentQuery::default(),
    });
    let search = synced(Memo::new(move |_| query.get().search), move |search| {
        debounce(move || {
            ctx.replace(Route::Experiments(ExperimentQuery {
                search: search.clone(),
                ..query.get_untracked()
            }))
        })
    });
    let sort = synced(
        Memo::new(move |_| {
            let q = query.get();
            Sort {
                key: q.sort,
                descending: q.descending,
            }
        }),
        move |sort| {
            ctx.replace(Route::Experiments(ExperimentQuery {
                sort: sort.key,
                descending: sort.descending,
                ..query.get_untracked()
            }))
        },
    );
    let experiments = load::<Vec<ExperimentInfo>>(move || {
        format!("{PREFIX}/experiments?{}", query.get().to_query())
    });

    let table = loaded(experiments, move |list: Vec<ExperimentInfo>| {
        if list.is_empty() {
            let text = match query.with_untracked(|q| q.search.is_empty()) {
                true => {
                    "Record one with tempest-sim run, such as: tempest-sim run crates/sim/scenarios/flash-crowd.yaml. It appears here as soon as it finishes."
                }
                false => {
                    "No experiment's name, description, scenario file or commit contains that."
                }
            };
            return view! { <Empty title="No experiments" text=text.to_owned() /> }.into_any();
        }
        let count = plural(list.len() as u64, "experiment");
        view! {
            <p class="toolbar__count dim">{count}</p>
            <div class="table-scroll">
                <table class="table experiments">
                    <thead>
                        <tr>
                            <SortHeader label="#" key=ExperimentSort::Id sort numeric=true />
                            <SortHeader label="Scenario" key=ExperimentSort::Name sort />
                            <SortHeader label="Variants" key=ExperimentSort::Variants sort numeric=true />
                            <th class="num">"Seeds"</th>
                            <SortHeader label="Runs" key=ExperimentSort::Runs sort numeric=true />
                            <SortHeader label="Peers" key=ExperimentSort::Peers sort numeric=true />
                            <th>"Recorded"</th>
                            <th>"Commit"</th>
                        </tr>
                    </thead>
                    <tbody>
                        {list.into_iter().map(|e| view! { <ExperimentRow e /> }).collect_view()}
                    </tbody>
                </table>
            </div>
        }
        .into_any()
    });
    view! {
        <section class="page">
            <header class="page__head">
                <h1 class="page__title glow">"Experiments"</h1>
                <p class="dim">"Each "<code>"tempest-sim run"</code>" of a scenario: its variants, run with the same seeds so they compare fairly."</p>
            </header>
            <div class="toolbar">
                <SearchBox value=search placeholder="search name, file or commit…" node_ref=ctx.search />
            </div>
            <div>{table}</div>
        </section>
    }
}

#[component]
fn ExperimentRow(e: ExperimentInfo) -> impl IntoView {
    let ctx = use_ctx();
    let href = Route::Experiment(e.id).hash();
    let id = e.id;
    view! {
        <tr class="row--link" on:click=move |_| ctx.go(Route::Experiment(id))>
            <td class="num dim">{e.id}</td>
            <td class="table__clip">
                <a class="row__name" href=href>{e.name}</a>
                {e.baseline.then(|| view! { " " <Badge tone=Tone::Hot label="golden" /> })}
                {e.logged.then(|| view! { " " <Badge tone=Tone::Dim label="events" /> })}
                <div class="row__sub dim">{e.description}</div>
            </td>
            <td class="num">{e.variants}</td>
            <td class="num">{e.seeds}</td>
            <td class="num">{format::count(u64::from(e.runs))}</td>
            <td class="num">{format::count(u64::from(e.peers))}</td>
            <td class="dim nowrap">{e.created_at}</td>
            <td class="dim mono">{e.git_commit.as_deref().map(short).unwrap_or("–").to_owned()}</td>
        </tr>
    }
}

/// One experiment.
#[component]
pub fn ExperimentPage(id: i64) -> impl IntoView {
    let detail = load::<ExperimentDetail>(move || format!("{PREFIX}/experiments/{id}"));
    loaded(detail, |d: ExperimentDetail| view! { <Experiment d /> })
}

#[component]
fn Experiment(d: ExperimentDetail) -> impl IntoView {
    let ctx = use_ctx();
    let e = d.experiment;
    let id = e.id;
    let count = d.variants.len() as u32;
    let reference = RwSignal::new(0_usize);
    let compared = Signal::derive(move || CompareQuery {
        reference: reference.get(),
        ..CompareQuery::experiment(id, count)
    });
    let runs = RwSignal::new(RunQuery {
        experiment: Some(id),
        ..RunQuery::default()
    });
    let meta = [
        e.scenario_file.clone().map(|f| ("file", f)),
        e.scenario_sha256
            .as_deref()
            .map(|s| ("sha-256", short(s).to_owned())),
        e.git_commit
            .as_deref()
            .map(|c| ("commit", short(c).to_owned())),
        Some(("recorded", e.created_at.clone())),
    ];
    let variants = d.variants.clone();
    view! {
        <section class="page">
            <header class="page__head">
                <Crumbs steps=vec![("experiments".to_owned(), "#/experiments".to_owned())] here=format!("#{id}") />
                <h1 class="page__title glow">{e.name.clone()}</h1>
                <p class="dim">{e.description.clone()}</p>
                <dl class="meta">
                    {meta
                        .into_iter()
                        .flatten()
                        .map(|(k, v)| view! { <div><dt>{k}</dt><dd>{v}</dd></div> })
                        .collect_view()}
                </dl>
            </header>
            <Stats>
                <Fact label="variants" value=e.variants.to_string() />
                <Fact label="seeds" value=e.seeds.to_string() />
                <Fact label="runs" value=format::count(u64::from(e.runs)) />
                <Fact label="peers" value=format::count(u64::from(e.peers)) />
            </Stats>
            <Panel
                title="variants"
                actions=move || {
                    let all = CompareQuery::experiment(id, count);
                    view! {
                        <Button small=true on_click=move || ctx.go(Route::Compare(Some(all.clone())))>
                            "compare all ↗"
                        </Button>
                    }
                }
            >
                <div class="table-scroll">
                    <table class="table">
                        <thead>
                            <tr>
                                <th class="num">"#"</th>
                                <th>"Settings"</th>
                                <th class="num">"Runs"</th>
                                <th>"Compare"</th>
                            </tr>
                        </thead>
                        <tbody>
                            {variants
                                .into_iter()
                                .map(|v| {
                                    let item = VariantRef { experiment: id, variant: v.index };
                                    view! {
                                        <tr>
                                            <td class="num dim">{v.index}</td>
                                            <td><Settings settings=v.vary /></td>
                                            <td class="num">{v.runs}</td>
                                            <td><Pick item /></td>
                                        </tr>
                                    }
                                })
                                .collect_view()}
                        </tbody>
                    </table>
                </div>
            </Panel>
            <ComparisonView query=compared on_reference=Callback::new(move |i| reference.set(i)) />
            {e.baseline.then(|| view! { <BaselinePanel id /> })}
            <Panel title="runs">
                <RunTable query=runs fixed_experiment=true />
            </Panel>
        </section>
    }
}

/// A toggle that adds a variant to the comparison basket, or takes it out.
#[component]
pub fn Pick(item: VariantRef) -> impl IntoView {
    let ctx = use_ctx();
    let picked = move || ctx.basket.with(|b| b.contains(&item));
    view! {
        <button
            class="pick"
            type="button"
            aria-pressed=move || tempest_ui_kit::theme::bool_attr(picked())
            title="Add to the comparison basket, or take it out"
            on:click=move |_| {
                ctx.basket.update(|b| match b.iter().position(|v| *v == item) {
                    Some(i) => {
                        b.remove(i);
                    }
                    // A comparison takes at most so many.
                    None if b.len() < MAX_COMPARED => b.push(item),
                    None => {}
                })
            }
        >
            {move || if picked() { "✓ picked" } else { "+ pick" }}
        </button>
    }
}
