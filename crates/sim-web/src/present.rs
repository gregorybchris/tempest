//! How results read: metric values, intervals and differences in words, variants' short
//! labels, and the data each chart draws. Plain functions, so they are tested without a
//! browser.

use tempest_sim_api::data::{
    BaselineCheck, CheckInfo, ComparedVariant, Comparison, Difference, Estimate, MetricUnit,
    PeerInfo, RunSeries, SeriesBands, Verdict,
};
use tempest_ui_kit::chart::{Bar, Color, LineData, Series, Unit, XAxis};
use tempest_ui_kit::components::Tone;
use tempest_ui_kit::format;

/// The chart unit for a metric's unit.
pub fn chart_unit(unit: MetricUnit) -> Unit {
    match unit {
        MetricUnit::Fraction => Unit::Fraction,
        MetricUnit::Seconds => Unit::Seconds,
        MetricUnit::Number => Unit::Number,
    }
}

/// `value` in `unit`: `93.0%`, `41.2s`, `3m 05s` or `12.4`.
pub fn value(unit: MetricUnit, value: f64) -> String {
    match unit {
        MetricUnit::Fraction => format::percent(value),
        MetricUnit::Seconds if value.abs() < 60.0 => format!("{value:.1}s"),
        MetricUnit::Seconds => format::duration(value.round() as u64),
        MetricUnit::Number => format::compact(value),
    }
}

/// An optional value, or a dash.
pub fn maybe(unit: MetricUnit, v: Option<f64>) -> String {
    v.map_or_else(|| "–".to_owned(), |v| value(unit, v))
}

/// An interval's bounds: `38.1s – 44.0s`.
pub fn interval(unit: MetricUnit, e: &Estimate) -> String {
    format!("{} – {}", value(unit, e.low), value(unit, e.high))
}

/// A signed change in `unit`: `+3.2s`, `−4.1 pts` for fractions, `+1.5`.
pub fn signed(unit: MetricUnit, v: f64) -> String {
    let sign = if v < 0.0 { "−" } else { "+" };
    let size = v.abs();
    match unit {
        MetricUnit::Fraction => format!("{sign}{:.1} pts", size * 100.0),
        MetricUnit::Seconds => format!("{sign}{size:.1}s"),
        MetricUnit::Number => format!("{sign}{}", format::compact(size)),
    }
}

/// A variant's difference from the reference in words, and its accent: better is the
/// theme's main accent, worse is red, and anything not meaningful is quiet.
pub fn difference(unit: MetricUnit, lower_is_better: bool, d: &Difference) -> (String, Tone) {
    let median = d.estimate.median;
    let amount = match d.relative {
        Some(r) => format!("{:.1}%", r.abs() * 100.0),
        None => signed(unit, median)
            .trim_start_matches(['+', '−'])
            .to_owned(),
    };
    let better = (median < 0.0) == lower_is_better;
    let direction = match (unit, better) {
        (MetricUnit::Seconds, true) => "faster",
        (MetricUnit::Seconds, false) => "slower",
        (_, true) => "better",
        (_, false) => "worse",
    };
    match d.verdict {
        Verdict::TooFewPairs => ("too few paired seeds".to_owned(), Tone::Dim),
        _ if median == 0.0 => ("no difference".to_owned(), Tone::Dim),
        Verdict::NotMeaningful => (format!("{amount} {direction}, not meaningful"), Tone::Dim),
        Verdict::Meaningful if better => (format!("{amount} {direction}"), Tone::Down),
        Verdict::Meaningful => (format!("{amount} {direction}"), Tone::Bad),
    }
}

