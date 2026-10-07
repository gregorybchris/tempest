//! Statistics across seeds (D-073, D-167): medians with bootstrap confidence intervals, and
//! paired comparisons of variants with a verdict on whether they differ meaningfully.
//!
//! Each run is one sample. A variant's runs give one value per seed for each [`Metric`]. Two
//! variants are compared seed by seed, since runs with the same seed share their world: the same
//! arrivals, sessions and links. The comparison reads only the runs' headline metrics, so the
//! results database's `runs` table holds everything needed to reproduce it.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use rand::{Rng as _, SeedableRng};
use rand_chacha::ChaCha8Rng;

use crate::engine::RunResult;
use crate::scenario::Scenario;
use crate::summary::RunMetrics;

/// How confidence intervals are estimated: the percentile bootstrap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bootstrap {
    /// How many resamples to draw.
    pub resamples: u32,
    /// The confidence level of intervals, such as 0.95.
    pub confidence: f64,
    /// The seed of the resampler. Every interval starts from it afresh, so results don't
    /// depend on what else was estimated.
    pub seed: u64,
}

impl Default for Bootstrap {
    fn default() -> Self {
        Self {
            resamples: 10_000,
            confidence: 0.95,
            seed: 0,
        }
    }
}

/// A confidence interval.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Interval {
    /// The lower bound.
    pub low: f64,
    /// The upper bound.
    pub high: f64,
}

impl Interval {
    /// Whether `value` lies within the interval, bounds included.
    pub fn contains(&self, value: f64) -> bool {
        self.low <= value && value <= self.high
    }
}

/// The median of a sample, with its confidence interval.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimate {
    /// The number of values.
    pub n: usize,
    /// Their median.
    pub median: f64,
    /// The confidence interval of the median.
    pub interval: Interval,
}

/// The median of `values`: the middle one, or the mean of the middle two. `None` if empty.
pub fn median(values: &[f64]) -> Option<f64> {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    median_of_sorted(&sorted)
}

fn median_of_sorted(sorted: &[f64]) -> Option<f64> {
    let n = sorted.len();
    match n {
        0 => None,
        _ if n % 2 == 1 => Some(sorted[n / 2]),
        _ => Some((sorted[n / 2 - 1] + sorted[n / 2]) / 2.0),
    }
}

impl Bootstrap {
    /// The median of `values` with its percentile-bootstrap interval. `None` if empty.
    pub fn median(&self, values: &[f64]) -> Option<Estimate> {
        let median = median(values)?;
        let mut rng = ChaCha8Rng::seed_from_u64(self.seed);
        let mut resample = vec![0.0; values.len()];
        let mut medians: Vec<f64> = (0..self.resamples.max(1))
            .map(|_| {
                for slot in &mut resample {
                    *slot = values[rng.random_range(0..values.len())];
                }
                resample.sort_by(f64::total_cmp);
                median_of_sorted(&resample).expect("not empty")
            })
            .collect();
        medians.sort_by(f64::total_cmp);
        let tail = (1.0 - self.confidence) / 2.0;
        let count = medians.len();
        let low = ((tail * count as f64).floor() as usize).min(count - 1);
        let high = (((1.0 - tail) * count as f64).ceil() as usize).clamp(1, count) - 1;
        Some(Estimate {
            n: values.len(),
            median,
            interval: Interval {
                low: medians[low],
                high: medians[high],
            },
        })
    }

    /// Compares `other` with `reference`, pairing their values by seed. `None` if no seed has
    /// both.
    pub fn compare(
        &self,
        reference: &BTreeMap<u64, f64>,
        other: &BTreeMap<u64, f64>,
        rule: Meaningful,
    ) -> Option<Difference> {
        let differences: Vec<f64> = reference
            .iter()
            .filter_map(|(seed, r)| Some(other.get(seed)? - r))
            .collect();
        let estimate = self.median(&differences)?;
        let paired: Vec<f64> = reference
            .iter()
            .filter(|(seed, _)| other.contains_key(seed))
            .map(|(_, &r)| r)
            .collect();
        let base = median(&paired).expect("paired values exist");
        let relative = (base != 0.0).then(|| estimate.median / base.abs());
        let verdict = if estimate.n < rule.min_pairs {
            Verdict::TooFewPairs
        } else if estimate.interval.contains(0.0)
            || relative.is_some_and(|r| r.abs() < rule.min_effect)
        {
            Verdict::NotMeaningful
        } else {
            Verdict::Meaningful
        };
        Some(Difference {
            estimate,
            relative,
            verdict,
        })
    }
}

