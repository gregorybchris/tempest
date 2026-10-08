//! What the results UI reads from the results database: experiments and runs with filters,
//! a run's peers, traffic and time series, comparisons of variants with confidence intervals,
//! and checks against golden baselines. Each function answers one route of the results API
//! with its types (`tempest_sim_api`).
//!
//! Comparisons use [`crate::stats`] and baseline checks [`crate::baseline`], on the metrics
//! the `runs` table holds, so they agree with what `tempest-sim run` and `tempest-sim baseline`
//! print.

mod compare;
mod golden;
pub mod sql;

use std::collections::BTreeMap;
use std::time::Duration;

use duckdb::{Connection, OptionalExt, Row, params, params_from_iter};
use tempest_sim_api::data::{
    AvailabilitySample, ClassSummary, ExperimentDetail, ExperimentInfo, PeerInfo, RunDetail,
    RunInfo, RunList, RunSeries, Setting, TrafficInfo, VariantInfo,
};
use tempest_sim_api::query::{ExperimentQuery, RunQuery};

use crate::summary::RunMetrics;

pub use compare::{compare, compare_series};
pub use golden::{Baselines, baseline_report, baselines};

/// Why an answer couldn't be read.
#[derive(Debug, thiserror::Error)]
pub enum BrowseError {
    /// DuckDB failed.
    #[error("results database: {0}")]
    Database(#[from] duckdb::Error),
    /// Something asked for isn't in the database.
    #[error("{0}")]
    NotFound(String),
    /// The golden baselines couldn't be read.
    #[error(transparent)]
    Baseline(#[from] crate::baseline::BaselineError),
}

type Result<T> = std::result::Result<T, BrowseError>;

/// Experiments, filtered and sorted as `query` asks.
pub fn experiments(
    conn: &Connection,
    query: &ExperimentQuery,
    baselines: &Baselines,
) -> Result<Vec<ExperimentInfo>> {
    let sql = sql::experiments(query);
    let names = baselines.names()?;
    let mut statement = conn.prepare(&sql.text)?;
    let rows = statement.query_map(params_from_iter(&sql.params), read_experiment)?;
    let mut experiments = Vec::new();
    for row in rows {
        let (mut experiment, run_ids) = row?;
        experiment.logged = logged(conn, run_ids)?;
        experiment.baseline = names.contains(&experiment.name);
        experiments.push(experiment);
    }
    Ok(experiments)
}

/// Reads [`sql::EXPERIMENT_COLUMNS`], with the experiment's lowest and highest run IDs.
fn read_experiment(row: &Row) -> duckdb::Result<(ExperimentInfo, Option<(i64, i64)>)> {
    let first: Option<i64> = row.get(11)?;
    let last: Option<i64> = row.get(12)?;
    let experiment = ExperimentInfo {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        scenario_file: row.get(3)?,
        scenario_sha256: row.get(4)?,
        git_commit: row.get(5)?,
        created_at: row.get(6)?,
        variants: count(row.get(7)?),
        seeds: count(row.get(8)?),
        runs: count(row.get(9)?),
        peers: count(row.get(10)?),
        logged: false,
        baseline: false,
    };
    Ok((experiment, first.zip(last)))
}

fn count(n: i64) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Whether any of the runs from `first` to `last` has its event log recorded. An experiment's
/// runs have consecutive IDs, so the range finds them cheaply.
fn logged(conn: &Connection, run_ids: Option<(i64, i64)>) -> Result<bool> {
    let Some((first, last)) = run_ids else {
        return Ok(false);
    };
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM events WHERE run_id BETWEEN ? AND ?)",
        params![first, last],
        |row| row.get(0),
    )?)
}

