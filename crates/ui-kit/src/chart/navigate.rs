//! Moving around a line chart without a mouse: stepping the readout from sample to sample, and
//! zooming, panning and pinching the window shown. Windows are in x values; `None` shows all
//! the data.

use std::collections::BTreeSet;

use super::line::{LineData, LinePlot};

/// The narrowest window, as a share of all the data, so zooming in always shows a few samples.
const MIN_SHARE: f64 = 1.0 / 512.0;

/// A move of a chart's readout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// This many samples later, or earlier when negative.
    By(isize),
    /// To the first sample.
    First,
    /// To the last sample.
    Last,
}

impl LineData {
    /// The x of every sample of the series not `hidden`, in order, each once.
    pub fn samples(&self, hidden: &BTreeSet<String>) -> Vec<f64> {
        let mut xs: Vec<f64> = self
            .series
            .iter()
            .filter(|s| !hidden.contains(&s.key))
            .flat_map(|s| s.points.iter().map(|p| p.0))
            .collect();
        xs.sort_by(f64::total_cmp);
        xs.dedup();
        xs
    }
}

/// The sample the readout moves to from the one at `from`, among `samples` in order. Without a
/// readout, a step forward starts at the first sample and a step back at the last.
pub fn step(samples: &[f64], from: Option<f64>, to: Step) -> Option<f64> {
    let last = samples.len().checked_sub(1)?;
    let at = from.map(|x| {
        let after = samples.partition_point(|&s| s < x);
        match after.checked_sub(1) {
            Some(before) if after > last || x - samples[before] < samples[after] - x => before,
            _ => after.min(last),
        }
    });
    let i = match (to, at) {
        (Step::First, _) => 0,
        (Step::Last, _) => last,
        (Step::By(n), None) => {
            if n < 0 {
                last
            } else {
                0
            }
        }
        (Step::By(n), Some(i)) => i.saturating_add_signed(n).min(last),
    };
    Some(samples[i])
}

impl LinePlot {
    /// The window that shows `x`: this one if it does, else this one moved just far enough.
    pub fn showing(&self, x: f64) -> Option<(f64, f64)> {
        let (lo, hi) = self.window;
        let shift = if x < lo {
            x - lo
        } else if x > hi {
            x - hi
        } else {
            0.0
        };
        self.within((lo + shift, hi + shift))
    }

    /// The window zoomed about `center` by `factor`: below 1 zooms in, above 1 out.
    pub fn zoomed_by(&self, center: f64, factor: f64) -> Option<(f64, f64)> {
        let (lo, hi) = self.window;
        let center = center.clamp(lo, hi);
        self.within((
            center - (center - lo) * factor,
            center + (hi - center) * factor,
        ))
    }

    /// The window moved left by `dx` pixels' worth, as a drag to the right moves the data.
    pub fn panned(&self, dx: f64) -> Option<(f64, f64)> {
        let (lo, hi) = self.window;
        let per_pixel = (hi - lo) / self.plot_width();
        self.within((lo - dx * per_pixel, hi - dx * per_pixel))
    }

    /// The window that keeps the x values `anchors`, where two fingers touched, under those
    /// fingers, now at pixels `fingers`.
    pub fn pinched(&self, anchors: (f64, f64), fingers: (f64, f64)) -> Option<(f64, f64)> {
        let span = anchors.1 - anchors.0;
        let apart = fingers.1 - fingers.0;
        if span.abs() < f64::EPSILON || apart.abs() < 1.0 || span.signum() != apart.signum() {
            return self.zoomed.then_some(self.window);
        }
        let per_pixel = span / apart;
        let lo = anchors.0 - (fingers.0 - self.layout.plot_left()) * per_pixel;
        self.within((lo, lo + self.plot_width() * per_pixel))
    }

    /// `window` kept inside the data and no narrower than the narrowest, or `None` once it
    /// shows all of it.
    fn within(&self, window: (f64, f64)) -> Option<(f64, f64)> {
        let (start, end) = self.full;
        let full = end - start;
        let width = (window.1 - window.0).max(full * MIN_SHARE);
        if width >= full {
            return None;
        }
        let center = (window.0 + window.1) / 2.0;
        let lo = (center - width / 2.0).clamp(start, end - width);
        Some((lo, lo + width))
    }

