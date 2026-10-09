//! Golden baselines (D-077, D-163, D-171): the headline metrics of golden scenarios for each
//! seed, saved so that later runs can be checked against them.
//!
//! Golden scenarios are the scenario files in one directory, and each one's baseline is the JSON
//! file of the same name in another. A check runs some of a scenario's seeds and compares each
//! metric with the baseline on the same seeds, so both sides see the same random worlds (D-073).
//! Each metric has a tolerance: a change within it passes, so that small, deliberate changes to
//! strategies don't fail the check, while a larger regression in speed, availability, fairness
//! or overhead does.
//!
//! A [quick](Mode::Quick) check runs the first few seeds and compares means. A
//! [long](Mode::Long) one runs every saved seed and compares seed by seed with the statistics
//! of [`crate::stats`], so one unusual seed can't fail it and noise can't pass a real change.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::engine::{RunOptions, RunResult};
use crate::experiment::run_all;
use crate::scenario::{Scenario, ScenarioError};
use crate::stats::{self, Bootstrap, Estimate};
use crate::summary::RunMetrics;

/// Seeds a quick check runs, at most.
pub const QUICK_SEEDS: u32 = 3;

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
    /// The mean availability of the rarest chunk.
    pub rarest_copies: Option<f64>,
    /// Bytes sent that weren't payload.
    pub overhead_bytes: u64,
    /// Endgame duplicate bytes.
    pub duplicate_bytes: u64,
}

impl Metrics {
    /// The metrics of `result`, a run of `scenario`.
    pub fn of(result: &RunResult, scenario: &Scenario) -> Self {
        Self::from(&result.metrics(scenario))
    }
}

impl From<&RunMetrics> for Metrics {
    fn from(m: &RunMetrics) -> Self {
        let secs = |d: Option<std::time::Duration>| d.map(|d| d.as_secs_f64());
        Self {
            completed: m.completed,
            completion_p50_s: secs(m.completion_p50),
            completion_p90_s: secs(m.completion_p90),
            first_byte_p50_s: secs(m.first_byte_p50),
            upload_utilization: m.upload_utilization,
            fairness: m.fairness,
            rarest_copies: m.rarest_copies,
            overhead_bytes: m.overhead,
            duplicate_bytes: m.duplicate,
        }
    }
}

/// Why golden scenarios or their baselines can't be used.
#[derive(Debug, thiserror::Error)]
pub enum BaselineError {
    /// A file or directory couldn't be read or written.
    #[error("{path}: {source}")]
    Io {
        /// The file or directory.
        path: String,
        /// Why.
        source: std::io::Error,
    },
    /// A baseline file isn't well-formed.
    #[error("{path}: {source}")]
    Json {
        /// The file.
        path: String,
        /// Why.
        source: serde_json::Error,
    },
    /// A golden scenario can't be loaded or run.
    #[error(transparent)]
    Scenario(#[from] ScenarioError),
    /// A golden scenario and its baseline don't fit together.
    #[error("{0}")]
    Mismatch(String),
}

fn io_error(path: &Path) -> impl FnOnce(std::io::Error) -> BaselineError + '_ {
    move |source| BaselineError::Io {
        path: path.display().to_string(),
        source,
    }
}

impl Baseline {
    /// Reads a baseline file.
    pub fn load(path: &Path) -> Result<Self, BaselineError> {
        let text = std::fs::read_to_string(path).map_err(io_error(path))?;
        serde_json::from_str(&text).map_err(|source| BaselineError::Json {
            path: path.display().to_string(),
            source,
        })
    }

    /// Writes this baseline to `path`.
    pub fn save(&self, path: &Path) -> Result<(), BaselineError> {
        let json = serde_json::to_string_pretty(self).expect("metrics serialize");
        std::fs::write(path, json + "\n").map_err(io_error(path))
    }
}

/// How thorough a check is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The first [`QUICK_SEEDS`] seeds, compared by their means. Fast enough for every change.
    Quick,
    /// Every saved seed, compared seed by seed. For more certainty, between milestones.
    Long,
}

