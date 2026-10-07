//! Bar charts with confidence intervals, for comparing runs and configurations.

use super::line::{Color, Unit};
use super::scale::{Layout, Scale, Tick};

/// One bar: a value, and optionally the interval it is likely in.
#[derive(Clone, Debug, PartialEq)]
pub struct Bar {
    /// Its name under the axis.
    pub label: String,
    /// Its height.
    pub value: f64,
    /// A low and high value, such as a 95% confidence interval.
    pub interval: Option<(f64, f64)>,
    /// A line of detail for the tooltip, such as the number of seeds.
    pub note: Option<String>,
    /// Its color.
    pub color: Color,
}

/// A bar laid out for drawing.
#[derive(Clone, Debug, PartialEq)]
pub struct BarShape {
    /// Its left edge.
    pub x: f64,
    /// Its width.
    pub width: f64,
    /// Its top edge, the higher of the value and zero.
    pub top: f64,
    /// Its height.
    pub height: f64,
    /// The interval's ends, as pixel ys.
    pub whisker: Option<(f64, f64)>,
    /// Its label, shortened to fit.
    pub label: String,
    /// Its color.
    pub color: Color,
    /// The tooltip's lines.
    pub tooltip: Vec<String>,
}

impl BarShape {
    /// Its horizontal center.
    pub fn center(&self) -> f64 {
        self.x + self.width / 2.0
    }
}

/// A bar chart laid out for drawing.
#[derive(Clone, Debug, PartialEq)]
pub struct BarPlot {
    /// Its size and margins.
    pub layout: Layout,
    /// Maps values to pixels.
    pub y: Scale,
    /// The y axis's ticks.
    pub y_ticks: Vec<Tick>,
    /// The bars, in order.
    pub bars: Vec<BarShape>,
}

/// Lays out `bars`, whose values are in `unit`, in `layout`.
pub fn plot(bars: &[Bar], unit: Unit, layout: Layout) -> BarPlot {
    let (mut low, mut high) = (0.0_f64, 0.0_f64);
    for bar in bars {
        let (lo, hi) = bar.interval.unwrap_or((bar.value, bar.value));
        low = low.min(bar.value).min(lo);
        high = high.max(bar.value).max(hi);
    }
    if high <= low {
        high = low + 1.0;
    }
    let values = unit.ticks(low, high, 4);
    let y = Scale {
        domain: (values[0], *values.last().unwrap_or(&high)),
        range: (layout.plot_bottom(), layout.plot_top()),
    };
    let y_ticks = values
        .iter()
        .map(|&v| Tick {
            pos: y.map(v),
            label: unit.axis(v),
        })
        .collect();

    let slot = (layout.plot_right() - layout.plot_left()) / bars.len().max(1) as f64;
    let width = (slot * 0.62).min(72.0);
    // Labels get about one character per 6.6 pixels of their slot.
    let chars = ((slot / 6.6).floor() as usize).max(3);
    let zero = y.map(0.0);
    let bars = bars
        .iter()
        .enumerate()
        .map(|(i, bar)| {
            let value = y.map(bar.value);
            let mut tooltip = vec![bar.label.clone(), unit.format(bar.value)];
            if let Some((lo, hi)) = bar.interval {
                tooltip[1] = format!("{} ({}–{})", tooltip[1], unit.format(lo), unit.format(hi));
            }
            tooltip.extend(bar.note.clone());
            BarShape {
                x: layout.plot_left() + slot * i as f64 + (slot - width) / 2.0,
                width,
                top: value.min(zero),
                height: (value - zero).abs(),
                whisker: bar.interval.map(|(lo, hi)| (y.map(lo), y.map(hi))),
                label: shorten(&bar.label, chars),
                color: bar.color,
                tooltip,
            }
        })
        .collect();
    BarPlot {
        layout,
        y,
        y_ticks,
        bars,
    }
}

/// `text`, cut to `max` characters with an ellipsis when longer.
pub fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut short: String = text.chars().take(max.saturating_sub(1)).collect();
    short.push('…');
    short
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lays_out_bars_with_whiskers() {
        let bars = [
            Bar {
                label: "rarest-first".to_owned(),
                value: 0.8,
                interval: Some((0.7, 0.9)),
                note: Some("20 seeds".to_owned()),
                color: Color::Series(0),
            },
            Bar {
                label: "random".to_owned(),
                value: -0.2,
                interval: None,
                note: None,
                color: Color::Series(1),
            },
        ];
        let plot = plot(&bars, Unit::Fraction, Layout::new(400.0, 200.0));
        let (a, b) = (&plot.bars[0], &plot.bars[1]);
        let zero = plot.y.map(0.0);
        assert!(
            (a.top + a.height - zero).abs() < 1e-9,
            "positive bars stand on zero"
        );
        assert!((b.top - zero).abs() < 1e-9, "negative bars hang from zero");
        let (lo, hi) = a.whisker.unwrap();
        assert!(hi < a.top && a.top < lo);
        assert!(a.center() < b.center());
        assert_eq!(
            a.tooltip,
            ["rarest-first", "80.0% (70.0%–90.0%)", "20 seeds"]
        );
        assert_eq!(plot.y_ticks.first().unwrap().label, "-50%");
    }

    #[test]
    fn shortens_labels() {
        assert_eq!(shorten("tit-for-tat", 20), "tit-for-tat");
        assert_eq!(shorten("tit-for-tat", 6), "tit-f…");
    }
}
