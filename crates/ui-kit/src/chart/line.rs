//! Line charts: series over time or any other x, with optional confidence bands, and what the
//! pointer is over.

use std::collections::BTreeSet;

use super::scale::{self, Layout, Scale, Tick};
use crate::format;

/// What a chart's values mean, which decides how they read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Unit {
    /// Bytes per second.
    Rate,
    /// Bytes.
    Bytes,
    /// A fraction from 0 to 1, read as a percentage.
    Fraction,
    /// Seconds.
    Seconds,
    /// A plain number.
    #[default]
    Number,
}

impl Unit {
    /// `value` in full, for tooltips.
    pub fn format(self, value: f64) -> String {
        match self {
            Unit::Rate => format::rate(value),
            Unit::Bytes => format::bytes(value.max(0.0) as u64),
            Unit::Fraction => format::percent(value),
            Unit::Seconds if value.abs() >= 60.0 => format::duration(value.abs() as u64),
            Unit::Seconds => format!("{}s", format::compact(value)),
            Unit::Number => format::compact(value),
        }
    }

    /// Round values from `min` to `max` for an axis, about `target` of them.
    pub fn ticks(self, min: f64, max: f64, target: usize) -> Vec<f64> {
        match self {
            Unit::Seconds => scale::duration_ticks(min, max, target),
            _ => scale::nice_ticks(min, max, target),
        }
    }

    /// `value` briefly, for axis labels.
    pub fn axis(self, value: f64) -> String {
        match self {
            Unit::Rate if value < 1.0 => "0".to_owned(),
            Unit::Fraction => format!("{}%", format::compact(value * 100.0)),
            Unit::Seconds if value.abs() >= 86_400.0 && value % 86_400.0 == 0.0 => {
                format!("{}d", value / 86_400.0)
            }
            Unit::Seconds if value.abs() >= 3600.0 && value % 3600.0 == 0.0 => {
                format!("{}h", value / 3600.0)
            }
            Unit::Seconds if value.abs() >= 60.0 && value % 60.0 == 0.0 => {
                format!("{}m", value / 60.0)
            }
            unit => unit.format(value),
        }
    }
}

/// What a line chart's x axis shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum XAxis {
    /// Milliseconds, labeled by how long before the latest sample they are.
    Ago,
    /// Values in a unit.
    Value(Unit),
    /// Plain numbers.
    #[default]
    Number,
}

/// A color from the theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Color {
    /// Downloads.
    Down,
    /// Uploads.
    Up,
    /// The brightest accent.
    Hot,
    /// Warnings.
    Warn,
    /// Errors.
    Bad,
    /// Body text.
    Fg,
    /// One of the six series colors, counted from 0 and wrapping.
    Series(usize),
}

impl Color {
    /// The CSS value for the color.
    pub fn css(self) -> String {
        match self {
            Color::Down => "var(--down)".to_owned(),
            Color::Up => "var(--up)".to_owned(),
            Color::Hot => "var(--hot)".to_owned(),
            Color::Warn => "var(--warn)".to_owned(),
            Color::Bad => "var(--bad)".to_owned(),
            Color::Fg => "var(--fg)".to_owned(),
            Color::Series(i) => format!("var(--series-{})", i % 6 + 1),
        }
    }

    /// A style that sets `--c`, which chart parts color themselves with.
    pub fn style(self) -> String {
        format!("--c:{}", self.css())
    }
}

/// One line on a chart.
#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    /// Stays the same across updates, so toggles and the DOM follow the series.
    pub key: String,
    /// Its name in the legend and tooltip.
    pub label: String,
    /// Its color.
    pub color: Color,
    /// Its points, in increasing x.
    pub points: Vec<(f64, f64)>,
    /// A low and high y per point, such as a confidence interval, or empty for none.
    pub band: Vec<(f64, f64)>,
    /// Shade the area under the line.
    pub fill: bool,
    /// Draw the line dashed.
    pub dashed: bool,
    /// Make the line glow.
    pub glow: bool,
}

