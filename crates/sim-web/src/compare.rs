//! Variants side by side: each metric's median across seeds with its confidence interval, the
//! paired difference from a reference with a verdict (D-073, D-167), and the variants over
//! time as bands.

use leptos::prelude::*;
use tempest_sim_api::PREFIX;
use tempest_sim_api::data::{Comparison, ComparisonSeries, MetricRow};
use tempest_sim_api::query::{CompareQuery, VariantRef};
use tempest_ui_kit::chart::{BarChart, Color, LineChart};
use tempest_ui_kit::components::*;

use crate::app::use_ctx;
use crate::present::{self, interval, labels};
use crate::request::load;
use crate::route::Route;
use crate::views::loaded;

/// The comparison the address names, of the basket's variants.
#[component]
pub fn ComparePage() -> impl IntoView {
    let ctx = use_ctx();
    let query = Memo::new(move |_| match ctx.route.get() {
        Route::Compare(query) => query,
        _ => None,
    });
    let body = move || {
        match query.get() {
        None => view! {
            <Empty
                title="Nothing to compare yet"
                text="Pick variants with “+ pick” on an experiment's page, from one experiment or several, or use “compare all” there. Runs are paired by seed, so variants run with the same seeds compare most fairly.".to_owned()
            >
                <a class="btn btn--primary" href="#/experiments">"browse experiments"</a>
            </Empty>
        }
        .into_any(),
        Some(_) => {
            let compared = Signal::derive(move || query.get().unwrap_or_default());
            let on_reference = Callback::new(move |reference| {
                if let Some(q) = query.get_untracked() {
                    ctx.replace(Route::Compare(Some(CompareQuery { reference, ..q })));
                }
            });
            view! { <ComparisonView query=compared on_reference /> }.into_any()
        }
    }
    };
    view! {
        <section class="page">
            <header class="page__head">
                <h1 class="page__title glow">"Compare"</h1>
                <p class="dim">"Medians across seeds with 95% intervals, and each variant's difference from the reference, paired seed by seed."</p>
            </header>
            {body}
        </section>
    }
}

/// The comparison `query` asks for: the variants, a table of every metric, a bar chart of
/// each and the bands over time.
#[component]
pub fn ComparisonView(
    /// The variants and the reference.
    #[prop(into)]
    query: Signal<CompareQuery>,
    /// Picks another reference.
    on_reference: Callback<usize>,
) -> impl IntoView {
    let comparison =
        load::<Comparison>(move || format!("{PREFIX}/compare?{}", query.get().to_query()));
    // The bands don't depend on the reference.
    let variants = Memo::new(move |_| query.get().variants);
    let bands = load::<ComparisonSeries>(move || {
        let q = CompareQuery {
            variants: variants.get(),
            reference: 0,
        };
        format!("{PREFIX}/compare/series?{}", q.to_query())
    });

    let metrics = loaded(comparison, move |c: Comparison| {
        let names = labels(&c.variants);
        let legend = c
            .variants
            .iter()
            .zip(&names)
            .enumerate()
            .map(|(i, (v, label))| {
                let item = VariantRef {
                    experiment: v.experiment,
                    variant: v.variant,
                };
                let reference = i == c.reference;
                let seeds = present::plural(v.seeds.len() as u64, "seed");
                view! {
                    <li class="legend__item" style=Color::Series(i).style() aria-current=reference.then_some("true")>
                        <i class="legend__swatch"></i>
                        <span class="legend__label" title=v.name.clone()>{label.clone()}</span>
                        <span class="dim">{seeds}</span>
                        {match reference {
                            true => view! { <Badge tone=Tone::Hot label="reference" /> }.into_any(),
                            false => view! {
                                <button class="link" type="button" on:click=move |_| on_reference.run(i)>"make reference"</button>
                            }
                            .into_any(),
                        }}
                        <RemoveFromBasket item />
                    </li>
                }
            })
            .collect_view();
        let charts: Vec<_> = c
            .metrics
            .iter()
            .enumerate()
            .map(|(m, row)| {
                let bars = present::metric_bars(&c, m, &names);
                let unit = present::chart_unit(row.unit);
                let title = format!("{} · median, 95% CI", row.name);
                let better = if row.lower_is_better {
                    "lower is better"
                } else {
                    "higher is better"
                };
                (title, better, bars, unit, row.name.clone())
            })
            .collect();
        let charts = charts
            .into_iter()
            .map(|(title, better, bars, unit, name)| {
                view! {
                    <Panel title=title actions=move || view! { <span class="dim panel__note">{better}</span> }>
                        <BarChart bars=Signal::stored(bars) unit height=200.0 label=name />
                    </Panel>
                }
            })
            .collect_view();
        let rule = format!(
            "A difference is meaningful when its {:.0}% interval excludes zero, it is at least {:.0}% of the reference, and at least {} seeds pair up.",
            c.confidence * 100.0,
            c.min_effect * 100.0,
            c.min_pairs
        );
        view! {
            <ul class="legend">{legend}</ul>
            <Panel title="every metric">
                <MetricTable c=c.clone() names=names.clone() />
                <p class="dim note">{rule}</p>
            </Panel>
            <div class="grid">{charts}</div>
        }
    });
    let bands = loaded(bands, |s: ComparisonSeries| {
        let names = labels(&s.variants);
        s.series
            .into_iter()
            .map(|bands| {
                let data = present::band_chart(&bands, &names);
                let title = format!("{} · median, 95% CI", bands.title.to_lowercase());
                view! {
                    <Panel title=title>
                        <LineChart data=Signal::stored(data) label=bands.title />
                    </Panel>
                }
            })
            .collect_view()
    });
    view! {
        <div class="comparison">
            {metrics}
            <h2 class="section-title">"Over time"</h2>
            <div class="grid">{bands}</div>
        </div>
    }
}

