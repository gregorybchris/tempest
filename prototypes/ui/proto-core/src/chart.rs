//! Chart geometry: scales, ticks and SVG path data.
//!
//! Each prototype renders these into its own SVG and handles interaction
//! (hover, drag-to-zoom, series toggles) by calling [`hover`] and [`zoom`].

use crate::format;
use crate::sim::Sample;

/// The chart's size and the padding around its plot area, in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub width: f64,
    pub height: f64,
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
}

impl Layout {
    /// The rates chart's default layout.
    pub fn rates(width: f64, height: f64) -> Self {
        Self {
            width,
            height,
            left: 72.0,
            right: 16.0,
            top: 12.0,
            bottom: 28.0,
        }
    }

    pub fn plot_left(&self) -> f64 {
        self.left
    }
    pub fn plot_right(&self) -> f64 {
        self.width - self.right
    }
    pub fn plot_top(&self) -> f64 {
        self.top
    }
    pub fn plot_bottom(&self) -> f64 {
        self.height - self.bottom
    }
}

/// A linear map from a domain to a pixel range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scale {
    pub domain: (f64, f64),
    pub range: (f64, f64),
}

impl Scale {
    pub fn map(&self, v: f64) -> f64 {
        let (d0, d1) = self.domain;
        let (r0, r1) = self.range;
        if (d1 - d0).abs() < f64::EPSILON {
            return r0;
        }
        r0 + (v - d0) / (d1 - d0) * (r1 - r0)
    }

    pub fn invert(&self, px: f64) -> f64 {
        let (d0, d1) = self.domain;
        let (r0, r1) = self.range;
        if (r1 - r0).abs() < f64::EPSILON {
            return d0;
        }
        d0 + (px - r0) / (r1 - r0) * (d1 - d0)
    }
}

/// An axis tick: a pixel position and its label.
#[derive(Clone, Debug, PartialEq)]
pub struct Tick {
    pub pos: f64,
    pub label: String,
}

/// Which series are shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Visible {
    pub down: bool,
    pub up: bool,
}

impl Default for Visible {
    fn default() -> Self {
        Self {
            down: true,
            up: true,
        }
    }
}

/// One series' SVG path data.
#[derive(Clone, Debug, PartialEq)]
pub struct SeriesPath {
    /// `down` or `up`, used as a CSS modifier class.
    pub key: &'static str,
    /// Legend label, e.g. `download`.
    pub label: &'static str,
    /// SVG path data for the line.
    pub line: String,
    /// SVG path data for the filled area under the line.
    pub area: String,
}

/// Everything needed to draw the rates chart.
#[derive(Clone, Debug, PartialEq)]
pub struct RateChart {
    pub layout: Layout,
    pub x: Scale,
    pub y: Scale,
    pub x_ticks: Vec<Tick>,
    pub y_ticks: Vec<Tick>,
    pub series: Vec<SeriesPath>,
    /// The time window shown, in simulation milliseconds.
    pub window: (u64, u64),
    /// Whether the chart is zoomed in (not showing all history).
    pub zoomed: bool,
}

