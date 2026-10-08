//! Scales, ticks and SVG path data, shared by every chart.

use std::fmt::Write;

use crate::format;

/// A chart's size and the margins around its plot area, in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    /// Total width.
    pub width: f64,
    /// Total height.
    pub height: f64,
    /// Room left of the plot, for the y axis's labels.
    pub left: f64,
    /// Room right of the plot.
    pub right: f64,
    /// Room above the plot.
    pub top: f64,
    /// Room below the plot, for the x axis's labels.
    pub bottom: f64,
}

impl Layout {
    /// A layout of `width` by `height` with room for axis labels.
    pub fn new(width: f64, height: f64) -> Self {
        Self {
            width: width.max(1.0),
            height: height.max(1.0),
            left: 58.0,
            right: 14.0,
            top: 10.0,
            bottom: 26.0,
        }
    }

    /// The plot's left edge.
    pub fn plot_left(&self) -> f64 {
        self.left
    }

    /// The plot's right edge, never left of its left edge.
    pub fn plot_right(&self) -> f64 {
        (self.width - self.right).max(self.left + 1.0)
    }

    /// The plot's top edge.
    pub fn plot_top(&self) -> f64 {
        self.top
    }

    /// The plot's bottom edge, never above its top edge.
    pub fn plot_bottom(&self) -> f64 {
        (self.height - self.bottom).max(self.top + 1.0)
    }
}

/// A linear map from a domain of values to a range of pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scale {
    /// The values at the range's ends.
    pub domain: (f64, f64),
    /// The pixels the domain's ends map to.
    pub range: (f64, f64),
}

impl Scale {
    /// The pixel for `value`. An empty domain maps everything to the range's start.
    pub fn map(&self, value: f64) -> f64 {
        let (d0, d1) = self.domain;
        let (r0, r1) = self.range;
        if d1 == d0 {
            return r0;
        }
        r0 + (value - d0) / (d1 - d0) * (r1 - r0)
    }

    /// The value at pixel `px`.
    pub fn invert(&self, px: f64) -> f64 {
        let (d0, d1) = self.domain;
        let (r0, r1) = self.range;
        if r1 == r0 {
            return d0;
        }
        d0 + (px - r0) / (r1 - r0) * (d1 - d0)
    }
}

/// An axis tick: where it goes and what it says.
#[derive(Clone, Debug, PartialEq)]
pub struct Tick {
    /// Its pixel position along the axis.
    pub pos: f64,
    /// Its label.
    pub label: String,
}

/// About `target` round tick values covering `min` to `max`: steps of 1, 2 or 5 times a power
/// of ten, the first at or below `min` and the last at or above `max`.
pub fn nice_ticks(min: f64, max: f64, target: usize) -> Vec<f64> {
    let (min, max) = if min.is_finite() && max.is_finite() && max > min {
        (min, max)
    } else if min.is_finite() {
        (min, min + 1.0)
    } else {
        (0.0, 1.0)
    };
    let step = nice_step((max - min) / target.max(1) as f64);
    let first = (min / step).floor();
    let last = (max / step - 1e-9).ceil();
    (0..=(last - first) as i64)
        .map(|i| clean((first + i as f64) * step, step))
        .collect()
}

/// Like [`nice_ticks`] for spans of seconds, but stepping in whole minutes or hours once the
/// span passes two minutes: `0`, `2m`, `4m` rather than `0`, `100`, `200`.
pub fn duration_ticks(min: f64, max: f64, target: usize) -> Vec<f64> {
    const STEPS: [f64; 10] = [
        60.0, 120.0, 300.0, 600.0, 900.0, 1800.0, 3600.0, 7200.0, 21_600.0, 43_200.0,
    ];
    if !(max - min).is_finite() || max - min <= 120.0 {
        return nice_ticks(min, max, target);
    }
    let raw = (max - min) / target.max(1) as f64;
    let Some(step) = STEPS.into_iter().find(|&step| step >= raw) else {
        // Past half a day, count in days.
        return nice_ticks(min / 86_400.0, max / 86_400.0, target)
            .into_iter()
            .map(|d| d * 86_400.0)
            .collect();
    };
    let first = (min / step).floor();
    let last = (max / step - 1e-9).ceil();
    (0..=(last - first) as i64)
        .map(|i| (first + i as f64) * step)
        .collect()
}

/// The smallest step of 1, 2 or 5 times a power of ten at least `raw`.
fn nice_step(raw: f64) -> f64 {
    let magnitude = 10f64.powf(raw.log10().floor());
    [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|m| m * magnitude)
        .find(|&step| step >= raw * (1.0 - 1e-12))
        .unwrap_or(10.0 * magnitude)
}

