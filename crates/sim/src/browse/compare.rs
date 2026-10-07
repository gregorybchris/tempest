//! Comparisons of variants, from any experiments, with medians across seeds and their
//! confidence intervals (D-073, D-167): metric by metric, and over time.

use duckdb::{Connection, OptionalExt, params};
use tempest_sim_api::data::{
    BandPoint, Cell, ComparedVariant, Comparison, ComparisonSeries, Difference, Estimate,
    MetricRow, MetricUnit, SeriesBands, Verdict,
};
use tempest_sim_api::query::{CompareQuery, VariantRef};

use super::{BrowseError, Measured, Result, completion_times, measured};
use crate::stats::{self, Bootstrap, Meaningful, Metric, Samples};

/// What a metric's values mean.
pub fn unit(metric: Metric) -> MetricUnit {
    match metric {
        Metric::Completed | Metric::Utilization | Metric::Fairness => MetricUnit::Fraction,
        Metric::CompletionP50 | Metric::CompletionP90 | Metric::FirstByteP50 => MetricUnit::Seconds,
        Metric::RarestCopies => MetricUnit::Number,
    }
}

/// The compared variant `v`, with its runs' metrics.
fn load(conn: &Connection, v: VariantRef) -> Result<(ComparedVariant, Vec<Measured>)> {
    let names: Option<(String, String)> = conn
        .query_row(
            "SELECT e.name, v.name FROM variants v JOIN experiments e USING (experiment_id)
             WHERE v.experiment_id = ? AND v.variant = ?",
            params![v.experiment, v.variant],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let (experiment_name, name) =
        names.ok_or_else(|| BrowseError::NotFound(format!("there is no variant {v}")))?;
    let runs = measured(conn, v.experiment, v.variant)?;
    let variant = ComparedVariant {
        experiment: v.experiment,
        experiment_name,
        variant: v.variant,
        name,
        seeds: runs.iter().map(|r| r.seed).collect(),
    };
    Ok((variant, runs))
}

/// Each metric's value in each run that has one, by seed.
fn samples(name: &str, runs: &[Measured]) -> Samples {
    let values = Metric::ALL
        .iter()
        .map(|&metric| {
            let by_seed = runs
                .iter()
                .filter_map(|r| Some((r.seed, metric.value(&r.metrics)?)))
                .collect();
            (metric, by_seed)
        })
        .collect();
    Samples {
        name: name.to_owned(),
        values,
    }
}

fn estimate(e: stats::Estimate) -> Estimate {
    Estimate {
        n: u32::try_from(e.n).unwrap_or(u32::MAX),
        median: e.median,
        low: e.interval.low,
        high: e.interval.high,
    }
}

/// The variants `query` names, each compared with its reference metric by metric.
pub fn compare(conn: &Connection, query: &CompareQuery) -> Result<Comparison> {
    let loaded = query
        .variants
        .iter()
        .map(|&v| load(conn, v))
        .collect::<Result<Vec<_>>>()?;
    let samples: Vec<Samples> = loaded
        .iter()
        .map(|(variant, runs)| samples(&variant.name, runs))
        .collect();
    let (bootstrap, rule) = (Bootstrap::default(), Meaningful::default());
    let rows = stats::compare(&samples, query.reference, bootstrap, rule);
    let metrics = rows
        .chunks(samples.len())
        .map(|rows| MetricRow {
            name: rows[0].metric.name().to_owned(),
            unit: unit(rows[0].metric),
            lower_is_better: rows[0].metric.lower_is_better(),
            cells: rows
                .iter()
                .map(|row| Cell {
                    estimate: row.estimate.map(estimate),
                    difference: row.difference.map(|d| Difference {
                        estimate: estimate(d.estimate),
                        relative: d.relative,
                        verdict: match d.verdict {
                            stats::Verdict::Meaningful => Verdict::Meaningful,
                            stats::Verdict::NotMeaningful => Verdict::NotMeaningful,
                            stats::Verdict::TooFewPairs => Verdict::TooFewPairs,
                        },
                    }),
                })
                .collect(),
        })
        .collect();
    Ok(Comparison {
        variants: loaded.into_iter().map(|(variant, _)| variant).collect(),
        reference: query.reference,
        confidence: bootstrap.confidence,
        min_pairs: u32::try_from(rule.min_pairs).unwrap_or(u32::MAX),
        min_effect: rule.min_effect,
        metrics,
    })
}

/// Moments a band is estimated at, evenly spaced.
const BAND_POINTS: usize = 80;

/// Bands resample less than single estimates, since each draws one per moment.
const BAND_RESAMPLES: u32 = 1000;

/// One run over time: its completions and its availability samples.
struct Timeline {
    downloaders: u32,
    /// When each downloader completed, earliest first.
    completions: Vec<f64>,
    /// (time, peers present, present peers holding the rarest chunk), in time order.
    samples: Vec<(f64, f64, f64)>,
}

impl Timeline {
    fn completed(&self, t: f64) -> Option<f64> {
        let done = self.completions.partition_point(|&c| c <= t);
        (self.downloaders > 0).then(|| done as f64 / f64::from(self.downloaders))
    }

    /// The last sample at or before `t`.
    fn sample(&self, t: f64) -> Option<(f64, f64, f64)> {
        let i = self.samples.partition_point(|s| s.0 <= t);
        i.checked_sub(1).map(|i| self.samples[i])
    }
}

fn timelines(conn: &Connection, v: &ComparedVariant) -> Result<Vec<Timeline>> {
    let mut statement = conn.prepare(
        "SELECT run_id, downloaders FROM runs WHERE experiment_id = ? AND variant = ?
         ORDER BY seed",
    )?;
    let runs: Vec<(i64, u32)> = statement
        .query_map(params![v.experiment, v.variant], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?
        .collect::<duckdb::Result<_>>()?;
    let mut samples = conn.prepare(
        "SELECT time_s, peers, rarest FROM availability WHERE run_id = ? ORDER BY time_s",
    )?;
    runs.into_iter()
        .map(|(id, downloaders)| {
            Ok(Timeline {
                downloaders,
                completions: completion_times(conn, id)?,
                samples: samples
                    .query_map([id], |row| {
                        Ok((
                            row.get(0)?,
                            f64::from(row.get::<_, i32>(1)?),
                            f64::from(row.get::<_, i32>(2)?),
                        ))
                    })?
                    .collect::<duckdb::Result<_>>()?,
            })
        })
        .collect()
}

/// The median across runs of `value` at each of `points` moments from 0 to `end`, with its
/// interval. Moments where no run has a value are left out.
fn band(
    runs: &[Timeline],
    end: f64,
    bootstrap: Bootstrap,
    value: impl Fn(&Timeline, f64) -> Option<f64>,
) -> Vec<BandPoint> {
    (0..=BAND_POINTS)
        .filter_map(|i| {
            let t = end * i as f64 / BAND_POINTS as f64;
            let values: Vec<f64> = runs.iter().filter_map(|run| value(run, t)).collect();
            let (min, max) = values
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &v| {
                    (lo.min(v), hi.max(v))
                });
            // Where every run agrees, as before the first completion, there is nothing to
            // resample.
            let (median, low, high) = if min == max {
                (min, min, max)
            } else {
                let e = bootstrap.median(&values)?;
                (e.median, e.interval.low, e.interval.high)
            };
            (!values.is_empty()).then_some(BandPoint {
                t,
                median,
                low,
                high,
            })
        })
        .collect()
}

/// The variants `query` names over time: the fraction of downloaders completed, the
/// rarest chunk's copies and the peers present, each a median across seeds with its interval.
/// A quantity no run has two moments of is left out.
pub fn compare_series(conn: &Connection, query: &CompareQuery) -> Result<ComparisonSeries> {
    let variants = query
        .variants
        .iter()
        .map(|&v| Ok(load(conn, v)?.0))
        .collect::<Result<Vec<_>>>()?;
    let runs = variants
        .iter()
        .map(|v| timelines(conn, v))
        .collect::<Result<Vec<_>>>()?;
    let all = || runs.iter().flatten();
    let last_completion = all()
        .filter_map(|r| r.completions.last().copied())
        .fold(0.0, f64::max);
    let last_sample = all()
        .filter_map(|r| r.samples.last().map(|s| s.0))
        .fold(0.0, f64::max);
    let bootstrap = Bootstrap {
        resamples: BAND_RESAMPLES,
        ..Bootstrap::default()
    };
    // A little past the last completion, so every curve is seen to level off.
    let completion_end = last_completion * 1.05;
    let mut series = Vec::new();
    let mut add =
        |key: &str, title: &str, unit, end: f64, value: &dyn Fn(&Timeline, f64) -> Option<f64>| {
            let variants: Vec<Vec<BandPoint>> = runs
                .iter()
                .map(|runs| band(runs, end, bootstrap, value))
                .collect();
            // A curve needs two moments; runs too short to sample twice have none.
            if end > 0.0 && variants.iter().any(|points| points.len() > 1) {
                series.push(SeriesBands {
                    key: key.to_owned(),
                    title: title.to_owned(),
                    unit,
                    variants,
                });
            }
        };
    add(
        "completed",
        "Downloaders completed",
        MetricUnit::Fraction,
        completion_end,
        &|run, t| run.completed(t),
    );
    add(
        "rarest",
        "Copies of the rarest chunk",
        MetricUnit::Number,
        last_sample,
        &|run, t| run.sample(t).map(|s| s.2),
    );
    add(
        "present",
        "Peers present",
        MetricUnit::Number,
        last_sample,
        &|run, t| run.sample(t).map(|s| s.1),
    );
    Ok(ComparisonSeries {
        variants,
        confidence: bootstrap.confidence,
        series,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn timeline(completions: &[f64], downloaders: u32) -> Timeline {
        Timeline {
            downloaders,
            completions: completions.to_vec(),
            samples: vec![(0.0, 1.0, 0.0), (10.0, 4.0, 1.0), (20.0, 4.0, 3.0)],
        }
    }

    #[test]
    fn timelines_step_at_completions_and_samples() {
        let run = timeline(&[5.0, 15.0], 4);
        assert_eq!(run.completed(0.0), Some(0.0));
        assert_eq!(run.completed(5.0), Some(0.25));
        assert_eq!(run.completed(100.0), Some(0.5));
        assert_eq!(timeline(&[], 0).completed(1.0), None);
        assert_eq!(run.sample(-1.0), None);
        assert_eq!(run.sample(12.0), Some((10.0, 4.0, 1.0)));
    }

    #[test]
    fn bands_hold_the_median_and_agree_where_runs_agree() {
        let runs = [
            timeline(&[10.0], 1),
            timeline(&[20.0], 1),
            timeline(&[30.0], 1),
        ];
        let points = band(&runs, 40.0, Bootstrap::default(), |r, t| r.completed(t));
        assert_eq!(points.len(), BAND_POINTS + 1);
        let at = |t: f64| points.iter().find(|p| p.t == t).copied().unwrap();
        assert_eq!((at(0.0).low, at(0.0).median, at(0.0).high), (0.0, 0.0, 0.0));
        assert_eq!(at(40.0).median, 1.0);
        let middle = at(25.0);
        assert_eq!(middle.median, 1.0);
        assert!(middle.low <= middle.median && middle.median <= middle.high);
        for p in &points {
            assert!(p.low <= p.median && p.median <= p.high, "{p:?}");
        }
    }

    #[test]
    fn every_metric_has_a_unit() {
        for metric in Metric::ALL {
            let unit = unit(metric);
            assert_eq!(unit == MetricUnit::Seconds, metric.lower_is_better());
        }
    }

    #[test]
    fn samples_skip_runs_without_a_value() {
        let measured = |seed, completed| Measured {
            seed,
            metrics: crate::summary::RunMetrics {
                downloaders: 2,
                completed,
                completion_p50: None,
                completion_p90: None,
                completion_p99: None,
                first_byte_p50: None,
                first_peer_p50: None,
                upload_utilization: Some(0.5),
                fairness: None,
                rarest_copies: None,
                overhead: 0,
                duplicate: 0,
            },
        };
        let s = samples("v", &[measured(1, 2), measured(2, 1)]);
        assert_eq!(
            s.values[&Metric::Completed],
            BTreeMap::from([(1, 1.0), (2, 0.5)])
        );
        assert!(s.values[&Metric::CompletionP50].is_empty());
    }
}
