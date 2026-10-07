//! Golden baselines (D-077): a scenario's headline metrics for each seed, saved so that later
//! runs can be checked against them.
//!
//! A check compares the mean of each metric over some seeds with the baseline's mean over the
//! same seeds, so both sides see the same random worlds (D-073). Each metric has a tolerance:
//! a change within it passes, so that small, deliberate changes to strategies don't fail the
//! check, while a significant regression in speed, fairness or overhead does.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::engine::RunResult;
use crate::scenario::Scenario;

/// A scenario's saved metrics, by seed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Baseline {
    /// The scenario's name.
    pub scenario: String,
    /// Each seed's metrics.
    pub runs: BTreeMap<u64, Metrics>,
}

/// The metrics a baseline keeps for one run. Times are seconds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Metrics {
    /// Downloaders that completed.
    pub completed: usize,
    /// The median completion time.
    pub completion_p50_s: Option<f64>,
    /// The 90th percentile completion time.
    pub completion_p90_s: Option<f64>,
    /// The median time to first byte.
    pub first_byte_p50_s: Option<f64>,
    /// The upload utilization.
    pub upload_utilization: Option<f64>,
    /// Jain's fairness index.
    pub fairness: Option<f64>,
    /// Bytes sent that weren't payload.
    pub overhead_bytes: u64,
    /// Endgame duplicate bytes.
    pub duplicate_bytes: u64,
}

impl Metrics {
    /// The metrics of `result`, a run of `scenario`.
    pub fn of(result: &RunResult, scenario: &Scenario) -> Self {
        let m = result.metrics(scenario);
        let secs = |d: Option<std::time::Duration>| d.map(|d| d.as_secs_f64());
        Self {
            completed: m.completed,
            completion_p50_s: secs(m.completion_p50),
            completion_p90_s: secs(m.completion_p90),
            first_byte_p50_s: secs(m.first_byte_p50),
            upload_utilization: m.upload_utilization,
            fairness: m.fairness,
            overhead_bytes: m.overhead,
            duplicate_bytes: m.duplicate,
        }
    }
}

/// Which way a metric gets worse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Worse {
    Higher,
    Lower,
}

/// How far a metric may move the worse way before it counts as a regression.
#[derive(Debug, Clone, Copy)]
enum Tolerance {
    /// A fraction of the baseline.
    Relative(f64),
    /// An absolute amount.
    Absolute(f64),
}

type Extract = fn(&Metrics) -> Option<f64>;

/// Each checked metric: its name, how to read it, which way is worse, and its tolerance.
const CHECKS: [(&str, Extract, Worse, Tolerance); 8] = [
    (
        "completed",
        |m| Some(m.completed as f64),
        Worse::Lower,
        Tolerance::Absolute(0.0),
    ),
    (
        "completion p50 (s)",
        |m| m.completion_p50_s,
        Worse::Higher,
        Tolerance::Relative(0.10),
    ),
    (
        "completion p90 (s)",
        |m| m.completion_p90_s,
        Worse::Higher,
        Tolerance::Relative(0.10),
    ),
    (
        "first byte p50 (s)",
        |m| m.first_byte_p50_s,
        Worse::Higher,
        Tolerance::Relative(0.20),
    ),
    (
        "upload utilization",
        |m| m.upload_utilization,
        Worse::Lower,
        Tolerance::Relative(0.10),
    ),
    (
        "fairness",
        |m| m.fairness,
        Worse::Lower,
        Tolerance::Absolute(0.05),
    ),
    (
        "overhead (bytes)",
        |m| Some(m.overhead_bytes as f64),
        Worse::Higher,
        Tolerance::Relative(0.10),
    ),
    (
        "duplicate (bytes)",
        |m| Some(m.duplicate_bytes as f64),
        Worse::Higher,
        // Endgame duplicates are rare and lumpy: allow a few chunks' worth.
        Tolerance::Absolute(256.0 * 1024.0),
    ),
];

/// One metric compared with its baseline.
#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    /// The metric.
    pub metric: &'static str,
    /// The baseline's mean over the seeds compared.
    pub baseline: Option<f64>,
    /// The current mean over the same seeds.
    pub current: Option<f64>,
    /// The worst current value that passes.
    pub limit: Option<f64>,
    /// Whether the metric got worse by more than its tolerance.
    pub regressed: bool,
    /// Whether it got better by more than its tolerance, so the baseline is out of date.
    pub improved: bool,
}