/// When a difference is meaningful.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Meaningful {
    /// The fewest paired seeds for a verdict.
    pub min_pairs: usize,
    /// The smallest effect worth reporting, relative to the reference's median.
    pub min_effect: f64,
}

impl Default for Meaningful {
    fn default() -> Self {
        Self {
            min_pairs: 5,
            min_effect: 0.02,
        }
    }
}

/// A paired comparison: the median of the per-seed differences (other − reference).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Difference {
    /// The median difference and its interval, over the seeds both have.
    pub estimate: Estimate,
    /// The median difference over the reference's median on those seeds. `None` if that is 0.
    pub relative: Option<f64>,
    /// Whether the difference is meaningful.
    pub verdict: Verdict,
}

/// Whether a difference is meaningful.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The interval excludes zero, and the effect is at least the minimum.
    Meaningful,
    /// The interval includes zero, or the effect is too small to matter.
    NotMeaningful,
    /// Too few paired seeds to say.
    TooFewPairs,
}

/// A headline metric compared across variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Metric {
    /// The fraction of downloaders that completed.
    Completed,
    /// The median completion time.
    CompletionP50,
    /// The 90th percentile completion time.
    CompletionP90,
    /// The median time to first byte.
    FirstByteP50,
    /// The upload utilization.
    Utilization,
    /// Jain's fairness index.
    Fairness,
    /// The mean availability of the rarest chunk.
    RarestCopies,
}

impl Metric {
    /// Every metric, in table order.
    pub const ALL: [Self; 7] = [
        Self::Completed,
        Self::CompletionP50,
        Self::CompletionP90,
        Self::FirstByteP50,
        Self::Utilization,
        Self::Fairness,
        Self::RarestCopies,
    ];

    /// A short name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::CompletionP50 => "completion p50",
            Self::CompletionP90 => "completion p90",
            Self::FirstByteP50 => "first byte p50",
            Self::Utilization => "utilization",
            Self::Fairness => "fairness",
            Self::RarestCopies => "rarest copies",
        }
    }

    /// The run's value, if it has one. Times are in seconds.
    pub fn value(self, metrics: &RunMetrics) -> Option<f64> {
        match self {
            Self::Completed => (metrics.downloaders > 0)
                .then(|| metrics.completed as f64 / metrics.downloaders as f64),
            Self::CompletionP50 => metrics.completion_p50.map(|d| d.as_secs_f64()),
            Self::CompletionP90 => metrics.completion_p90.map(|d| d.as_secs_f64()),
            Self::FirstByteP50 => metrics.first_byte_p50.map(|d| d.as_secs_f64()),
            Self::Utilization => metrics.upload_utilization,
            Self::Fairness => metrics.fairness,
            Self::RarestCopies => metrics.rarest_copies,
        }
    }

    /// Whether smaller values are better.
    pub fn lower_is_better(self) -> bool {
        matches!(
            self,
            Self::CompletionP50 | Self::CompletionP90 | Self::FirstByteP50
        )
    }

    fn format(self, value: f64) -> String {
        if self.lower_is_better() {
            format!("{value:.1}s")
        } else {
            format!("{value:.3}")
        }
    }

    fn format_signed(self, value: f64) -> String {
        if self.lower_is_better() {
            format!("{value:+.1}s")
        } else {
            format!("{value:+.3}")
        }
    }
}

