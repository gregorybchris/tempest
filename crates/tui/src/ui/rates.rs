//! The selected transfer's rates over the last ten minutes, full screen, from the samples the
//! TUI takes once a second.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Axis, Chart, Dataset, GraphType, Paragraph};

use crate::app::App;
use crate::format;

/// The shortest span the time axis covers, in seconds, so that a new chart doesn't stretch a
/// few samples across the screen.
const MIN_SPAN: f64 = 60.0;

/// The lowest top of the rate axis, in bytes per second.
const MIN_TOP: f64 = 1000.0;

/// The time axis, in seconds before now, and the rate axis, in bytes per second, for
/// `samples` taken a second apart, the newest last. The rate axis tops out at a round number
/// above the peak.
pub(crate) fn bounds(samples: &[(u64, u64)]) -> ([f64; 2], [f64; 2]) {
    let span = (samples.len().saturating_sub(1) as f64).max(MIN_SPAN);
    let peak = samples
        .iter()
        .map(|&(down, up)| down.max(up) as f64)
        .fold(0.0, f64::max);
    ([-span, 0.0], [0.0, round_up(peak * 1.1).max(MIN_TOP)])
}

/// The smallest round number at least `value`: a power of ten times 1, 1.2, 1.5, 2, 2.5, 3, 4,
/// 5, 6 or 8.
fn round_up(value: f64) -> f64 {
    if value <= 0.0 {
        return 0.0;
    }
    let power = 10f64.powf(value.log10().floor());
    [1.0, 1.2, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0]
        .into_iter()
        .map(|step| step * power)
        .find(|top| *top >= value)
        .unwrap_or(10.0 * power)
}

/// `seconds` before now as an axis label: `-10m`, `-30s`.
fn ago(seconds: f64) -> String {
    match seconds.round() as u64 {
        seconds if seconds >= 60 => format!("-{}m", (seconds as f64 / 60.0).round()),
        seconds => format!("-{seconds}s"),
    }
}

/// The points of one series: each sample at its time before now.
fn points(samples: &[(u64, u64)], pick: impl Fn((u64, u64)) -> u64) -> Vec<(f64, f64)> {
    let newest = samples.len().saturating_sub(1) as f64;
    samples
        .iter()
        .enumerate()
        .map(|(index, sample)| (index as f64 - newest, pick(*sample) as f64))
        .collect()
}

pub(super) fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let Some(transfer) = app.selected_transfer() else {
        return;
    };
    let [title, _, chart_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(area);
    let samples: Vec<(u64, u64)> = app
        .model
        .history(&transfer.id)
        .map(|history| history.samples().collect())
        .unwrap_or_default();
    let peak = |pick: fn(&(u64, u64)) -> u64| samples.iter().map(pick).max();
    let series = |arrow: &'static str, now: u64, peak: Option<u64>, color| {
        let mut spans = vec![
            Span::styled(format!("  {arrow} "), Style::new().fg(color)),
            Span::styled(format!("{}/s", format::bytes(now)), Style::new().fg(color)),
            Span::styled(" now", Style::new().fg(theme.dim)),
        ];
        if let Some(peak) = peak {
            spans.push(Span::styled(
                format!(", peak {}/s", format::bytes(peak)),
                Style::new().fg(theme.dim),
            ));
        }
        spans
    };
    let mut heading = vec![Span::styled(
        format!(" {}", transfer.name),
        Style::new().fg(theme.fg).add_modifier(Modifier::BOLD),
    )];
    heading.extend(series(
        "↓",
        transfer.download_rate,
        peak(|s| s.0),
        theme.down,
    ));
    heading.extend(series("↑", transfer.upload_rate, peak(|s| s.1), theme.up));
    frame.render_widget(Paragraph::new(Line::from(heading)), title);

    if samples.len() < 2 {
        let [middle] = Layout::vertical([Constraint::Length(1)])
            .flex(Flex::Center)
            .areas(chart_area);
        let line = Line::styled(
            "Collecting samples, one per second.",
            Style::new().fg(theme.dim),
        );
        frame.render_widget(Paragraph::new(line).centered(), middle);
        return;
    }

    let (x, y) = bounds(&samples);
    let down = points(&samples, |(down, _)| down);
    let up = points(&samples, |(_, up)| up);
    let label = |text: String| Span::styled(text, Style::new().fg(theme.dim));
    let rate = |value: f64| format!("{}/s", format::bytes(value as u64));
    let chart = Chart::new(vec![
        Dataset::default()
            .name("↓")
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::new().fg(theme.down))
            .data(&down),
        Dataset::default()
            .name("↑")
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::new().fg(theme.up))
            .data(&up),
    ])
    .style(Style::new().bg(theme.bg))
    .legend_position(None)
    .x_axis(
        Axis::default()
            .bounds(x)
            .style(Style::new().fg(theme.line))
            .labels([
                label(ago(-x[0])),
                label(ago(-x[0] / 2.0)),
                label("now".to_owned()),
            ]),
    )
    .y_axis(
        Axis::default()
            .bounds(y)
            .style(Style::new().fg(theme.line))
            .labels([
                label("0".to_owned()),
                label(rate(y[1] / 2.0)),
                label(rate(y[1])),
            ]),
    );
    frame.render_widget(chart, chart_area);
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn the_axes_cover_the_samples_with_room_above() {
        assert_eq!(bounds(&[]), ([-60.0, 0.0], [0.0, 1000.0]));
        let samples = [(9_800_000, 1_200_000), (8_000_000, 3_000_000)];
        assert_eq!(bounds(&samples), ([-60.0, 0.0], [0.0, 12_000_000.0]));
        let long = vec![(100, 50); 600];
        assert_eq!(bounds(&long).0, [-599.0, 0.0]);
        assert_eq!(round_up(1100.0), 1200.0);
        assert_eq!(round_up(500.0), 500.0);
        assert_eq!(round_up(8001.0), 10_000.0);
        assert_eq!(ago(600.0), "-10m");
        assert_eq!(ago(299.5), "-5m");
        assert_eq!(ago(30.0), "-30s");
    }

    #[test]
    fn points_end_now() {
        assert_eq!(
            points(&[(1, 2), (3, 4), (5, 6)], |(down, _)| down),
            [(-2.0, 1.0), (-1.0, 3.0), (0.0, 5.0)]
        );
    }

    proptest! {
        #[test]
        fn every_point_is_inside_the_axes(samples in prop::collection::vec((0u64..1 << 40, 0u64..1 << 40), 0..700)) {
            let (x, y) = bounds(&samples);
            for (time, rate) in points(&samples, |(down, _)| down).into_iter().chain(points(&samples, |(_, up)| up)) {
                prop_assert!(x[0] <= time && time <= x[1]);
                prop_assert!(y[0] <= rate && rate <= y[1], "{rate} above {}", y[1]);
            }
        }
    }
}