/// An experiment with its variants and scenario, if there is one with ID `id`.
pub fn experiment(
    conn: &Connection,
    id: i64,
    baselines: &Baselines,
) -> Result<Option<ExperimentDetail>> {
    let sql = format!(
        "SELECT {} FROM experiments e LEFT JOIN runs r USING (experiment_id)
         WHERE e.experiment_id = ?
         GROUP BY e.experiment_id, e.name, e.description, e.scenario_file, e.scenario_sha256,
            e.git_commit, e.created_at",
        sql::EXPERIMENT_COLUMNS
    );
    let Some((mut experiment, run_ids)) = conn.query_row(&sql, [id], read_experiment).optional()?
    else {
        return Ok(None);
    };
    experiment.logged = logged(conn, run_ids)?;
    experiment.baseline = baselines.names()?.contains(&experiment.name);
    let scenario: String = conn.query_row(
        "SELECT scenario::VARCHAR FROM experiments WHERE experiment_id = ?",
        [id],
        |row| row.get(0),
    )?;
    let mut statement = conn.prepare(
        "SELECT v.variant, v.name, v.vary::VARCHAR, count(r.run_id)
         FROM variants v LEFT JOIN runs r USING (experiment_id, variant)
         WHERE v.experiment_id = ? GROUP BY ALL ORDER BY v.variant",
    )?;
    let variants = statement
        .query_map([id], |row| {
            Ok(VariantInfo {
                index: row.get(0)?,
                name: row.get(1)?,
                vary: settings(&row.get::<_, String>(2)?),
                runs: count(row.get(3)?),
            })
        })?
        .collect::<duckdb::Result<_>>()?;
    Ok(Some(ExperimentDetail {
        experiment,
        variants,
        scenario: serde_json::from_str(&scenario).unwrap_or(serde_json::Value::Null),
    }))
}

/// A variant's varied settings from its `vary` JSON, as the scenario file would write them.
fn settings(vary: &str) -> Vec<Setting> {
    let values: BTreeMap<String, serde_json::Value> =
        serde_json::from_str(vary).unwrap_or_default();
    values
        .into_iter()
        .map(|(key, value)| Setting {
            key,
            value: match value {
                serde_json::Value::String(text) => text,
                other => other.to_string(),
            },
        })
        .collect()
}

/// A page of runs, filtered and sorted as `query` asks.
pub fn runs(conn: &Connection, query: &RunQuery) -> Result<RunList> {
    let (page, count) = sql::runs(query);
    let total: i64 = conn.query_row(&count.text, params_from_iter(&count.params), |row| {
        row.get(0)
    })?;
    let mut statement = conn.prepare(&page.text)?;
    let runs = statement
        .query_map(params_from_iter(&page.params), read_run)?
        .collect::<duckdb::Result<_>>()?;
    Ok(RunList {
        runs,
        total: total.max(0) as u64,
    })
}

/// Reads [`sql::RUN_COLUMNS`].
fn read_run(row: &Row) -> duckdb::Result<RunInfo> {
    Ok(RunInfo {
        id: row.get(0)?,
        experiment: row.get(1)?,
        experiment_name: row.get(2)?,
        variant: row.get(3)?,
        variant_name: row.get(4)?,
        seed: row.get(5)?,
        end_s: row.get(6)?,
        events: row.get(7)?,
        log_entries: row.get(8)?,
        log_sha256: row.get(9)?,
        peers: row.get(10)?,
        downloaders: row.get(11)?,
        completed: row.get(12)?,
        completion_p50_s: row.get(13)?,
        completion_p90_s: row.get(14)?,
        completion_p99_s: row.get(15)?,
        first_byte_p50_s: row.get(16)?,
        first_peer_p50_s: row.get(17)?,
        upload_utilization: row.get(18)?,
        fairness: row.get(19)?,
        rarest_copies: row.get(20)?,
        payload_bytes: row.get(21)?,
        overhead_bytes: row.get(22)?,
        duplicate_bytes: row.get(23)?,
    })
}

/// The run with ID `id`.
fn run_info(conn: &Connection, id: i64) -> Result<Option<RunInfo>> {
    let sql = format!(
        "SELECT {} {} WHERE r.run_id = ?",
        sql::RUN_COLUMNS,
        sql::RUNS_FROM
    );
    Ok(conn.query_row(&sql, [id], read_run).optional()?)
}