impl Series {
    /// A plain series of `points`.
    pub fn new(
        key: impl Into<String>,
        label: impl Into<String>,
        color: Color,
        points: Vec<(f64, f64)>,
    ) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            color,
            points,
            band: Vec::new(),
            fill: false,
            dashed: false,
            glow: false,
        }
    }

    /// With a band: a low and high y for each point.
    pub fn with_band(mut self, band: Vec<(f64, f64)>) -> Self {
        self.band = band;
        self
    }

    /// With the area under the line shaded and the line glowing.
    pub fn filled(mut self) -> Self {
        self.fill = true;
        self.glow = true;
        self
    }

    /// With a dashed line.
    pub fn dashed(mut self) -> Self {
        self.dashed = true;
        self
    }

    /// The index of the point with x nearest `x`.
    fn nearest(&self, x: f64) -> Option<usize> {
        let after = self.points.partition_point(|&(px, _)| px < x);
        [
            after.checked_sub(1),
            (after < self.points.len()).then_some(after),
        ]
        .into_iter()
        .flatten()
        .min_by(|&a, &b| {
            (self.points[a].0 - x)
                .abs()
                .total_cmp(&(self.points[b].0 - x).abs())
        })
    }
}

/// Everything a line chart shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LineData {
    /// The lines.
    pub series: Vec<Series>,
    /// What x means.
    pub x: XAxis,
    /// What y means.
    pub y: Unit,
    /// The lowest the y axis may top out at, so a quiet chart doesn't magnify noise.
    pub y_floor: f64,
}

/// A series as SVG path data.
#[derive(Clone, Debug, PartialEq)]
pub struct SeriesPaths {
    /// The series' key.
    pub key: String,
    /// Its color.
    pub color: Color,
    /// The line.
    pub line: String,
    /// The area under it, or empty.
    pub area: String,
    /// Its band, or empty.
    pub band: String,
    /// Whether the line is dashed.
    pub dashed: bool,
    /// Whether the line glows.
    pub glow: bool,
}

/// A line chart laid out for drawing.
#[derive(Clone, Debug, PartialEq)]
pub struct LinePlot {
    /// Its size and margins.
    pub layout: Layout,
    /// Maps x values to pixels.
    pub x: Scale,
    /// Maps y values to pixels.
    pub y: Scale,
    /// The x axis's ticks.
    pub x_ticks: Vec<Tick>,
    /// The y axis's ticks, which are also the grid lines.
    pub y_ticks: Vec<Tick>,
    /// The shown series' paths, in the data's order.
    pub paths: Vec<SeriesPaths>,
    /// The x values shown.
    pub window: (f64, f64),
    /// Whether that is less than all the data.
    pub zoomed: bool,
}

impl LineData {
    /// The x values the data spans, or `None` without points.
    pub fn extent(&self) -> Option<(f64, f64)> {
        let xs = self
            .series
            .iter()
            .flat_map(|s| s.points.iter().map(|p| p.0));
        xs.fold(None, |acc, x| match acc {
            None => Some((x, x)),
            Some((lo, hi)) => Some((x.min(lo), x.max(hi))),
        })
    }