impl Mode {
    /// The seeds this mode runs of `scenario`.
    pub fn seeds(self, scenario: &Scenario) -> Vec<u64> {
        let count = match self {
            Self::Quick => scenario.seeds.min(QUICK_SEEDS),
            Self::Long => scenario.seeds,
        };
        (1..=u64::from(count)).collect()
    }

    /// How this mode compares runs with a baseline.
    pub fn method(self) -> Method {
        match self {
            Self::Quick => Method::Mean,
            Self::Long => Method::Paired(Bootstrap::default()),
        }
    }
}

/// How runs are compared with a baseline over the same seeds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Method {
    /// The mean over the seeds against the baseline's mean. A metric regresses when the mean
    /// moves the worse way by more than its tolerance.
    Mean,
    /// Seed by seed: the median of the per-seed changes, with its bootstrap confidence
    /// interval. A metric regresses when the median change is worse than its tolerance and
    /// the interval excludes no change.
    Paired(Bootstrap),
}

/// Which way a metric gets worse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Worse {
    Higher,
    Lower,
}

impl Worse {
    /// +1 if higher is worse, −1 if lower is.
    fn sign(self) -> f64 {
        match self {
            Self::Higher => 1.0,
            Self::Lower => -1.0,
        }
    }
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
const CHECKS: [(&str, Extract, Worse, Tolerance); 9] = [
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
        "rarest copies",
        |m| m.rarest_copies,
        Worse::Lower,
        // The rarest chunk's count moves in whole peers: allow more.
        Tolerance::Relative(0.20),
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
    /// The baseline over the seeds compared: their mean, or with [`Method::Paired`], their
    /// median.
    pub baseline: Option<f64>,
    /// The current runs, summarized the same way.
    pub current: Option<f64>,
    /// The worst current value that passes.
    pub limit: Option<f64>,
    /// With [`Method::Paired`], the median per-seed change (current − baseline).
    pub change: Option<Estimate>,
    /// Whether the metric got worse by more than its tolerance.
    pub regressed: bool,
    /// Whether it got better by more than its tolerance, so the baseline is out of date.
    pub improved: bool,
}

/// The mean of `values`, if there are any.
fn mean(values: &[f64]) -> Option<f64> {
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

impl Baseline {
    /// Compares `current`, metrics by seed, with this baseline over the same seeds. Fails if
    /// the baseline lacks one of the seeds.
    pub fn compare(
        &self,
        current: &BTreeMap<u64, Metrics>,
        method: Method,
    ) -> Result<Vec<Check>, BaselineError> {
        let pairs: Vec<(&Metrics, &Metrics)> = current
            .iter()
            .map(|(seed, now)| {
                let base = self.runs.get(seed).ok_or_else(|| {
                    BaselineError::Mismatch(format!(
                        "the baseline of {} has no run with seed {seed}",
                        self.scenario
                    ))
                })?;
                Ok((base, now))
            })
            .collect::<Result<_, BaselineError>>()?;
        Ok(CHECKS
            .iter()
            .map(|&(metric, extract, worse, tolerance)| {
                let values: Vec<(Option<f64>, Option<f64>)> = pairs
                    .iter()
                    .map(|(base, now)| (extract(base), extract(now)))
                    .collect();
                check(metric, &values, worse, tolerance, method)
            })
            .collect())
    }
}

/// Compares one metric's (baseline, current) values, one pair per seed.
fn check(
    metric: &'static str,
    values: &[(Option<f64>, Option<f64>)],
    worse: Worse,
    tolerance: Tolerance,
    method: Method,
) -> Check {
    // A metric that vanished on some seed is a regression; one that appeared, an improvement.
    let vanished = values.iter().any(|v| matches!(v, (Some(_), None)));
    let appeared = values.iter().any(|v| matches!(v, (None, Some(_))));
    // The seeds where both have the metric are compared; the others count only as above.
    let both: Vec<(f64, f64)> = values.iter().filter_map(|&(b, n)| b.zip(n)).collect();
    let (base, now, change, shift) = match method {
        Method::Mean => {
            let base = mean(&both.iter().map(|p| p.0).collect::<Vec<_>>());
            let now = mean(&both.iter().map(|p| p.1).collect::<Vec<_>>());
            // The shift the worse way, and the interval of it nearest and furthest from 0.
            let shift = base.zip(now).map(|(b, n)| {
                let d = worse.sign() * (n - b);
                (d, d, d)
            });
            (base, now, None, shift)
        }
        Method::Paired(bootstrap) => {
            let base = stats::median(&both.iter().map(|p| p.0).collect::<Vec<_>>());
            let now = stats::median(&both.iter().map(|p| p.1).collect::<Vec<_>>());
            let change = bootstrap.median(&both.iter().map(|(b, n)| n - b).collect::<Vec<_>>());
            let shift = change.map(|c| {
                let (low, high) = (
                    worse.sign() * c.interval.low,
                    worse.sign() * c.interval.high,
                );
                (worse.sign() * c.median, low.min(high), low.max(high))
            });
            (base, now, change, shift)
        }
    };
    let slack = base.map(|b| match tolerance {
        Tolerance::Relative(fraction) => b.abs() * fraction,
        Tolerance::Absolute(amount) => amount,
    });
    let (worsened, bettered) = match shift.zip(slack) {
        Some(((shift, nearest, furthest), slack)) => (
            shift > slack && nearest > 0.0,
            shift < -slack && furthest < 0.0,
        ),
        None => (false, false),
    };
    let regressed = vanished || worsened;
    Check {
        metric,
        baseline: base,
        current: now,
        limit: base.zip(slack).map(|(b, s)| b + worse.sign() * s),
        change,
        regressed,
        improved: !regressed && (appeared || bettered),
    }
}

/// A table of checks.
pub fn table(checks: &[Check]) -> String {
    let value = |v: Option<f64>| v.map_or_else(|| "-".to_owned(), |v| format!("{v:.3}"));
    let paired = checks.iter().any(|c| c.change.is_some());
    let mut out = format!(
        "{:<20} {:>14} {:>14} {:>14}",
        "metric", "baseline", "current", "limit"
    );
    if paired {
        let _ = write!(out, " {:>14} {:>29}", "change", "95% CI");
    }
    out.push('\n');
    for check in checks {
        let _ = write!(
            out,
            "{:<20} {:>14} {:>14} {:>14}",
            check.metric,
            value(check.baseline),
            value(check.current),
            value(check.limit),
        );
        if paired {
            let (change, interval) = check.change.map_or_else(
                || ("-".to_owned(), "-".to_owned()),
                |c| {
                    (
                        format!("{:+.3}", c.median),
                        format!("{:+.3} – {:+.3}", c.interval.low, c.interval.high),
                    )
                },
            );
            let _ = write!(out, " {change:>14} {interval:>29}");
        }
        if check.regressed {
            out.push_str("  REGRESSED");
        } else if check.improved {
            out.push_str("  improved");
        }
        out.push('\n');
    }
    out
}

/// Runs `scenario`'s only variant with each of `seeds`, on up to `threads` threads at once.
pub fn measure(
    scenario: &Scenario,
    seeds: &[u64],
    threads: NonZeroUsize,
) -> Result<BTreeMap<u64, Metrics>, ScenarioError> {
    Ok(
        run_all(scenario, seeds, threads, RunOptions::default(), &())?
            .iter()
            .map(|r| (r.seed, Metrics::of(r, scenario)))
            .collect(),
    )
}

/// A golden scenario, and where its baseline is saved.
#[derive(Debug, Clone)]
pub struct Golden {
    /// The scenario.
    pub scenario: Scenario,
    /// Its baseline file.
    pub baseline: PathBuf,
}

/// The golden scenarios in `scenarios`, in file-name order, each with its baseline in
/// `baselines`: `NAME.yaml` holds the scenario named `NAME`, saved as `NAME.json`. A golden
/// scenario has one variant.
pub fn golden(scenarios: &Path, baselines: &Path) -> Result<Vec<Golden>, BaselineError> {
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(scenarios).map_err(io_error(scenarios))? {
        let path = entry.map_err(io_error(scenarios))?.path();
        if path.extension().is_some_and(|e| e == "yaml") {
            paths.push(path);
        }
    }
    paths.sort();
    paths
        .iter()
        .map(|path| {
            let scenario = Scenario::load(path)?;
            let stem = path.file_stem().unwrap_or_default().to_string_lossy();
            if scenario.name != stem {
                return Err(BaselineError::Mismatch(format!(
                    "{}: a golden scenario's name must match its file name",
                    path.display()
                )));
            }
            if scenario.variants().len() != 1 {
                return Err(BaselineError::Mismatch(format!(
                    "{}: a golden scenario has no experiment",
                    path.display()
                )));
            }
            let baseline = baselines.join(format!("{stem}.json"));
            Ok(Golden { scenario, baseline })
        })
        .collect()
}

impl Golden {
    /// Checks the scenario against its baseline.
    pub fn check(&self, mode: Mode, threads: NonZeroUsize) -> Result<Vec<Check>, BaselineError> {
        let baseline = Baseline::load(&self.baseline)?;
        if baseline.scenario != self.scenario.name {
            return Err(BaselineError::Mismatch(format!(
                "{} is the baseline of {}, not {}",
                self.baseline.display(),
                baseline.scenario,
                self.scenario.name
            )));
        }
        let current = measure(&self.scenario, &mode.seeds(&self.scenario), threads)?;
        baseline.compare(&current, mode.method())
    }

    /// Runs every seed and saves the metrics as the scenario's baseline.
    pub fn update(&self, threads: NonZeroUsize) -> Result<(), BaselineError> {
        let seeds = Mode::Long.seeds(&self.scenario);
        let baseline = Baseline {
            scenario: self.scenario.name.clone(),
            runs: measure(&self.scenario, &seeds, threads)?,
        };
        baseline.save(&self.baseline)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn metrics(p50: f64, fairness: f64) -> Metrics {
        Metrics {
            completed: 10,
            completion_p50_s: Some(p50),
            completion_p90_s: Some(p50 * 2.0),
            first_byte_p50_s: Some(1.0),
            upload_utilization: Some(0.5),
            fairness: Some(fairness),
            rarest_copies: Some(4.0),
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

    /// A baseline of `n` seeds with varied completion times.
    fn many(n: u64) -> Baseline {
        Baseline {
            scenario: "test".into(),
            runs: (1..=n)
                .map(|seed| (seed, metrics(10.0 + (seed % 7) as f64, 0.9)))
                .collect(),
        }
    }

    /// `base` with each seed's completion times scaled by `factor(seed)`.
    fn slowed(base: &Baseline, factor: impl Fn(u64) -> f64) -> BTreeMap<u64, Metrics> {
        base.runs
            .iter()
            .map(|(&seed, m)| {
                let scale = |v: Option<f64>| v.map(|v| v * factor(seed));
                let mut m = *m;
                m.completion_p50_s = scale(m.completion_p50_s);
                m.completion_p90_s = scale(m.completion_p90_s);
                (seed, m)
            })
            .collect()
    }

    fn paired() -> Method {
        Method::Paired(Bootstrap {
            resamples: 500,
            ..Bootstrap::default()
        })
    }

    fn check<'a>(checks: &'a [Check], metric: &str) -> &'a Check {
        checks.iter().find(|c| c.metric == metric).unwrap()
    }

    #[test]
    fn the_same_metrics_pass() {
        let base = baseline();
        let checks = base.compare(&base.runs, Method::Mean).unwrap();
        assert!(checks.iter().all(|c| !c.regressed && !c.improved));
        let p50 = check(&checks, "completion p50 (s)");
        assert_eq!(p50.baseline, Some(15.0));
        assert!((p50.limit.unwrap() - 16.5).abs() < 1e-9);
        assert_eq!(p50.change, None);
    }

    #[test]
    fn only_the_seeds_compared_count() {
        let current = [(2, metrics(21.0, 0.8))].into();
        let checks = baseline().compare(&current, Method::Mean).unwrap();
        let p50 = check(&checks, "completion p50 (s)");
        assert_eq!(p50.baseline, Some(20.0));
        assert!(!p50.regressed, "5% slower is within tolerance");
        let missing = [(3, metrics(20.0, 0.8))].into();
        assert!(baseline().compare(&missing, Method::Mean).is_err());
    }

    #[test]
    fn regressions_and_improvements_are_flagged() {
        let current = [(1, metrics(12.0, 0.8)), (2, metrics(22.0, 0.7))].into();
        let checks = baseline().compare(&current, Method::Mean).unwrap();
        assert!(check(&checks, "completion p50 (s)").regressed, "17 > 16.5");
        assert!(check(&checks, "fairness").regressed, "0.75 < 0.80");
        assert!(!check(&checks, "overhead (bytes)").regressed);

        let current = [(1, metrics(5.0, 0.95)), (2, metrics(10.0, 0.9))].into();
        let checks = baseline().compare(&current, Method::Mean).unwrap();
        assert!(check(&checks, "completion p50 (s)").improved);
        assert!(check(&checks, "fairness").improved);
        assert!(checks.iter().all(|c| !c.regressed));
    }

    #[test]
    fn a_vanished_metric_regresses() {
        let mut none = metrics(10.0, 0.9);
        none.completion_p50_s = None;
        let current = [(1, none)].into();
        for method in [Method::Mean, paired()] {
            let checks = baseline().compare(&current, method).unwrap();
            assert!(check(&checks, "completion p50 (s)").regressed);
        }
    }

    #[test]
    fn a_seed_missing_a_metric_doesnt_hide_the_others() {
        let values = [(None, Some(10.0)), (Some(10.0), Some(20.0))];
        let result = super::check(
            "metric",
            &values,
            Worse::Higher,
            Tolerance::Relative(0.1),
            Method::Mean,
        );
        assert!(result.regressed, "{result:?}");
    }

    #[test]
    fn paired_checks_flag_a_consistent_slowdown() {
        let base = many(20);
        let checks = base
            .compare(
                &slowed(&base, |seed| 1.2 + (seed % 3) as f64 * 0.01),
                paired(),
            )
            .unwrap();
        let p50 = check(&checks, "completion p50 (s)");
        assert!(p50.regressed, "{p50:?}");
        let change = p50.change.unwrap();
        assert!(change.interval.low > 0.0, "{change:?}");
        assert!(!check(&checks, "fairness").regressed);
        assert!(table(&checks).contains("REGRESSED"));
    }

    #[test]
    fn paired_checks_ignore_one_unusual_seed() {
        let base = many(20);
        let current = slowed(&base, |seed| if seed == 7 { 6.0 } else { 1.0 });
        let paired = base.compare(&current, paired()).unwrap();
        assert!(paired.iter().all(|c| !c.regressed), "{}", table(&paired));
        // The mean moves with it.
        let mean = base.compare(&current, Method::Mean).unwrap();
        assert!(check(&mean, "completion p50 (s)").regressed);
    }

    #[test]
    fn paired_checks_flag_improvements() {
        let base = many(20);
        let checks = base.compare(&slowed(&base, |_| 0.7), paired()).unwrap();
        assert!(check(&checks, "completion p90 (s)").improved);
        assert!(checks.iter().all(|c| !c.regressed));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]

        #[test]
        fn unchanged_runs_pass_and_slower_ones_fail(
            times in prop::collection::vec(1.0..100.0f64, 5..25),
            slowdown in 0.15..1.0f64,
        ) {
            let base = Baseline {
                scenario: "test".into(),
                runs: (1..).zip(times.iter().map(|&t| metrics(t, 0.9))).collect(),
            };
            for method in [Method::Mean, paired()] {
                let checks = base.compare(&base.runs, method).unwrap();
                prop_assert!(checks.iter().all(|c| !c.regressed && !c.improved));
                let checks = base.compare(&slowed(&base, |_| 1.0 + slowdown), method).unwrap();
                prop_assert!(check(&checks, "completion p50 (s)").regressed);
                prop_assert!(check(&checks, "completion p90 (s)").regressed);
            }
        }
    }
}