/// A run with its variant's settings, its peers by class and its traffic, if there is one
/// with ID `id`.
pub fn run(conn: &Connection, id: i64) -> Result<Option<RunDetail>> {
    let Some(run) = run_info(conn, id)? else {
        return Ok(None);
    };
    let vary: String = conn.query_row(
        "SELECT vary::VARCHAR FROM variants WHERE experiment_id = ? AND variant = ?",
        params![run.experiment, run.variant],
        |row| row.get(0),
    )?;
    let mut statement = conn.prepare(
        "SELECT class, behavior, starts, count(*), count(completed_s),
            median(completed_s - arrived_s), sum(downloaded)::UBIGINT, sum(uploaded)::UBIGINT
         FROM peers WHERE run_id = ?
         GROUP BY class, behavior, starts ORDER BY min(peer)",
    )?;
    let classes = statement
        .query_map([id], |row| {
            Ok(ClassSummary {
                class: row.get(0)?,
                behavior: row.get(1)?,
                starts: row.get(2)?,
                peers: count(row.get(3)?),
                completed: count(row.get(4)?),
                completion_p50_s: row.get(5)?,
                downloaded: row.get(6)?,
                uploaded: row.get(7)?,
            })
        })?
        .collect::<duckdb::Result<_>>()?;
    let mut statement = conn.prepare(
        "SELECT message, count, bytes, overhead_bytes FROM traffic WHERE run_id = ?
         ORDER BY bytes DESC, message",
    )?;
    let traffic = statement
        .query_map([id], |row| {
            Ok(TrafficInfo {
                message: row.get(0)?,
                count: row.get(1)?,
                bytes: row.get(2)?,
                overhead_bytes: row.get(3)?,
            })
        })?
        .collect::<duckdb::Result<_>>()?;
    Ok(Some(RunDetail {
        logged: logged(conn, Some((id, id)))?,
        vary: settings(&vary),
        run,
        classes,
        traffic,
    }))
}

/// A run's peers, in order, if there is a run with ID `id`.
pub fn peers(conn: &Connection, id: i64) -> Result<Option<Vec<PeerInfo>>> {
    if run_info(conn, id)?.is_none() {
        return Ok(None);
    }
    let mut statement = conn.prepare(
        "SELECT peer, class, starts, behavior, upload_capacity, download_capacity, arrived_s,
            first_peer_s, first_byte_s, completed_s, left_s, downloaded, uploaded, duplicate, bad
         FROM peers WHERE run_id = ? ORDER BY peer",
    )?;
    let peers = statement
        .query_map([id], |row| {
            Ok(PeerInfo {
                peer: row.get(0)?,
                class: row.get(1)?,
                starts: row.get(2)?,
                behavior: row.get(3)?,
                upload_capacity: row.get(4)?,
                download_capacity: row.get(5)?,
                arrived_s: row.get(6)?,
                first_peer_s: row.get(7)?,
                first_byte_s: row.get(8)?,
                completed_s: row.get(9)?,
                left_s: row.get(10)?,
                downloaded: row.get(11)?,
                uploaded: row.get(12)?,
                duplicate: row.get(13)?,
                bad: row.get(14)?,
            })
        })?
        .collect::<duckdb::Result<_>>()?;
    Ok(Some(peers))
}

/// About how many bins a run's throughput is split into.
const THROUGHPUT_BINS: f64 = 120.0;