    /// Lays the chart out in `layout`, showing x values in `window` (or all of them) and
    /// leaving out the series whose keys are `hidden`.
    pub fn plot(
        &self,
        layout: Layout,
        window: Option<(f64, f64)>,
        hidden: &BTreeSet<String>,
    ) -> LinePlot {
        let full = match self.extent() {
            Some((lo, hi)) if hi > lo => (lo, hi),
            Some((lo, _)) => (lo - 0.5, lo + 0.5),
            None => (0.0, 1.0),
        };
        let window = window.filter(|w| w.1 > w.0).unwrap_or(full);
        let zoomed = window != full;
        let shown = || self.series.iter().filter(|s| !hidden.contains(&s.key));

        // The y axis covers every shown value in the window, bands included, and zero.
        let (mut low, mut high) = (0.0_f64, self.y_floor);
        for series in shown() {
            for (i, &(x, y)) in series.points.iter().enumerate() {
                if x < window.0 || x > window.1 {
                    continue;
                }
                let (lo, hi) = series.band.get(i).copied().unwrap_or((y, y));
                low = low.min(y).min(lo);
                high = high.max(y).max(hi);
            }
        }
        if high <= low {
            high = low + 1.0;
        }
        let y_values = self.y.ticks(low, high, 4);
        let y = Scale {
            domain: (y_values[0], *y_values.last().unwrap_or(&high)),
            range: (layout.plot_bottom(), layout.plot_top()),
        };
        let x = Scale {
            domain: window,
            range: (layout.plot_left(), layout.plot_right()),
        };

        let x_ticks = match self.x {
            XAxis::Ago => scale::ago_ticks(window, full.1),
            XAxis::Value(unit) => {
                labeled(&unit.ticks(window.0, window.1, 6), window, |v| unit.axis(v))
            }
            XAxis::Number => labeled(
                &scale::nice_ticks(window.0, window.1, 6),
                window,
                format::compact,
            ),
        };
        let x_ticks = x_ticks
            .into_iter()
            .map(|(value, label)| Tick {
                pos: x.map(value),
                label,
            })
            .collect();
        let y_ticks = y_values
            .iter()
            .map(|&value| Tick {
                pos: y.map(value),
                label: self.y.axis(value),
            })
            .collect();

        let paths = shown()
            .map(|series| {
                // One point either side of the window keeps lines running to its edges; the
                // plot is clipped.
                let first = series
                    .points
                    .partition_point(|p| p.0 < window.0)
                    .saturating_sub(1);
                let end = (series.points.partition_point(|p| p.0 <= window.1) + 1)
                    .min(series.points.len());
                let range = first..end.max(first);
                let points: Vec<_> = series.points[range.clone()]
                    .iter()
                    .map(|&(px, py)| (x.map(px), y.map(py)))
                    .collect();
                let band: Vec<_> = range
                    .filter_map(|i| Some((series.points[i].0, series.band.get(i)?)))
                    .map(|(px, &(lo, hi))| (x.map(px), y.map(lo), y.map(hi)))
                    .collect();
                SeriesPaths {
                    key: series.key.clone(),
                    color: series.color,
                    line: scale::line_path(&points),
                    area: if series.fill {
                        scale::area_path(&points, y.map(0.0_f64.clamp(y.domain.0, y.domain.1)))
                    } else {
                        String::new()
                    },
                    band: scale::band_path(&band),
                    dashed: series.dashed,
                    glow: series.glow,
                }
            })
            .collect();

        LinePlot {
            layout,
            x,
            y,
            x_ticks,
            y_ticks,
            paths,
            window,
            zoomed,
        }
    }

    /// What the chart shows at pixel `px`: the sample nearest the pointer and each shown
    /// series' value there. `None` when the pointer is off the plot or nothing is shown.
    pub fn hover(&self, plot: &LinePlot, hidden: &BTreeSet<String>, px: f64) -> Option<Hover> {
        let layout = plot.layout;
        if px < layout.plot_left() - 1.0 || px > layout.plot_right() + 1.0 {
            return None;
        }
        let target = plot.x.invert(px);
        let in_window = |x: f64| x >= plot.window.0 && x <= plot.window.1;
        let shown: Vec<_> = self
            .series
            .iter()
            .filter(|s| !hidden.contains(&s.key))
            .collect();
        let x = shown
            .iter()
            .filter_map(|s| s.nearest(target).map(|i| s.points[i].0))
            .filter(|&x| in_window(x))
            .min_by(|a, b| (a - target).abs().total_cmp(&(b - target).abs()))?;
        let rows = shown
            .iter()
            .filter_map(|series| {
                let i = series.nearest(x)?;
                let (_, value) = series.points[i];
                let mut text = self.y.format(value);
                if let Some(&(lo, hi)) = series.band.get(i) {
                    text = format!("{text} ({}–{})", self.y.format(lo), self.y.format(hi));
                }
                Some(HoverRow {
                    label: series.label.clone(),
                    color: series.color,
                    value: text,
                    y: plot.y.map(value),
                })
            })
            .collect();
        let title = match self.x {
            XAxis::Ago => format::ago((x - plot_end(self)).round() as i64),
            XAxis::Value(unit) => unit.format(x),
            XAxis::Number => format::compact(x),
        };
        Some(Hover {
            x: plot.x.map(x),
            title,
            rows,
        })
    }
}