/// The mean of `metric` over `runs`, if every run has it.
fn mean<'a>(runs: impl Iterator<Item = &'a Metrics>, metric: Extract) -> Option<f64> {
    let values: Option<Vec<f64>> = runs.map(metric).collect();
    let values = values?;
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

impl Baseline {
    /// Compares `current`, metrics by seed, with this baseline over the same seeds. Fails if
    /// the baseline lacks one of the seeds.
    pub fn compare(&self, current: &BTreeMap<u64, Metrics>) -> Result<Vec<Check>, String> {
        let baseline: Vec<&Metrics> = current
            .keys()
            .map(|seed| {
                self.runs
                    .get(seed)
                    .ok_or_else(|| format!("the baseline has no run with seed {seed}"))
            })
            .collect::<Result<_, _>>()?;
        Ok(CHECKS
            .iter()
            .map(|&(metric, extract, worse, tolerance)| {
                let base = mean(baseline.iter().copied(), extract);
                let now = mean(current.values(), extract);
                let limit = base.map(|b| {
                    let slack = match tolerance {
                        Tolerance::Relative(fraction) => b.abs() * fraction,
                        Tolerance::Absolute(amount) => amount,
                    };
                    match worse {
                        Worse::Higher => b + slack,
                        Worse::Lower => b - slack,
                    }
                });
                let (regressed, improved) = match (base, now, limit) {
                    (Some(base), Some(now), Some(limit)) => match worse {
                        Worse::Higher => (now > limit, now < base - (limit - base)),
                        Worse::Lower => (now < limit, now > base + (base - limit)),
                    },
                    (None, None, _) => (false, false),
                    // A metric that appeared is an improvement; one that vanished, a regression.
                    (None, Some(_), _) => (false, true),
                    _ => (true, false),
                };
                Check {
                    metric,
                    baseline: base,
                    current: now,
                    limit,
                    regressed,
                    improved,
                }
            })
            .collect())
    }
}

/// A table of checks.
pub fn table(checks: &[Check]) -> String {
    let value = |v: Option<f64>| v.map_or_else(|| "-".to_owned(), |v| format!("{v:.3}"));
    let mut out = format!(
        "{:<20} {:>14} {:>14} {:>14}\n",
        "metric", "baseline", "current", "limit"
    );
    for check in checks {
        let flag = if check.regressed {
            "  REGRESSED"
        } else if check.improved {
            "  improved"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "{:<20} {:>14} {:>14} {:>14}{flag}",
            check.metric,
            value(check.baseline),
            value(check.current),
            value(check.limit),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics(p50: f64, fairness: f64) -> Metrics {
        Metrics {
            completed: 10,
            completion_p50_s: Some(p50),
            completion_p90_s: Some(p50 * 2.0),
            first_byte_p50_s: Some(1.0),
            upload_utilization: Some(0.5),
            fairness: Some(fairness),
            overhead_bytes: 1000,
            duplicate_bytes: 0,
        }
    }

    fn baseline() -> Baseline {
        Baseline {
            scenario: "test".into(),
            runs: [(1, metrics(10.0, 0.9)), (2, metrics(20.0, 0.8))].into(),
        }
    }

    fn check<'a>(checks: &'a [Check], metric: &str) -> &'a Check {
        checks.iter().find(|c| c.metric == metric).unwrap()
    }

    #[test]
    fn the_same_metrics_pass() {
        let base = baseline();
        let checks = base.compare(&base.runs).unwrap();
        assert!(checks.iter().all(|c| !c.regressed && !c.improved));
        let p50 = check(&checks, "completion p50 (s)");
        assert_eq!(p50.baseline, Some(15.0));
        assert!((p50.limit.unwrap() - 16.5).abs() < 1e-9);
    }

    #[test]
    fn only_the_seeds_compared_count() {
        let current = [(2, metrics(21.0, 0.8))].into();
        let checks = baseline().compare(&current).unwrap();
        let p50 = check(&checks, "completion p50 (s)");
        assert_eq!(p50.baseline, Some(20.0));
        assert!(!p50.regressed, "5% slower is within tolerance");
        let missing = [(3, metrics(20.0, 0.8))].into();
        assert!(baseline().compare(&missing).is_err());
    }

    #[test]
    fn regressions_and_improvements_are_flagged() {
        let current = [(1, metrics(12.0, 0.8)), (2, metrics(22.0, 0.7))].into();
        let checks = baseline().compare(&current).unwrap();
        assert!(check(&checks, "completion p50 (s)").regressed, "17 > 16.5");
        assert!(check(&checks, "fairness").regressed, "0.75 < 0.80");
        assert!(!check(&checks, "overhead (bytes)").regressed);

        let current = [(1, metrics(5.0, 0.95)), (2, metrics(10.0, 0.9))].into();
        let checks = baseline().compare(&current).unwrap();
        assert!(check(&checks, "completion p50 (s)").improved);
        assert!(check(&checks, "fairness").improved);
        assert!(checks.iter().all(|c| !c.regressed));
    }

    #[test]
    fn a_vanished_metric_regresses() {
        let mut none = metrics(10.0, 0.9);
        none.completion_p50_s = None;
        let current = [(1, none)].into();
        let checks = baseline().compare(&current).unwrap();
        assert!(check(&checks, "completion p50 (s)").regressed);
    }
}