/// Builds the rates chart for `history`, optionally zoomed to `window`.
pub fn rates(
    history: &[Sample],
    window: Option<(u64, u64)>,
    layout: Layout,
    visible: Visible,
) -> RateChart {
    let full = match (history.first(), history.last()) {
        (Some(a), Some(b)) => (a.t_ms, b.t_ms.max(a.t_ms + 1)),
        _ => (0, 1),
    };
    let window = window.unwrap_or(full);
    let zoomed = window != full;
    let shown: Vec<&Sample> = history
        .iter()
        .filter(|s| s.t_ms + 1000 >= window.0 && s.t_ms <= window.1 + 1000)
        .collect();

    let peak = shown
        .iter()
        .flat_map(|s| {
            [
                visible.down.then_some(s.down_bps),
                visible.up.then_some(s.up_bps),
            ]
            .into_iter()
            .flatten()
        })
        .fold(0.0_f64, f64::max);
    let y_ticks_values = nice_ticks(0.0, (peak * 1.12).max(1_000_000.0), 4);
    let y_max = *y_ticks_values.last().unwrap_or(&1.0);

    let x = Scale {
        domain: (window.0 as f64, window.1 as f64),
        range: (layout.plot_left(), layout.plot_right()),
    };
    let y = Scale {
        domain: (0.0, y_max),
        range: (layout.plot_bottom(), layout.plot_top()),
    };

    let now = full.1 as i64;
    let x_ticks = time_ticks(window, now)
        .into_iter()
        .map(|(t, label)| Tick {
            pos: x.map(t as f64),
            label,
        })
        .collect();
    let y_ticks = y_ticks_values
        .into_iter()
        .map(|v| Tick {
            pos: y.map(v),
            label: format::rate(v).replace("—", "0"),
        })
        .collect();

    let mut series = Vec::new();
    if visible.down {
        let pts: Vec<(f64, f64)> = shown
            .iter()
            .map(|s| (x.map(s.t_ms as f64), y.map(s.down_bps)))
            .collect();
        series.push(SeriesPath {
            key: "down",
            label: "download",
            line: line_path(&pts),
            area: area_path(&pts, y.map(0.0)),
        });
    }
    if visible.up {
        let pts: Vec<(f64, f64)> = shown
            .iter()
            .map(|s| (x.map(s.t_ms as f64), y.map(s.up_bps)))
            .collect();
        series.push(SeriesPath {
            key: "up",
            label: "upload",
            line: line_path(&pts),
            area: area_path(&pts, y.map(0.0)),
        });
    }

    RateChart {
        layout,
        x,
        y,
        x_ticks,
        y_ticks,
        series,
        window,
        zoomed,
    }
}

/// What the chart shows under the pointer.
#[derive(Clone, Debug, PartialEq)]
pub struct Hover {
    /// Pixel x of the nearest sample (for the crosshair).
    pub x: f64,
    /// Pixel y of each visible series at that sample: `(key, y)`.
    pub points: Vec<(&'static str, f64)>,
    pub sample: Sample,
    /// Tooltip lines, e.g. `14:03:27`, `download 14.2 MB/s`.
    pub lines: Vec<String>,
}

/// Finds the sample nearest to pixel `px`, if the pointer is over the plot.
pub fn hover(chart: &RateChart, history: &[Sample], visible: Visible, px: f64) -> Option<Hover> {
    let l = chart.layout;
    if px < l.plot_left() || px > l.plot_right() {
        return None;
    }
    let t = chart.x.invert(px);
    let sample = *history
        .iter()
        .filter(|s| s.t_ms >= chart.window.0 && s.t_ms <= chart.window.1)
        .min_by(|a, b| {
            (a.t_ms as f64 - t)
                .abs()
                .total_cmp(&(b.t_ms as f64 - t).abs())
        })?;
    let mut points = Vec::new();
    let mut lines = vec![format::clock(sample.t_ms)];
    if visible.down {
        points.push(("down", chart.y.map(sample.down_bps)));
        lines.push(format!("download {}", format::rate(sample.down_bps)));
    }
    if visible.up {
        points.push(("up", chart.y.map(sample.up_bps)));
        lines.push(format!("upload {}", format::rate(sample.up_bps)));
    }
    lines.push(format!("{} peers", sample.peers));
    Some(Hover {
        x: chart.x.map(sample.t_ms as f64),
        points,
        sample,
        lines,
    })
}

/// Turns a drag from pixel `px0` to `px1` into a zoom window. Returns `None`
/// for drags too short to mean anything (under 8 px or 10 s).
pub fn zoom(chart: &RateChart, px0: f64, px1: f64) -> Option<(u64, u64)> {
    let l = chart.layout;
    let (a, b) = (
        px0.min(px1).max(l.plot_left()),
        px0.max(px1).min(l.plot_right()),
    );
    if b - a < 8.0 {
        return None;
    }
    let (t0, t1) = (chart.x.invert(a) as u64, chart.x.invert(b) as u64);
    (t1 - t0 >= 10_000).then_some((t0, t1))
}

/// SVG path data for a sparkline of `values` in a `width` × `height` box:
/// `(line, area)`. The y axis starts at zero.
pub fn sparkline(values: &[f64], width: f64, height: f64) -> (String, String) {
    if values.len() < 2 {
        return (String::new(), String::new());
    }
    let max = values.iter().copied().fold(0.0_f64, f64::max).max(1.0);
    let step = width / (values.len() - 1) as f64;
    let pts: Vec<(f64, f64)> = values
        .iter()
        .enumerate()
        .map(|(i, v)| (i as f64 * step, height - 1.0 - (v / max) * (height - 2.0)))
        .collect();
    (line_path(&pts), area_path(&pts, height))
}

fn line_path(pts: &[(f64, f64)]) -> String {
    let mut d = String::with_capacity(pts.len() * 16);
    for (i, (x, y)) in pts.iter().enumerate() {
        d.push_str(if i == 0 { "M" } else { "L" });
        d.push_str(&format!("{x:.1} {y:.1}"));
    }
    d
}

fn area_path(pts: &[(f64, f64)], baseline: f64) -> String {
    match (pts.first(), pts.last()) {
        (Some(first), Some(last)) => {
            format!(
                "{}L{:.1} {baseline:.1}L{:.1} {baseline:.1}Z",
                line_path(pts),
                last.0,
                first.0
            )
        }
        _ => String::new(),
    }
}

/// "Nice" tick values from `min` to at least `max`: steps of 1, 2 or 5 × 10ⁿ.
pub fn nice_ticks(min: f64, max: f64, target: usize) -> Vec<f64> {
    let span = (max - min).max(f64::EPSILON);
    let raw = span / target.max(1) as f64;
    let mag = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 5.0, 10.0]
        .iter()
        .map(|m| m * mag)
        .find(|s| *s >= raw)
        .unwrap_or(10.0 * mag);
    let mut ticks = Vec::new();
    let mut v = (min / step).floor() * step;
    loop {
        ticks.push(v);
        if v >= max - step * 1e-9 {
            return ticks;
        }
        v += step;
    }
}

