//! The SQL that lists experiments and runs. Values from a query only ever travel as
//! parameters; the text depends only on which filters are set and on the sort, whose columns
//! come from a fixed table.

use duckdb::types::Value;
use tempest_sim_api::query::{ExperimentQuery, ExperimentSort, Outcome, RunQuery, RunSort};

/// A statement and its parameters, one per `?`.
#[derive(Debug, Clone, PartialEq)]
pub struct Sql {
    /// The statement.
    pub text: String,
    /// The parameters, in order.
    pub params: Vec<Value>,
}

fn direction(descending: bool) -> &'static str {
    if descending { "DESC" } else { "ASC" }
}

/// The columns [`super::read_experiment`] reads, in order.
pub const EXPERIMENT_COLUMNS: &str = "e.experiment_id, e.name, e.description, e.scenario_file,
    e.scenario_sha256, e.git_commit, strftime(e.created_at, '%Y-%m-%d %H:%M:%S'),
    (SELECT count(*) FROM variants v WHERE v.experiment_id = e.experiment_id) AS variant_count,
    count(DISTINCT r.seed) AS seed_count, count(r.run_id) AS run_count,
    coalesce(max(r.peers), 0) AS peer_count, min(r.run_id), max(r.run_id)";

/// Lists the experiments `query` asks for.
pub fn experiments(query: &ExperimentQuery) -> Sql {
    let mut text = format!(
        "SELECT {EXPERIMENT_COLUMNS} FROM experiments e LEFT JOIN runs r USING (experiment_id)"
    );
    let mut params = Vec::new();
    if !query.search.is_empty() {
        text.push_str(
            " WHERE contains(lower(concat_ws(' ', e.name, e.description, e.scenario_file,
                e.git_commit)), lower(?))",
        );
        params.push(Value::Text(query.search.clone()));
    }
    let column = match query.sort {
        ExperimentSort::Id => "e.experiment_id",
        ExperimentSort::Name => "lower(e.name)",
        ExperimentSort::Variants => "variant_count",
        ExperimentSort::Runs => "run_count",
        ExperimentSort::Peers => "peer_count",
    };
    let dir = direction(query.descending);
    text.push_str(&format!(
        " GROUP BY e.experiment_id, e.name, e.description, e.scenario_file, e.scenario_sha256,
            e.git_commit, e.created_at
         ORDER BY {column} {dir}, e.experiment_id {dir}"
    ));
    Sql { text, params }
}

/// The columns [`super::read_run`] reads, in order.
pub const RUN_COLUMNS: &str = "r.run_id, r.experiment_id, e.name, r.variant, v.name, r.seed,
    r.end_s, r.events, r.log_entries, r.log_sha256, r.peers, r.downloaders, r.completed,
    r.completion_p50_s, r.completion_p90_s, r.completion_p99_s, r.first_byte_p50_s,
    r.first_peer_p50_s, r.upload_utilization, r.fairness, r.rarest_copies, r.payload_bytes,
    r.overhead_bytes, r.duplicate_bytes";

/// Runs with their experiment's and variant's names.
pub const RUNS_FROM: &str = "FROM runs r JOIN experiments e USING (experiment_id)
    JOIN variants v ON v.experiment_id = r.experiment_id AND v.variant = r.variant";