    fn plot_width(&self) -> f64 {
        self.layout.plot_right() - self.layout.plot_left()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::{Color, Layout, Series, XAxis};

    fn data() -> LineData {
        LineData {
            series: vec![
                Series::new(
                    "a",
                    "a",
                    Color::Series(0),
                    (0..=100).map(|i| (i as f64, 1.0)).collect(),
                ),
                Series::new("b", "b", Color::Series(1), vec![(0.5, 1.0), (50.0, 2.0)]),
            ],
            x: XAxis::Number,
            ..LineData::default()
        }
    }

    fn plot(window: Option<(f64, f64)>) -> LinePlot {
        data().plot(Layout::new(572.0, 200.0), window, &BTreeSet::new())
    }

    #[test]
    fn the_readout_steps_through_every_shown_sample() {
        let all = data().samples(&BTreeSet::new());
        assert_eq!(all.len(), 102);
        assert_eq!(all[..3], [0.0, 0.5, 1.0]);
        let only_a = data().samples(&BTreeSet::from(["b".to_owned()]));
        assert_eq!(only_a.len(), 101);

        assert_eq!(step(&all, None, Step::By(1)), Some(0.0));
        assert_eq!(step(&all, None, Step::By(-1)), Some(100.0));
        assert_eq!(step(&all, Some(0.0), Step::By(1)), Some(0.5));
        assert_eq!(step(&all, Some(0.5), Step::By(-1)), Some(0.0));
        assert_eq!(step(&all, Some(0.0), Step::By(-1)), Some(0.0));
        assert_eq!(step(&all, Some(99.0), Step::By(10)), Some(100.0));
        // From between samples, it steps from the nearest.
        assert_eq!(step(&all, Some(9.9), Step::By(1)), Some(11.0));
        assert_eq!(step(&all, Some(250.0), Step::By(-1)), Some(99.0));
        assert_eq!(step(&all, Some(3.0), Step::Last), Some(100.0));
        assert_eq!(step(&all, Some(3.0), Step::First), Some(0.0));
        assert_eq!(step(&[], None, Step::First), None);
    }

    #[test]
    fn zooming_stays_inside_the_data_and_comes_back_out() {
        let all = plot(None);
        assert_eq!(all.full, (0.0, 100.0));
        let window = all.zoomed_by(100.0, 0.5).unwrap();
        assert_eq!(window, (50.0, 100.0));
        let zoomed = plot(Some(window));
        assert_eq!(zoomed.zoomed_by(75.0, 2.0), None);
        // Zooming in keeps the point under the center where it was.
        assert_eq!(zoomed.zoomed_by(60.0, 0.5), Some((55.0, 80.0)));
        // There is a narrowest window.
        let mut window = Some(window);
        for _ in 0..40 {
            window = plot(window).zoomed_by(30.0, 0.5);
        }
        let (lo, hi) = window.unwrap();
        assert!((hi - lo - 100.0 / 512.0).abs() < 1e-9 && lo >= 0.0);
    }

    #[test]
    fn panning_and_stepping_past_the_edge_move_the_window() {
        let zoomed = plot(Some((20.0, 40.0)));
        // The plot is 500 pixels wide, so 25 pixels are one x.
        assert_eq!(zoomed.panned(25.0), Some((19.0, 39.0)));
        assert_eq!(zoomed.panned(-1e6), Some((80.0, 100.0)));
        assert_eq!(zoomed.showing(30.0), Some((20.0, 40.0)));
        assert_eq!(zoomed.showing(41.0), Some((21.0, 41.0)));
        assert_eq!(zoomed.showing(5.0), Some((5.0, 25.0)));
        assert_eq!(plot(None).panned(40.0), None);
    }

    #[test]
    fn pinching_keeps_the_touched_values_under_the_fingers() {
        let all = plot(None);
        let left = all.layout.plot_left();
        // Fingers on 40 and 60, spread to twice as far apart about the middle.
        let (a, b) = (all.x.map(40.0), all.x.map(60.0));
        let (lo, hi) = all.pinched((40.0, 60.0), (a - 50.0, b + 50.0)).unwrap();
        assert!((lo - 25.0).abs() < 1e-9 && (hi - 75.0).abs() < 1e-9);
        let zoomed = plot(Some((lo, hi)));
        assert!((zoomed.x.map(40.0) - (a - 50.0)).abs() < 1e-9);
        // Pinching back in past everything shows all of it.
        assert_eq!(
            zoomed.pinched((40.0, 60.0), (left + 240.0, left + 260.0)),
            None
        );
        // Crossed or touching fingers leave the window be.
        assert_eq!(zoomed.pinched((40.0, 60.0), (b, a)), Some((lo, hi)));
    }
}