/// Time ticks for `window`, labeled relative to `now`, e.g. `-4m`.
fn time_ticks(window: (u64, u64), now: i64) -> Vec<(u64, String)> {
    let span = window.1.saturating_sub(window.0);
    let step = [10_000, 15_000, 30_000, 60_000, 120_000, 300_000]
        .into_iter()
        .find(|s| span / s <= 7)
        .unwrap_or(600_000);
    // Align ticks to `now`, so the rightmost tick reads "now" when it's visible.
    let mut ticks = Vec::new();
    let mut t = now - ((now - window.0 as i64).max(0) / step as i64) * step as i64;
    while t <= window.1 as i64 {
        if t >= window.0 as i64 {
            ticks.push((t as u64, format::ago(t - now)));
        }
        t += step as i64;
    }
    ticks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::World;

    #[test]
    fn builds_chart() {
        let w = World::new(3);
        let history: Vec<Sample> = w.bundles[0].history.iter().copied().collect();
        let chart = rates(
            &history,
            None,
            Layout::rates(800.0, 240.0),
            Visible::default(),
        );
        assert_eq!(chart.series.len(), 2);
        assert!(chart.series[0].line.starts_with('M'));
        assert!(!chart.x_ticks.is_empty() && !chart.y_ticks.is_empty());
        assert_eq!(chart.x_ticks.last().unwrap().label, "now");
        let h = hover(&chart, &history, Visible::default(), 400.0).unwrap();
        assert_eq!(h.lines.len(), 4);
        let window = zoom(&chart, 200.0, 400.0).unwrap();
        let zoomed = rates(
            &history,
            Some(window),
            Layout::rates(800.0, 240.0),
            Visible::default(),
        );
        assert!(zoomed.zoomed);
    }

    #[test]
    fn ticks_are_nice() {
        assert_eq!(
            nice_ticks(0.0, 17_000_000.0, 4),
            vec![0.0, 5e6, 10e6, 15e6, 20e6]
        );
    }
}