/// The latest x in the data.
fn plot_end(data: &LineData) -> f64 {
    data.extent().map_or(0.0, |e| e.1)
}

/// Labels the values inside `window`.
fn labeled(
    values: &[f64],
    window: (f64, f64),
    label: impl Fn(f64) -> String,
) -> Vec<(f64, String)> {
    values
        .iter()
        .filter(|&&v| v >= window.0 - 1e-9 && v <= window.1 + 1e-9)
        .map(|&v| (v, label(v)))
        .collect()
}

/// What a line chart shows under the pointer.
#[derive(Clone, Debug, PartialEq)]
pub struct Hover {
    /// The pixel x of the sample, for the crosshair.
    pub x: f64,
    /// The sample's x, as text.
    pub title: String,
    /// Each shown series' value there.
    pub rows: Vec<HoverRow>,
}

/// One series' value under the pointer.
#[derive(Clone, Debug, PartialEq)]
pub struct HoverRow {
    /// The series' label.
    pub label: String,
    /// Its color.
    pub color: Color,
    /// The value, as text.
    pub value: String,
    /// The value's pixel y, for its dot.
    pub y: f64,
}

/// The x values to zoom to after a drag from pixel `px0` to `px1`, or `None` for a drag under
/// eight pixels, which is more likely a click.
pub fn zoom(plot: &LinePlot, px0: f64, px1: f64) -> Option<(f64, f64)> {
    let layout = plot.layout;
    let a = px0.min(px1).max(layout.plot_left());
    let b = px0.max(px1).min(layout.plot_right());
    (b - a >= 8.0).then(|| (plot.x.invert(a), plot.x.invert(b)))
}