/// One variant's values of every metric, by seed.
#[derive(Debug, Clone, PartialEq)]
pub struct Samples {
    /// The variant's name.
    pub name: String,
    /// Each metric's value in each run that has one, by seed.
    pub values: BTreeMap<Metric, BTreeMap<u64, f64>>,
}

/// Each variant's samples, in variant order. Variants without results are left out.
pub fn samples(scenario: &Scenario, results: &[RunResult]) -> Vec<Samples> {
    scenario
        .variants()
        .iter()
        .filter_map(|variant| {
            let runs: Vec<&RunResult> = results
                .iter()
                .filter(|r| r.variant == variant.index)
                .collect();
            if runs.is_empty() {
                return None;
            }
            let metrics: Vec<(u64, RunMetrics)> =
                runs.iter().map(|r| (r.seed, r.metrics(scenario))).collect();
            let values = Metric::ALL
                .iter()
                .map(|&metric| {
                    let by_seed = metrics
                        .iter()
                        .filter_map(|(seed, m)| Some((*seed, metric.value(m)?)))
                        .collect();
                    (metric, by_seed)
                })
                .collect();
            Some(Samples {
                name: variant.name(),
                values,
            })
        })
        .collect()
}

/// One row of a comparison: one metric of one variant.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The metric.
    pub metric: Metric,
    /// The variant's name.
    pub variant: String,
    /// Whether this is the reference variant.
    pub reference: bool,
    /// The median across seeds, if any run has a value.
    pub estimate: Option<Estimate>,
    /// The paired difference from the reference, for other variants.
    pub difference: Option<Difference>,
}

/// Compares every variant in `samples` with the one at position `reference`, metric by metric.
pub fn compare(
    samples: &[Samples],
    reference: usize,
    bootstrap: Bootstrap,
    rule: Meaningful,
) -> Vec<Row> {
    let empty = BTreeMap::new();
    let mut rows = Vec::new();
    for metric in Metric::ALL {
        let base = samples[reference].values.get(&metric).unwrap_or(&empty);
        for (i, sample) in samples.iter().enumerate() {
            let values = sample.values.get(&metric).unwrap_or(&empty);
            let list: Vec<f64> = values.values().copied().collect();
            rows.push(Row {
                metric,
                variant: sample.name.clone(),
                reference: i == reference,
                estimate: bootstrap.median(&list),
                difference: (i != reference)
                    .then(|| bootstrap.compare(base, values, rule))
                    .flatten(),
            });
        }
    }
    rows
}