/// The page of runs `query` asks for, and the count of every run that matches it.
pub fn runs(query: &RunQuery) -> (Sql, Sql) {
    let mut conditions = Vec::new();
    let mut params = Vec::new();
    if let Some(experiment) = query.experiment {
        conditions.push("r.experiment_id = ?");
        params.push(Value::BigInt(experiment));
    }
    if let Some(variant) = query.variant {
        conditions.push("r.variant = ?");
        params.push(Value::UInt(variant));
    }
    if let Some(seed) = query.seed {
        conditions.push("r.seed = ?");
        params.push(Value::UBigInt(seed));
    }
    if !query.search.is_empty() {
        conditions.push("contains(lower(e.name || ' ' || v.name), lower(?))");
        params.push(Value::Text(query.search.clone()));
    }
    match query.outcome {
        Outcome::Any => {}
        Outcome::Complete => conditions.push("r.completed = r.downloaders"),
        Outcome::Incomplete => conditions.push("r.completed < r.downloaders"),
    }
    let filter = match conditions.is_empty() {
        true => String::new(),
        false => format!(" WHERE {}", conditions.join(" AND ")),
    };

    let column = match query.sort {
        RunSort::Id => "r.run_id",
        RunSort::Experiment => "lower(e.name)",
        RunSort::Variant => "lower(v.name)",
        RunSort::Seed => "r.seed",
        RunSort::Completed => "r.completed / nullif(r.downloaders, 0)",
        RunSort::CompletionP50 => "r.completion_p50_s",
        RunSort::CompletionP90 => "r.completion_p90_s",
        RunSort::FirstByteP50 => "r.first_byte_p50_s",
        RunSort::Utilization => "r.upload_utilization",
        RunSort::Fairness => "r.fairness",
        RunSort::Rarest => "r.rarest_copies",
        RunSort::Overhead => "r.overhead_bytes",
        RunSort::Duplicate => "r.duplicate_bytes",
        RunSort::End => "r.end_s",
    };
    let dir = direction(query.descending);
    let page = Sql {
        text: format!(
            "SELECT {RUN_COLUMNS} {RUNS_FROM}{filter}
             ORDER BY {column} {dir} NULLS LAST, r.run_id {dir} LIMIT ? OFFSET ?"
        ),
        params: [
            params.clone(),
            vec![Value::UInt(query.limit), Value::UBigInt(query.offset)],
        ]
        .concat(),
    };
    let count = Sql {
        text: format!("SELECT count(*) {RUNS_FROM}{filter}"),
        params,
    };
    (page, count)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use tempest_sim_api::query::Named;

    use super::*;

    fn placeholders(sql: &Sql) -> usize {
        sql.text.matches('?').count()
    }

    #[test]
    fn filters_become_conditions_with_parameters() {
        let query = RunQuery {
            experiment: Some(2),
            seed: Some(9),
            search: "crowd".to_owned(),
            outcome: Outcome::Complete,
            ..RunQuery::default()
        };
        let (page, count) = runs(&query);
        assert!(page.text.contains(
            "WHERE r.experiment_id = ? AND r.seed = ? AND contains(lower(e.name || ' ' || v.name), lower(?)) AND r.completed = r.downloaders"
        ));
        assert_eq!(
            page.params,
            [
                Value::BigInt(2),
                Value::UBigInt(9),
                Value::Text("crowd".to_owned()),
                Value::UInt(200),
                Value::UBigInt(0)
            ]
        );
        assert_eq!(placeholders(&page), page.params.len());
        assert_eq!(placeholders(&count), count.params.len());
        assert!(!count.text.contains("LIMIT"));
    }

    #[test]
    fn every_sort_has_a_column_and_the_run_id_breaks_ties() {
        for &sort in RunSort::ALL {
            let (page, _) = runs(&RunQuery {
                sort,
                descending: true,
                ..RunQuery::default()
            });
            assert!(page.text.contains("NULLS LAST, r.run_id DESC"), "{sort:?}");
        }
        for &sort in ExperimentSort::ALL {
            let sql = experiments(&ExperimentQuery {
                sort,
                ..ExperimentQuery::default()
            });
            assert!(sql.text.ends_with("e.experiment_id DESC"), "{sort:?}");
        }
    }

    proptest! {
        #[test]
        fn search_text_only_travels_as_a_parameter(search in ".+", descending in any::<bool>()) {
            let plain = RunQuery { search: "x".to_owned(), descending, ..RunQuery::default() };
            let query = RunQuery { search: search.clone(), ..plain.clone() };
            let (page, count) = runs(&query);
            prop_assert_eq!(&page.text, &runs(&plain).0.text);
            prop_assert_eq!(placeholders(&page), page.params.len());
            prop_assert_eq!(placeholders(&count), count.params.len());

            let listed = experiments(&ExperimentQuery { search, ..ExperimentQuery::default() });
            let plain = experiments(&ExperimentQuery { search: "x".to_owned(), ..ExperimentQuery::default() });
            prop_assert_eq!(&listed.text, &plain.text);
            prop_assert_eq!(placeholders(&listed), listed.params.len());
        }
    }
}