/// A run over time, if there is a run with ID `id`: its chunks' availability, its
/// completions, and, when its event log was recorded, the Data it sent per second.
pub fn series(conn: &Connection, id: i64) -> Result<Option<RunSeries>> {
    let Some(run) = run_info(conn, id)? else {
        return Ok(None);
    };
    let mut statement = conn.prepare(
        "SELECT time_s, peers, rarest, mean FROM availability WHERE run_id = ? ORDER BY time_s",
    )?;
    let availability = statement
        .query_map([id], |row| {
            Ok(AvailabilitySample {
                time_s: row.get(0)?,
                peers: row.get(1)?,
                rarest: row.get(2)?,
                mean: row.get(3)?,
            })
        })?
        .collect::<duckdb::Result<_>>()?;
    let times = completion_times(conn, id)?;
    let completed = times.iter().zip(1..).map(|(&t, n)| (t, n)).collect();

    let bin_s = nice_step(run.end_s / THROUGHPUT_BINS);
    let mut statement = conn.prepare(
        "SELECT floor(time_ns / ?)::BIGINT AS bin, sum(bytes)::DOUBLE FROM events
         WHERE run_id = ? AND kind = 'sent' AND message = 'Data'
         GROUP BY bin ORDER BY bin",
    )?;
    let sent: BTreeMap<i64, f64> = statement
        .query_map(params![bin_s * 1e9, id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?
        .collect::<duckdb::Result<_>>()?;
    let throughput = match sent.last_key_value() {
        None => Vec::new(),
        Some((&last, _)) => (0..=last)
            .map(|bin| {
                let bytes = sent.get(&bin).copied().unwrap_or(0.0);
                (bin as f64 * bin_s, bytes / bin_s)
            })
            .collect(),
    };
    Ok(Some(RunSeries {
        availability,
        downloaders: run.downloaders,
        completed,
        throughput,
        bin_s,
    }))
}

/// When each downloader of run `id` completed, earliest first.
fn completion_times(conn: &Connection, id: i64) -> Result<Vec<f64>> {
    let mut statement = conn.prepare(
        "SELECT completed_s FROM peers
         WHERE run_id = ? AND starts = 'empty' AND completed_s IS NOT NULL ORDER BY completed_s",
    )?;
    Ok(statement
        .query_map([id], |row| row.get(0))?
        .collect::<duckdb::Result<_>>()?)
}

/// The smallest of 1, 2 or 5 times a power of ten that is at least `step`, and at least a
/// millisecond.
fn nice_step(step: f64) -> f64 {
    let step = step.max(0.001);
    let power = 10f64.powf(step.log10().floor());
    [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|m| m * power)
        .find(|&s| s >= step * (1.0 - 1e-9))
        .unwrap_or(10.0 * power)
}

/// One run's headline metrics, as the `runs` table holds them.
#[derive(Debug, Clone, PartialEq)]
struct Measured {
    seed: u64,
    metrics: RunMetrics,
}

/// The metrics of each run of variant `variant` of experiment `experiment`, by seed.
fn measured(conn: &Connection, experiment: i64, variant: u32) -> Result<Vec<Measured>> {
    let mut statement = conn.prepare(
        "SELECT seed, downloaders, completed, completion_p50_s, completion_p90_s,
            completion_p99_s, first_byte_p50_s, first_peer_p50_s, upload_utilization, fairness,
            rarest_copies, overhead_bytes, duplicate_bytes
         FROM runs WHERE experiment_id = ? AND variant = ? ORDER BY seed",
    )?;
    let secs = |s: Option<f64>| s.map(Duration::from_secs_f64);
    Ok(statement
        .query_map(params![experiment, variant], |row| {
            Ok(Measured {
                seed: row.get(0)?,
                metrics: RunMetrics {
                    downloaders: row.get(1)?,
                    completed: row.get(2)?,
                    completion_p50: secs(row.get(3)?),
                    completion_p90: secs(row.get(4)?),
                    completion_p99: secs(row.get(5)?),
                    first_byte_p50: secs(row.get(6)?),
                    first_peer_p50: secs(row.get(7)?),
                    upload_utilization: row.get(8)?,
                    fairness: row.get(9)?,
                    rarest_copies: row.get(10)?,
                    overhead: row.get(11)?,
                    duplicate: row.get(12)?,
                },
            })
        })?
        .collect::<duckdb::Result<_>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_are_round() {
        assert_eq!(nice_step(0.7), 1.0);
        assert_eq!(nice_step(1.0), 1.0);
        assert_eq!(nice_step(1.2), 2.0);
        assert_eq!(nice_step(3.0), 5.0);
        assert_eq!(nice_step(30.0), 50.0);
        assert_eq!(nice_step(0.0), 0.001);
        assert!((nice_step(0.013) - 0.02).abs() < 1e-12);
    }

    #[test]
    fn varied_settings_read_as_the_scenario_writes_them() {
        assert_eq!(
            settings(r#"{"strategies.upload":"propshare","limits.requests":16}"#),
            [
                Setting {
                    key: "limits.requests".into(),
                    value: "16".into()
                },
                Setting {
                    key: "strategies.upload".into(),
                    value: "propshare".into()
                },
            ]
        );
        assert!(settings("{}").is_empty());
    }
}