/// A table of comparison rows, with intervals at `confidence`.
pub fn table(rows: &[Row], confidence: f64) -> String {
    let ci = format!("{:.0}% CI", confidence * 100.0);
    let header = [
        "metric",
        "variant",
        "runs",
        "median",
        ci.as_str(),
        "vs reference",
        ci.as_str(),
        "verdict",
    ];
    let cells: Vec<[String; 8]> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let m = row.metric;
            let first = i == 0 || rows[i - 1].metric != m;
            let interval = |e: &Estimate, f: &dyn Fn(f64) -> String| {
                format!("{} – {}", f(e.interval.low), f(e.interval.high))
            };
            let (median, range) = row.estimate.as_ref().map_or_else(
                || ("-".to_owned(), "-".to_owned()),
                |e| (m.format(e.median), interval(e, &|v| m.format(v))),
            );
            let (change, change_range, verdict) = if row.reference {
                (String::new(), String::new(), "reference".to_owned())
            } else if let Some(d) = &row.difference {
                let relative = d
                    .relative
                    .map_or_else(String::new, |r| format!(" ({:+.1}%)", r * 100.0));
                (
                    format!("{}{relative}", m.format_signed(d.estimate.median)),
                    interval(&d.estimate, &|v| m.format_signed(v)),
                    verdict(m, d).to_owned(),
                )
            } else {
                ("-".to_owned(), "-".to_owned(), "-".to_owned())
            };
            [
                if first {
                    m.name().to_owned()
                } else {
                    String::new()
                },
                row.variant.clone(),
                row.estimate.map_or(0, |e| e.n).to_string(),
                median,
                range,
                change,
                change_range,
                verdict,
            ]
        })
        .collect();
    let widths: Vec<usize> = (0..header.len())
        .map(|i| {
            cells
                .iter()
                .map(|r| r[i].chars().count())
                .chain([header[i].chars().count()])
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut out = String::new();
    let mut line = |cells: &[&str]| {
        let mut text = String::new();
        for (i, cell) in cells.iter().enumerate() {
            let pad = widths[i] - cell.chars().count();
            if i < 2 || i == cells.len() - 1 {
                let _ = write!(text, "{cell}{}  ", " ".repeat(pad));
            } else {
                let _ = write!(text, "{}{cell}  ", " ".repeat(pad));
            }
        }
        out.push_str(text.trim_end());
        out.push('\n');
    };
    line(&header);
    for row in &cells {
        line(&row.iter().map(String::as_str).collect::<Vec<_>>());
    }
    out
}

fn verdict(metric: Metric, difference: &Difference) -> &'static str {
    match difference.verdict {
        Verdict::TooFewPairs => "too few seeds",
        Verdict::NotMeaningful => "not meaningful",
        Verdict::Meaningful => {
            if (difference.estimate.median < 0.0) == metric.lower_is_better() {
                "better"
            } else {
                "worse"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn by_seed(values: &[f64]) -> BTreeMap<u64, f64> {
        (1..).zip(values.iter().copied()).collect()
    }

    #[test]
    fn medians() {
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[3.0]), Some(3.0));
        assert_eq!(median(&[4.0, 1.0, 3.0]), Some(3.0));
        assert_eq!(median(&[4.0, 1.0, 3.0, 2.0]), Some(2.5));
    }

    #[test]
    fn intervals_are_deterministic_and_narrow_with_more_samples() {
        let bootstrap = Bootstrap::default();
        let few: Vec<f64> = (0..10).map(|i| 100.0 + f64::from(i % 7) * 3.0).collect();
        let many: Vec<f64> = (0..200).map(|i| 100.0 + f64::from(i % 7) * 3.0).collect();
        let a = bootstrap.median(&few).unwrap();
        assert_eq!(a, bootstrap.median(&few).unwrap());
        let b = bootstrap.median(&many).unwrap();
        let width = |e: Estimate| e.interval.high - e.interval.low;
        assert!(width(b) <= width(a), "{a:?} {b:?}");
        assert!(
            a.interval.low < a.median && a.median < a.interval.high,
            "{a:?}"
        );
    }

    #[test]
    fn a_consistent_shift_is_meaningful_and_has_a_direction() {
        let bootstrap = Bootstrap::default();
        let reference: Vec<f64> = (0..20).map(|i| 100.0 + f64::from(i) * 5.0).collect();
        let faster: Vec<f64> = reference
            .iter()
            .enumerate()
            .map(|(i, r)| r * 0.9 + (i % 3) as f64)
            .collect();
        let d = bootstrap
            .compare(
                &by_seed(&reference),
                &by_seed(&faster),
                Meaningful::default(),
            )
            .unwrap();
        assert_eq!(d.verdict, Verdict::Meaningful);
        assert!(
            d.estimate.median < 0.0 && d.estimate.interval.high < 0.0,
            "{d:?}"
        );
        assert_eq!(verdict(Metric::CompletionP50, &d), "better");
        assert_eq!(verdict(Metric::Utilization, &d), "worse");
    }

    #[test]
    fn small_or_rare_effects_are_not_meaningful() {
        let bootstrap = Bootstrap::default();
        let rule = Meaningful::default();
        let reference: Vec<f64> = (0..20).map(|i| 100.0 + f64::from(i)).collect();
        // Consistent, but only 1%.
        let slightly: Vec<f64> = reference.iter().map(|r| r * 1.01).collect();
        let d = bootstrap
            .compare(&by_seed(&reference), &by_seed(&slightly), rule)
            .unwrap();
        assert!(d.estimate.interval.low > 0.0);
        assert_eq!(d.verdict, Verdict::NotMeaningful);
        // Large, but on only a few seeds.
        let once: Vec<f64> = reference
            .iter()
            .enumerate()
            .map(|(i, r)| if i < 3 { r * 2.0 } else { *r })
            .collect();
        let d = bootstrap
            .compare(&by_seed(&reference), &by_seed(&once), rule)
            .unwrap();
        assert_eq!(d.verdict, Verdict::NotMeaningful);
        // Too few seeds to say.
        let d = bootstrap
            .compare(&by_seed(&[1.0, 2.0]), &by_seed(&[5.0, 6.0]), rule)
            .unwrap();
        assert_eq!(d.verdict, Verdict::TooFewPairs);
    }

    #[test]
    fn only_seeds_both_variants_have_are_paired() {
        let reference = by_seed(&[10.0, 20.0, 30.0]);
        let other: BTreeMap<u64, f64> = [(2, 25.0), (3, 35.0), (4, 1000.0)].into();
        let d = Bootstrap::default()
            .compare(&reference, &other, Meaningful::default())
            .unwrap();
        assert_eq!(d.estimate.n, 2);
        assert_eq!(d.estimate.median, 5.0);
        assert_eq!(d.relative, Some(0.2));
        let none: BTreeMap<u64, f64> = [(9, 1.0)].into();
        assert_eq!(
            Bootstrap::default().compare(&reference, &none, Meaningful::default()),
            None
        );
    }

    #[test]
    fn the_table_marks_the_reference_and_verdicts() {
        let reference = Samples {
            name: "a".into(),
            values: [(Metric::CompletionP50, by_seed(&[100.0; 6]))].into(),
        };
        let other = Samples {
            name: "b".into(),
            values: [(Metric::CompletionP50, by_seed(&[80.0; 6]))].into(),
        };
        let rows = compare(
            &[reference, other],
            0,
            Bootstrap::default(),
            Meaningful::default(),
        );
        let text = table(&rows, 0.95);
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].contains("95% CI"), "{text}");
        // Metrics without values show dashes.
        assert!(
            lines[1].starts_with("completed") && lines[1].contains(" - "),
            "{text}"
        );
        assert!(lines[3].starts_with("completion p50") && lines[3].ends_with("reference"));
        assert!(
            lines[4].contains("-20.0s (-20.0%)") && lines[4].ends_with("better"),
            "{text}"
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn the_interval_contains_the_median(
            values in prop::collection::vec(0.0..1e4f64, 1..40),
            seed in any::<u64>(),
        ) {
            let bootstrap = Bootstrap { resamples: 2_000, seed, ..Bootstrap::default() };
            let e = bootstrap.median(&values).unwrap();
            prop_assert!(e.interval.contains(e.median), "{:?}", e);
            let min = values.iter().copied().fold(f64::INFINITY, f64::min);
            let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            prop_assert!(min <= e.interval.low && e.interval.high <= max);
        }

        #[test]
        fn identical_variants_are_never_meaningful(
            values in prop::collection::vec(0.0..1e4f64, 1..40),
        ) {
            let values = by_seed(&values);
            let d = Bootstrap::default()
                .compare(&values, &values, Meaningful::default())
                .unwrap();
            prop_assert_ne!(d.verdict, Verdict::Meaningful);
            prop_assert_eq!(d.estimate.median, 0.0);
        }

        #[test]
        fn large_shifts_are_detected(
            values in prop::collection::vec(100.0..200.0f64, 8..40),
            noise in prop::collection::vec(-1.0..1.0f64, 40),
            shift in 0.1..0.5f64,
        ) {
            let reference = by_seed(&values);
            let shifted: BTreeMap<u64, f64> = reference
                .iter()
                .zip(&noise)
                .map(|((&seed, &r), n)| (seed, r * (1.0 + shift) + n))
                .collect();
            let d = Bootstrap::default()
                .compare(&reference, &shifted, Meaningful::default())
                .unwrap();
            prop_assert_eq!(d.verdict, Verdict::Meaningful);
            prop_assert!(d.estimate.interval.low > 0.0);
        }
    }
}
