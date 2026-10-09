//! Where the page is, kept in the address's `#` so that every view has a link and the
//! browser's back button works: `#/experiments`, `#/experiments/3`, `#/runs?seed=2`,
//! `#/runs/12`, `#/compare?variants=3:0,3:1` and `#/baselines`.

use tempest_sim_api::query::{CompareQuery, ExperimentQuery, RunQuery};

/// The app's areas, in the navigation's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Area {
    /// Experiments and their variants.
    Experiments,
    /// Every run.
    Runs,
    /// Variants side by side.
    Compare,
    /// The golden baselines.
    Baselines,
}

impl Area {
    /// Every area, in order.
    pub const ALL: [Area; 4] = [
        Area::Experiments,
        Area::Runs,
        Area::Compare,
        Area::Baselines,
    ];

    /// Its name in the navigation.
    pub fn title(self) -> &'static str {
        match self {
            Area::Experiments => "Experiments",
            Area::Runs => "Runs",
            Area::Compare => "Compare",
            Area::Baselines => "Baselines",
        }
    }
}

/// A view and what it shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// The experiments, filtered and sorted.
    Experiments(ExperimentQuery),
    /// One experiment.
    Experiment(i64),
    /// The runs, filtered, sorted and paged.
    Runs(RunQuery),
    /// One run.
    Run(i64),
    /// A comparison, or none chosen yet.
    Compare(Option<CompareQuery>),
    /// The golden baselines.
    Baselines,
}

impl Default for Route {
    fn default() -> Self {
        Route::Experiments(ExperimentQuery::default())
    }
}

impl Route {
    /// The route a `#` fragment names, with or without the `#`. Anything unknown or unreadable
    /// is the experiments.
    pub fn parse(hash: &str) -> Route {
        let hash = hash.strip_prefix('#').unwrap_or(hash);
        let (path, query) = hash.split_once('?').unwrap_or((hash, ""));
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        let route = match parts[..] {
            ["experiments", id] => id.parse().ok().map(Route::Experiment),
            ["runs", id] => id.parse().ok().map(Route::Run),
            ["runs"] => RunQuery::parse(query).ok().map(Route::Runs),
            ["compare"] => Some(Route::Compare(CompareQuery::parse(query).ok())),
            ["baselines"] => Some(Route::Baselines),
            ["experiments"] => ExperimentQuery::parse(query).ok().map(Route::Experiments),
            _ => None,
        };
        route.unwrap_or_default()
    }

    /// Its `#` fragment.
    pub fn hash(&self) -> String {
        let with = |path: &str, query: String| match query.is_empty() {
            true => format!("#/{path}"),
            false => format!("#/{path}?{query}"),
        };
        match self {
            Route::Experiments(query) => with("experiments", query.to_query()),
            Route::Experiment(id) => format!("#/experiments/{id}"),
            Route::Runs(query) => with("runs", query.to_query()),
            Route::Run(id) => format!("#/runs/{id}"),
            Route::Compare(query) => with(
                "compare",
                query
                    .as_ref()
                    .map(CompareQuery::to_query)
                    .unwrap_or_default(),
            ),
            Route::Baselines => "#/baselines".to_owned(),
        }
    }

    /// The area it belongs to.
    pub fn area(&self) -> Area {
        match self {
            Route::Experiments(_) | Route::Experiment(_) => Area::Experiments,
            Route::Runs(_) | Route::Run(_) => Area::Runs,
            Route::Compare(_) => Area::Compare,
            Route::Baselines => Area::Baselines,
        }
    }
}

impl tempest_ui_kit::shell::Route for Route {
    fn parse(hash: &str) -> Self {
        Route::parse(hash)
    }

    fn hash(&self) -> String {
        Route::hash(self)
    }
}

#[cfg(test)]
mod tests {
    use tempest_sim_api::query::{RunSort, VariantRef};

    use super::*;

    #[test]
    fn routes_read_what_they_write() {
        let routes = [
            Route::default(),
            Route::Experiments(ExperimentQuery {
                search: "crowd".to_owned(),
                ..ExperimentQuery::default()
            }),
            Route::Experiment(3),
            Route::Runs(RunQuery::default()),
            Route::Runs(RunQuery {
                experiment: Some(2),
                sort: RunSort::Fairness,
                ..RunQuery::default()
            }),
            Route::Run(12),
            Route::Compare(None),
            Route::Compare(Some(CompareQuery {
                variants: vec![
                    VariantRef {
                        experiment: 1,
                        variant: 0,
                    },
                    VariantRef {
                        experiment: 2,
                        variant: 1,
                    },
                ],
                reference: 1,
            })),
            Route::Baselines,
        ];
        for route in routes {
            assert_eq!(Route::parse(&route.hash()), route, "{}", route.hash());
        }
        assert_eq!(Route::Experiment(3).hash(), "#/experiments/3");
        assert_eq!(Route::Runs(RunQuery::default()).hash(), "#/runs");
    }

    #[test]
    fn anything_unknown_is_the_experiments() {
        for hash in [
            "",
            "#",
            "#/",
            "#/nowhere",
            "#/runs/x",
            "#/runs?seed=-4",
            "#/a/b/c",
        ] {
            assert_eq!(Route::parse(hash), Route::default(), "{hash}");
        }
        assert_eq!(Route::parse("#/compare?variants=x"), Route::Compare(None));
        assert_eq!(Route::Run(4).area(), Area::Runs);
    }
}