/// SVG path data for a sparkline of `values` in a `width` by `height` box: the line, and the
/// area under it. The y axis starts at zero.
pub fn sparkline(values: &[f64], width: f64, height: f64) -> (String, String) {
    if values.len() < 2 {
        return (String::new(), String::new());
    }
    let max = values.iter().copied().fold(0.0_f64, f64::max);
    let max = if max > 0.0 { max } else { 1.0 };
    let step = width / (values.len() - 1) as f64;
    let points: Vec<_> = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            (
                i as f64 * step,
                height - 1.0 - (v.max(0.0) / max) * (height - 2.0),
            )
        })
        .collect();
    (scale::line_path(&points), scale::area_path(&points, height))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn rates() -> LineData {
        let down = (0..=60)
            .map(|s| (s as f64 * 1000.0, 1e6 + s as f64 * 1e5))
            .collect();
        let up = (0..=60).map(|s| (s as f64 * 1000.0, 2e5)).collect();
        LineData {
            series: vec![
                Series::new("down", "download", Color::Down, down).filled(),
                Series::new("up", "upload", Color::Up, up).dashed(),
            ],
            x: XAxis::Ago,
            y: Unit::Rate,
            y_floor: 1e6,
        }
    }

    #[test]
    fn plots_and_hovers() {
        let data = rates();
        let none = BTreeSet::new();
        let plot = data.plot(Layout::new(600.0, 240.0), None, &none);
        assert!(!plot.zoomed);
        assert_eq!(plot.paths.len(), 2);
        assert!(plot.paths[0].line.starts_with('M') && plot.paths[0].area.ends_with('Z'));
        assert!(plot.paths[1].area.is_empty() && plot.paths[1].dashed);
        assert_eq!(plot.x_ticks.last().unwrap().label, "now");
        assert_eq!(plot.y_ticks.last().unwrap().label, "8.00 MB/s");

        let hover = data.hover(&plot, &none, plot.layout.plot_right()).unwrap();
        assert_eq!(hover.title, "now");
        assert_eq!(hover.rows[0].value, "7.00 MB/s");
        assert_eq!(hover.rows[1].value, "200 KB/s");
        assert!(data.hover(&plot, &none, 5.0).is_none());

        let hidden = BTreeSet::from(["down".to_owned()]);
        let plot = data.plot(Layout::new(600.0, 240.0), None, &hidden);
        assert_eq!(plot.paths.len(), 1);
        assert_eq!(plot.y_ticks.last().unwrap().label, "1.00 MB/s");
    }

    #[test]
    fn zooms() {
        let data = rates();
        let none = BTreeSet::new();
        let plot = data.plot(Layout::new(600.0, 240.0), None, &none);
        assert_eq!(zoom(&plot, 100.0, 104.0), None);
        let window = zoom(&plot, 300.0, 100.0).unwrap();
        let zoomed = data.plot(plot.layout, Some(window), &none);
        assert!(zoomed.zoomed);
        assert!(
            zoomed
                .x_ticks
                .iter()
                .all(|t| t.pos >= plot.layout.plot_left() - 1e-6)
        );
        let hover = data
            .hover(&zoomed, &none, zoomed.layout.plot_right())
            .unwrap();
        assert!(hover.title.starts_with('-'));
    }

    #[test]
    fn bands_widen_the_axis_and_show_in_the_tooltip() {
        let data = LineData {
            series: vec![
                Series::new(
                    "a",
                    "run a",
                    Color::Series(0),
                    vec![(0.0, 0.5), (10.0, 0.6)],
                )
                .with_band(vec![(0.4, 0.6), (0.5, 0.9)]),
            ],
            x: XAxis::Value(Unit::Seconds),
            y: Unit::Fraction,
            y_floor: 0.0,
        };
        let plot = data.plot(Layout::new(400.0, 200.0), None, &BTreeSet::new());
        assert!(plot.paths[0].band.ends_with('Z'));
        assert_eq!(plot.y_ticks.last().unwrap().label, "100%");
        let hover = data
            .hover(&plot, &BTreeSet::new(), plot.layout.plot_right())
            .unwrap();
        assert_eq!(hover.title, "10s");
        assert_eq!(hover.rows[0].value, "60.0% (50.0%–90.0%)");
    }

    #[test]
    fn empty_data_plots_nothing() {
        let data = LineData::default();
        let plot = data.plot(Layout::new(300.0, 100.0), None, &BTreeSet::new());
        assert!(plot.paths.is_empty());
        assert!(data.hover(&plot, &BTreeSet::new(), 150.0).is_none());
        assert_eq!(
            sparkline(&[1.0], 10.0, 10.0),
            (String::new(), String::new())
        );
    }

    proptest! {
        #[test]
        fn hover_stays_on_the_plot(ys in prop::collection::vec(0f64..1e9, 2..80), px in 0f64..600.0) {
            let points = ys.iter().enumerate().map(|(i, &y)| (i as f64, y)).collect();
            let data = LineData {
                series: vec![Series::new("s", "s", Color::Series(0), points)],
                ..LineData::default()
            };
            let plot = data.plot(Layout::new(600.0, 200.0), None, &BTreeSet::new());
            if let Some(hover) = data.hover(&plot, &BTreeSet::new(), px) {
                let l = plot.layout;
                prop_assert!(hover.x >= l.plot_left() - 1e-6 && hover.x <= l.plot_right() + 1e-6);
                prop_assert!(hover.rows[0].y >= l.plot_top() - 1e-6 && hover.rows[0].y <= l.plot_bottom() + 1e-6);
            }
        }
    }
}