/// Short labels for compared variants: the settings that differ between them, and the
/// experiment when they come from more than one.
pub fn labels(variants: &[ComparedVariant]) -> Vec<String> {
    let settings: Vec<Vec<&str>> = variants
        .iter()
        .map(|v| v.name.split(' ').collect())
        .collect();
    let shared = |part: &str| variants.len() > 1 && settings.iter().all(|s| s.contains(&part));
    let experiments = variants
        .iter()
        .any(|v| v.experiment != variants[0].experiment);
    variants
        .iter()
        .zip(&settings)
        .map(|(v, parts)| {
            let differing: Vec<&str> = parts
                .iter()
                .filter(|p| !shared(p))
                .map(|p| p.split_once('=').map_or(*p, |(_, value)| value))
                .collect();
            let label = match differing.is_empty() {
                true => v.name.clone(),
                false => differing.join(" · "),
            };
            match experiments {
                true => format!("{}#{} {label}", v.experiment_name, v.experiment),
                false => label,
            }
        })
        .collect()
}

/// A variant's name with only the values of its settings: `limited · 16` for
/// `strategies.endgame=limited limits.requests=16`.
pub fn variant_values(name: &str) -> String {
    name.split(' ')
        .map(|part| part.split_once('=').map_or(part, |(_, value)| value))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// One bar per compared variant for metric `metric`: its median with its interval.
pub fn metric_bars(comparison: &Comparison, metric: usize, labels: &[String]) -> Vec<Bar> {
    let Some(row) = comparison.metrics.get(metric) else {
        return Vec::new();
    };
    row.cells
        .iter()
        .enumerate()
        .filter_map(|(i, cell)| {
            let e = cell.estimate?;
            let seeds = format!("median of {} seeds", e.n);
            let note = match (i == comparison.reference, cell.difference) {
                (true, _) => format!("{seeds} · the reference"),
                (false, Some(d)) => format!(
                    "{seeds} · {}",
                    difference(row.unit, row.lower_is_better, &d).0
                ),
                (false, None) => seeds,
            };
            Some(Bar {
                label: labels.get(i).cloned().unwrap_or_default(),
                value: e.median,
                interval: Some((e.low, e.high)),
                note: Some(note),
                color: Color::Series(i),
            })
        })
        .collect()
}

/// A band chart: one median curve per variant with its interval as a band.
pub fn band_chart(bands: &SeriesBands, labels: &[String]) -> LineData {
    let series = bands
        .variants
        .iter()
        .enumerate()
        .map(|(i, points)| {
            Series::new(
                format!("v{i}"),
                labels.get(i).cloned().unwrap_or_default(),
                Color::Series(i),
                points.iter().map(|p| (p.t, p.median)).collect(),
            )
            .with_band(points.iter().map(|p| (p.low, p.high)).collect())
        })
        .collect();
    LineData {
        series,
        x: XAxis::Value(Unit::Seconds),
        y: chart_unit(bands.unit),
        y_floor: 1.0,
    }
}

/// A run's swarm over time: peers present, and copies of the rarest chunk and of a chunk on
/// average.
pub fn swarm_chart(series: &RunSeries) -> LineData {
    let points = |f: fn(&tempest_sim_api::data::AvailabilitySample) -> f64| {
        series
            .availability
            .iter()
            .map(|s| (s.time_s, f(s)))
            .collect()
    };
    LineData {
        series: vec![
            Series::new(
                "present",
                "peers present",
                Color::Series(2),
                points(|s| f64::from(s.peers)),
            )
            .dashed(),
            Series::new(
                "mean",
                "copies of a chunk, on average",
                Color::Series(1),
                points(|s| s.mean),
            ),
            Series::new(
                "rarest",
                "copies of the rarest chunk",
                Color::Down,
                points(|s| f64::from(s.rarest)),
            )
            .filled(),
        ],
        x: XAxis::Value(Unit::Seconds),
        y: Unit::Number,
        y_floor: 1.0,
    }
}

/// Points of a step function that rises to each `(x, y)` at `x`, starting from `(0, 0)`.
pub fn steps(points: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out = vec![(0.0, 0.0)];
    let mut last = 0.0;
    for &(x, y) in points {
        out.push((x, last));
        out.push((x, y));
        last = y;
    }
    out
}

/// The fraction of a run's downloaders that had completed, over time.
pub fn completions_chart(series: &RunSeries) -> LineData {
    let downloaders = f64::from(series.downloaders.max(1));
    let points: Vec<(f64, f64)> = series
        .completed
        .iter()
        .map(|&(t, n)| (t, f64::from(n) / downloaders))
        .collect();
    let mut stepped = steps(&points);
    if let Some(&(_, last)) = stepped.last() {
        // Hold the last level a little longer, so the final step shows.
        let end = points.last().map_or(0.0, |p| p.0) * 1.05;
        stepped.push((end, last));
    }
    LineData {
        series: vec![
            Series::new("completed", "downloaders completed", Color::Down, stepped).filled(),
        ],
        x: XAxis::Value(Unit::Seconds),
        y: Unit::Fraction,
        y_floor: 1.0,
    }
}

/// The Data a run sent per second.
pub fn throughput_chart(series: &RunSeries) -> LineData {
    LineData {
        series: vec![
            Series::new("data", "Data sent", Color::Up, series.throughput.clone()).filled(),
        ],
        x: XAxis::Value(Unit::Seconds),
        y: Unit::Rate,
        y_floor: 1e3,
    }
}

/// How long each downloader took from arrival to completion, as the fraction done by each
/// time; downloaders that never completed keep the curve below 1.
pub fn completion_cdf(peers: &[PeerInfo]) -> LineData {
    let downloaders: Vec<&PeerInfo> = peers.iter().filter(|p| p.starts == "empty").collect();
    let mut times: Vec<f64> = downloaders
        .iter()
        .filter_map(|p| Some(p.completed_s? - p.arrived_s?))
        .collect();
    times.sort_by(f64::total_cmp);
    let total = downloaders.len().max(1) as f64;
    let points: Vec<(f64, f64)> = times
        .iter()
        .enumerate()
        .map(|(i, &t)| (t, (i + 1) as f64 / total))
        .collect();
    LineData {
        series: vec![Series::new(
            "cdf",
            "completed within",
            Color::Series(4),
            steps(&points),
        )],
        x: XAxis::Value(Unit::Seconds),
        y: Unit::Fraction,
        y_floor: 1.0,
    }
}

/// What a baseline check found, and its accent.
pub fn check_status(check: &CheckInfo) -> (&'static str, Tone) {
    match (check.regressed, check.improved) {
        (true, _) => ("regressed", Tone::Bad),
        (_, true) => ("improved", Tone::Hot),
        _ if check.baseline.is_none() && check.current.is_none() => ("no values", Tone::Dim),
        _ => ("within tolerance", Tone::Dim),
    }
}

/// Each checked metric's change from its baseline, relative to the baseline, with the
/// interval of a paired check. Metrics whose baseline is 0 or missing are left out.
pub fn baseline_bars(variant: &BaselineCheck) -> Vec<Bar> {
    variant
        .checks
        .iter()
        .filter_map(|c| {
            let base = c.baseline.filter(|b| *b != 0.0)?;
            let current = c.current?;
            let relative = |v: f64| v / base.abs();
            let (status, tone) = check_status(c);
            let color = match tone {
                Tone::Bad => Color::Bad,
                Tone::Hot => Color::Hot,
                _ => Color::Fg,
            };
            let mut note = format!("{} → {}", format::compact(base), format::compact(current));
            if let Some(change) = c.change {
                note.push_str(&format!(
                    " · 95% CI {:+.1}% to {:+.1}%",
                    relative(change.low) * 100.0,
                    relative(change.high) * 100.0
                ));
            }
            note.push_str(&format!(" · {status}"));
            Some(Bar {
                label: c
                    .metric
                    .trim_end_matches(" (s)")
                    .trim_end_matches(" (bytes)")
                    .to_owned(),
                value: match c.change {
                    Some(change) => relative(change.median),
                    None => relative(current - base),
                },
                interval: c.change.map(|e| (relative(e.low), relative(e.high))),
                note: Some(note),
                color,
            })
        })
        .collect()
}

/// A count of something, singular or plural: `1 seed`, `6 seeds`.
pub fn plural(n: u64, word: &str) -> String {
    format!(
        "{} {word}{}",
        format::count(n),
        if n == 1 { "" } else { "s" }
    )
}

/// A commit or digest, shortened.
pub fn short(hash: &str) -> &str {
    format::short_id(hash)
}

#[cfg(test)]
mod tests {
    use tempest_sim_api::data::{BandPoint, Cell, MetricRow};

    use super::*;

    fn estimate(median: f64, low: f64, high: f64) -> Estimate {
        Estimate {
            n: 6,
            median,
            low,
            high,
        }
    }

    fn diff(median: f64, relative: Option<f64>, verdict: Verdict) -> Difference {
        Difference {
            estimate: estimate(median, median - 1.0, median + 1.0),
            relative,
            verdict,
        }
    }

    #[test]
    fn values_read_in_their_units() {
        assert_eq!(value(MetricUnit::Fraction, 0.934), "93.4%");
        assert_eq!(value(MetricUnit::Seconds, 41.23), "41.2s");
        assert_eq!(value(MetricUnit::Seconds, 185.0), "3m 05s");
        assert_eq!(value(MetricUnit::Number, 12.0), "12");
        assert_eq!(maybe(MetricUnit::Number, None), "–");
        assert_eq!(
            interval(MetricUnit::Seconds, &estimate(40.0, 38.12, 44.0)),
            "38.1s – 44.0s"
        );
        assert_eq!(signed(MetricUnit::Fraction, -0.041), "−4.1 pts");
        assert_eq!(signed(MetricUnit::Seconds, 3.21), "+3.2s");
    }

    #[test]
    fn differences_say_which_way_is_better() {
        let faster = diff(-4.0, Some(-0.12), Verdict::Meaningful);
        assert_eq!(
            difference(MetricUnit::Seconds, true, &faster),
            ("12.0% faster".to_owned(), Tone::Down)
        );
        let slower = diff(4.0, Some(0.12), Verdict::Meaningful);
        assert_eq!(
            difference(MetricUnit::Seconds, true, &slower),
            ("12.0% slower".to_owned(), Tone::Bad)
        );
        let fairer = diff(0.05, Some(0.06), Verdict::NotMeaningful);
        assert_eq!(
            difference(MetricUnit::Fraction, false, &fairer),
            ("6.0% better, not meaningful".to_owned(), Tone::Dim)
        );
        let absolute = diff(-0.05, None, Verdict::Meaningful);
        assert_eq!(
            difference(MetricUnit::Fraction, false, &absolute),
            ("5.0 pts worse".to_owned(), Tone::Bad)
        );
        let few = diff(1.0, Some(0.1), Verdict::TooFewPairs);
        assert_eq!(difference(MetricUnit::Number, false, &few).1, Tone::Dim);
    }

    fn variant(experiment: i64, name: &str) -> ComparedVariant {
        ComparedVariant {
            experiment,
            experiment_name: format!("exp{experiment}"),
            variant: 0,
            name: name.to_owned(),
            seeds: vec![1, 2],
        }
    }

    #[test]
    fn labels_keep_only_what_differs() {
        let same = [
            variant(1, "strategies.upload=bittorrent limits.requests=8"),
            variant(1, "strategies.upload=propshare limits.requests=8"),
        ];
        assert_eq!(labels(&same), ["bittorrent", "propshare"]);
        let across = [variant(1, "default"), variant(2, "default")];
        assert_eq!(labels(&across), ["exp1#1 default", "exp2#2 default"]);
        assert_eq!(labels(&[variant(1, "default")]), ["default"]);
        assert_eq!(variant_values("a.b=x c=16"), "x · 16");
        assert_eq!(variant_values("default"), "default");
    }

    #[test]
    fn bars_carry_medians_intervals_and_verdicts() {
        let comparison = Comparison {
            variants: vec![variant(1, "a=x"), variant(1, "a=y")],
            reference: 0,
            confidence: 0.95,
            min_pairs: 5,
            min_effect: 0.02,
            metrics: vec![MetricRow {
                name: "completion p50".to_owned(),
                unit: MetricUnit::Seconds,
                lower_is_better: true,
                cells: vec![
                    Cell {
                        estimate: Some(estimate(40.0, 38.0, 42.0)),
                        difference: None,
                    },
                    Cell {
                        estimate: Some(estimate(30.0, 29.0, 33.0)),
                        difference: Some(diff(-10.0, Some(-0.25), Verdict::Meaningful)),
                    },
                ],
            }],
        };
        let bars = metric_bars(&comparison, 0, &labels(&comparison.variants));
        assert_eq!(bars.len(), 2);
        assert_eq!(
            bars[0].note.as_deref(),
            Some("median of 6 seeds · the reference")
        );
        assert_eq!(bars[1].interval, Some((29.0, 33.0)));
        assert!(bars[1].note.as_deref().unwrap().ends_with("25.0% faster"));
        assert!(metric_bars(&comparison, 3, &[]).is_empty());
    }

    #[test]
    fn bands_and_steps_follow_their_points() {
        let bands = SeriesBands {
            key: "completed".to_owned(),
            title: "Downloaders completed".to_owned(),
            unit: MetricUnit::Fraction,
            variants: vec![vec![
                BandPoint {
                    t: 0.0,
                    median: 0.0,
                    low: 0.0,
                    high: 0.0,
                },
                BandPoint {
                    t: 10.0,
                    median: 0.5,
                    low: 0.4,
                    high: 0.7,
                },
            ]],
        };
        let chart = band_chart(&bands, &["a".to_owned()]);
        assert_eq!(chart.series[0].points, [(0.0, 0.0), (10.0, 0.5)]);
        assert_eq!(chart.series[0].band, [(0.0, 0.0), (0.4, 0.7)]);
        assert_eq!(chart.y, Unit::Fraction);
        assert_eq!(
            steps(&[(1.0, 1.0), (3.0, 2.0)]),
            [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (3.0, 1.0), (3.0, 2.0)]
        );
    }

    fn peer(starts: &str, arrived: f64, completed: Option<f64>) -> PeerInfo {
        PeerInfo {
            peer: 0,
            class: "home".to_owned(),
            starts: starts.to_owned(),
            behavior: "normal".to_owned(),
            upload_capacity: None,
            download_capacity: None,
            arrived_s: Some(arrived),
            first_peer_s: None,
            first_byte_s: None,
            completed_s: completed,
            left_s: None,
            downloaded: 0,
            uploaded: 0,
            duplicate: 0,
            bad: 0,
        }
    }

    #[test]
    fn the_completion_curve_counts_downloaders_that_never_finished() {
        let peers = [
            peer("complete", 0.0, None),
            peer("empty", 10.0, Some(30.0)),
            peer("empty", 5.0, Some(15.0)),
            peer("empty", 0.0, None),
            peer("empty", 1.0, None),
        ];
        let chart = completion_cdf(&peers);
        let points = &chart.series[0].points;
        assert_eq!(points.last(), Some(&(20.0, 0.5)));
        assert_eq!(points[2], (10.0, 0.25));
    }

    #[test]
    fn baseline_bars_show_relative_changes() {
        let check = |metric: &str, baseline, current, regressed| CheckInfo {
            metric: metric.to_owned(),
            baseline,
            current,
            limit: None,
            change: None,
            regressed,
            improved: false,
        };
        let variant = BaselineCheck {
            variant: 0,
            variant_name: "default".to_owned(),
            method: tempest_sim_api::data::Method::Mean,
            seeds: vec![1, 2],
            missing: Vec::new(),
            checks: vec![
                check("completion p50 (s)", Some(40.0), Some(50.0), true),
                check("duplicate (bytes)", Some(0.0), Some(0.0), false),
                check("fairness", None, None, false),
            ],
        };
        let bars = baseline_bars(&variant);
        assert_eq!(bars.len(), 1);
        assert_eq!(bars[0].label, "completion p50");
        assert_eq!(bars[0].value, 0.25);
        assert_eq!(bars[0].color, Color::Bad);
        assert_eq!(check_status(&variant.checks[2]).0, "no values");
    }

    #[test]
    fn counts_agree_with_their_nouns() {
        assert_eq!(plural(1, "seed"), "1 seed");
        assert_eq!(plural(1200, "run"), "1,200 runs");
    }
}