/// Rounds away binary noise such as 0.30000000000000004, to a few digits finer than `step`.
fn clean(value: f64, step: f64) -> f64 {
    let digits = (3.0 - step.log10().floor()).clamp(0.0, 300.0) as i32;
    let scale = 10f64.powi(digits);
    let rounded = (value * scale).round() / scale;
    if rounded == 0.0 { 0.0 } else { rounded }
}

/// Ticks for a span of time in milliseconds, `window`, at round steps aligned so that one lands
/// on `now`, labeled by how long before `now` they are: `-4m`, `-30s`, `now`.
pub fn ago_ticks(window: (f64, f64), now: f64) -> Vec<(f64, String)> {
    const STEPS: [f64; 13] = [
        1e3, 5e3, 10e3, 15e3, 30e3, 60e3, 120e3, 300e3, 600e3, 1_800e3, 3_600e3, 7_200e3, 21_600e3,
    ];
    if !(window.0.is_finite() && window.1.is_finite() && now.is_finite()) {
        return Vec::new();
    }
    let span = (window.1 - window.0).max(1.0);
    // Past the largest step, a whole number of them keeps the ticks to about seven.
    let step = STEPS
        .into_iter()
        .find(|step| span / step <= 7.0)
        .unwrap_or_else(|| (span / 7.0 / 21_600e3).ceil() * 21_600e3);
    let mut t = now - ((now - window.0) / step).floor() * step;
    let mut ticks = Vec::new();
    while t <= window.1 + 1e-6 {
        if t >= window.0 - 1e-6 {
            ticks.push((t, format::ago((t - now).round() as i64)));
        }
        t += step;
    }
    ticks
}

/// SVG path data joining `points` with straight lines. A point that isn't finite, such as a
/// missing sample, breaks the line rather than the whole path, and a point alone on its
/// stretch draws as a dot.
pub fn line_path(points: &[(f64, f64)]) -> String {
    let mut d = String::with_capacity(points.len() * 14);
    let finite = |&(x, y): &(f64, f64)| x.is_finite() && y.is_finite();
    for (i, point) in points.iter().enumerate() {
        if !finite(point) {
            continue;
        }
        let (x, y) = point;
        let joined = i > 0 && finite(&points[i - 1]);
        let command = if joined { 'L' } else { 'M' };
        // Writing to a String can't fail.
        let _ = write!(d, "{command}{x:.1} {y:.1}");
        let alone = !joined && points.get(i + 1).is_none_or(|next| !finite(next));
        if alone {
            // A zero-length stroke with round caps.
            d.push_str("h0");
        }
    }
    d
}

/// SVG path data for the area between `points` and the horizontal line at `baseline`.
pub fn area_path(points: &[(f64, f64)], baseline: f64) -> String {
    let points: Vec<_> = points
        .iter()
        .copied()
        .filter(|(x, y)| x.is_finite() && y.is_finite())
        .collect();
    match (points.first(), points.last()) {
        (Some(first), Some(last)) if points.len() > 1 => format!(
            "{}L{:.1} {baseline:.1}L{:.1} {baseline:.1}Z",
            line_path(&points),
            last.0,
            first.0
        ),
        _ => String::new(),
    }
}