/// A button that takes a variant out of the basket, shown only while it is in it.
#[component]
fn RemoveFromBasket(item: VariantRef) -> impl IntoView {
    let ctx = use_ctx();
    let on_compare_page = move || matches!(ctx.route.get(), Route::Compare(Some(_)));
    view! {
        <Show when=on_compare_page>
            <button
                class="link"
                type="button"
                title="Take it out of the comparison"
                on:click=move |_| {
                    if let Route::Compare(Some(mut q)) = ctx.route.get_untracked() {
                        let Some(i) = q.variants.iter().position(|v| *v == item) else { return };
                        q.variants.remove(i);
                        q.reference = match q.reference {
                            r if r == i => 0,
                            r if r > i => r - 1,
                            r => r,
                        };
                        ctx.go(Route::Compare((!q.variants.is_empty()).then_some(q)));
                        if ctx.route.with_untracked(|r| matches!(r, Route::Compare(None))) {
                            ctx.basket.set(Vec::new());
                        }
                    }
                }
            >
                "remove"
            </button>
        </Show>
    }
}

/// Every metric down, every variant across: its median and interval, and its difference from
/// the reference.
#[component]
fn MetricTable(c: Comparison, names: Vec<String>) -> impl IntoView {
    let header = names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            view! {
                <th class="num" style=Color::Series(i).style()>
                    <i class="legend__swatch"></i>
                    {name.clone()}
                </th>
            }
        })
        .collect_view();
    let rows = c
        .metrics
        .iter()
        .map(|row| view! { <tr><th scope="row">{row.name.clone()}</th>{cells(row)}</tr> })
        .collect_view();
    view! {
        <div class="table-scroll">
            <table class="table metrics">
                <thead><tr><th>"Metric"</th>{header}</tr></thead>
                <tbody>{rows}</tbody>
            </table>
        </div>
    }
}

fn cells(row: &MetricRow) -> impl IntoView + use<> {
    row.cells
        .iter()
        .map(|cell| {
            let Some(e) = cell.estimate else {
                return view! { <td class="num dim">"–"</td> }.into_any();
            };
            let change = cell.difference.map(|d| {
                let (words, tone) = present::difference(row.unit, row.lower_is_better, &d);
                view! { <div class=format!("cell__change {}", tone.class())>{words}</div> }
            });
            view! {
                <td class="num">
                    <div class="cell__median">{present::value(row.unit, e.median)}</div>
                    <div class="cell__interval dim">{interval(row.unit, &e)}</div>
                    {change}
                </td>
            }
            .into_any()
        })
        .collect_view()
}