/// SVG path data for a band through `points`, each an x with a low and a high y.
pub fn band_path(points: &[(f64, f64, f64)]) -> String {
    if points.len() < 2 {
        return String::new();
    }
    let upper: Vec<_> = points.iter().map(|&(x, _, high)| (x, high)).collect();
    let mut d = line_path(&upper);
    for &(x, low, _) in points.iter().rev() {
        let _ = write!(d, "L{x:.1} {low:.1}");
    }
    d.push('Z');
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn ticks_are_round() {
        assert_eq!(nice_ticks(0.0, 17e6, 4), vec![0.0, 5e6, 10e6, 15e6, 20e6]);
        assert_eq!(nice_ticks(0.0, 1.0, 5), vec![0.0, 0.2, 0.4, 0.6, 0.8, 1.0]);
        assert_eq!(nice_ticks(0.1, 0.3, 2), vec![0.1, 0.2, 0.3]);
        assert_eq!(nice_ticks(5.0, 5.0, 4), vec![5.0, 5.5, 6.0]);
        assert_eq!(nice_ticks(f64::NAN, 1.0, 4), vec![0.0, 0.5, 1.0]);
    }

    #[test]
    fn duration_ticks_step_in_minutes() {
        assert_eq!(
            duration_ticks(0.0, 1000.0, 4),
            vec![0.0, 300.0, 600.0, 900.0, 1200.0]
        );
        assert_eq!(
            duration_ticks(0.0, 600.0, 6),
            vec![0.0, 120.0, 240.0, 360.0, 480.0, 600.0]
        );
        assert_eq!(duration_ticks(0.0, 50.0, 5), nice_ticks(0.0, 50.0, 5));
        assert_eq!(
            duration_ticks(0.0, 4.0 * 86_400.0, 4),
            vec![0.0, 86_400.0, 172_800.0, 259_200.0, 345_600.0]
        );
    }

    #[test]
    fn time_ticks_land_on_now() {
        let ticks = ago_ticks((0.0, 300_000.0), 300_000.0);
        let labels: Vec<_> = ticks.iter().map(|(_, l)| l.as_str()).collect();
        assert_eq!(labels, ["-5m", "-4m", "-3m", "-2m", "-1m", "now"]);
        let zoomed = ago_ticks((100_000.0, 160_000.0), 300_000.0);
        assert_eq!(zoomed.first().unwrap().1, "-3m 20s");
        assert_eq!(zoomed.len(), 7);
    }

    #[test]
    fn paths() {
        assert_eq!(line_path(&[(0.0, 1.0), (2.0, 3.25)]), "M0.0 1.0L2.0 3.2");
        assert_eq!(
            area_path(&[(0.0, 1.0), (2.0, 3.0)], 10.0),
            "M0.0 1.0L2.0 3.0L2.0 10.0L0.0 10.0Z"
        );
        assert_eq!(area_path(&[(0.0, 1.0)], 10.0), "");
        assert_eq!(
            band_path(&[(0.0, 5.0, 1.0), (2.0, 6.0, 2.0)]),
            "M0.0 1.0L2.0 2.0L2.0 6.0L0.0 5.0Z"
        );
    }

    #[test]
    fn gaps_break_lines_and_lone_points_show() {
        let nan = f64::NAN;
        assert_eq!(line_path(&[(0.0, 1.0), (1.0, 2.0)]), "M0.0 1.0L1.0 2.0");
        assert_eq!(
            line_path(&[(0.0, 1.0), (1.0, nan), (2.0, 3.0), (3.0, 4.0)]),
            "M0.0 1.0h0M2.0 3.0L3.0 4.0"
        );
        assert_eq!(line_path(&[(5.0, 5.0)]), "M5.0 5.0h0");
        assert_eq!(area_path(&[(0.0, 1.0), (1.0, nan)], 9.0), "");
    }

    #[test]
    fn time_ticks_stay_few_and_finite() {
        assert!(ago_ticks((0.0, f64::INFINITY), 0.0).is_empty());
        let day = 86_400e3;
        let ticks = ago_ticks((-day, 0.0), 0.0);
        assert!((2..=8).contains(&ticks.len()), "{}", ticks.len());
        let week = ago_ticks((-7.0 * day, 0.0), 0.0);
        assert!((2..=8).contains(&week.len()), "{}", week.len());
    }

    proptest! {
        #[test]
        fn tiny_spans_get_distinct_ticks(min in -1.0f64..1.0, span in 1e-12f64..1e-3) {
            let ticks = nice_ticks(min, min + span, 4);
            prop_assert!(ticks.len() >= 2);
            prop_assert!(ticks.windows(2).all(|w| w[1] > w[0]), "{:?}", ticks);
        }

        #[test]
        fn ticks_cover_the_span(min in -1e6f64..1e6, span in 1e-3f64..1e9, target in 1usize..10) {
            let max = min + span;
            let ticks = nice_ticks(min, max, target);
            prop_assert!(ticks[0] <= min + span * 1e-6);
            prop_assert!(*ticks.last().unwrap() >= max - span * 1e-6);
            prop_assert!(ticks.len() >= 2 && ticks.len() <= 2 * target + 3, "{ticks:?}");
            prop_assert!(ticks.windows(2).all(|w| w[1] > w[0]));
        }

        #[test]
        fn scale_inverts(d0 in -1e6f64..1e6, dspan in 1e-3f64..1e6, r0 in 0f64..1e3, rspan in 1f64..1e3, v in -1e6f64..1e6) {
            let scale = Scale { domain: (d0, d0 + dspan), range: (r0, r0 + rspan) };
            let back = scale.invert(scale.map(v));
            prop_assert!((back - v).abs() <= 1e-6 * (1.0 + v.abs() + dspan));
        }
    }
}
