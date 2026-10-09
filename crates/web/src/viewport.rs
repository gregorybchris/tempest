//! Which of the transfers list's rows to draw. Rows have one fixed height (`web.css`), so the
//! rows in view follow from the page's scroll alone: the list draws those and a few either
//! side, between two spacers that stand in for the rest. Ten thousand transfers draw a screenful
//! of rows, and scrolling only moves the window.

use std::ops::Range;

/// Rows drawn beyond each edge of the view, so a fast scroll or a short jump shows no gap.
const OVERSCAN: usize = 10;

/// The row height assumed until a row is measured, in pixels.
pub const ROW_HEIGHT: f64 = 38.0;

/// Where the list's body is on the screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// The body's top, in pixels from the top of the browser's viewport: negative once it has
    /// scrolled above it.
    pub top: f64,
    /// The viewport's height.
    pub height: f64,
    /// One row's height.
    pub row: f64,
}

impl Default for View {
    fn default() -> Self {
        Self {
            top: 0.0,
            height: 1080.0,
            row: ROW_HEIGHT,
        }
    }
}

impl View {
    /// The rows to draw out of `rows`: those in view and [`OVERSCAN`] either side.
    pub fn rows(&self, rows: usize) -> Range<usize> {
        let row = self.row.max(1.0);
        let first = ((-self.top).max(0.0) / row).floor() as usize;
        let end = ((self.height - self.top).max(0.0) / row).ceil() as usize;
        let end = end.saturating_add(OVERSCAN).min(rows);
        first.saturating_sub(OVERSCAN).min(end)..end
    }

    /// How far to scroll the page down, negative for up, so that row `index` shows between
    /// `top` and `bottom`, the edges of what the page's fixed bars leave of the viewport; 0 when
    /// it already does.
    pub fn scroll_to(&self, index: usize, top: f64, bottom: f64) -> f64 {
        let start = self.top + index as f64 * self.row;
        let end = start + self.row;
        if start < top {
            start - top
        } else if end > bottom {
            (end - bottom).min(start - top)
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn view(top: f64) -> View {
        View {
            top,
            height: 800.0,
            row: 40.0,
        }
    }

    #[test]
    fn only_the_rows_in_view_and_a_margin_are_drawn() {
        // The list starts 200 pixels down: 15 rows show, and 10 more below.
        assert_eq!(view(200.0).rows(10_000), 0..25);
        // Scrolled 4,000 pixels into the list: rows 100 to 120 show.
        assert_eq!(view(-4000.0).rows(10_000), 90..130);
        assert_eq!(view(-4000.0).rows(105), 90..105);
        assert_eq!(view(-4000.0).rows(50), 50..50);
        assert_eq!(view(0.0).rows(0), 0..0);
        // A list below the fold draws only its margin.
        assert_eq!(view(900.0).rows(100), 0..10);
    }

    #[test]
    fn the_page_scrolls_just_enough_to_show_the_row() {
        let v = view(-4000.0);
        // Row 105 is at 200..240 on the screen.
        assert_eq!(v.scroll_to(105, 60.0, 760.0), 0.0);
        // Row 100 is at 0..40, under a 60-pixel bar: up by 60.
        assert_eq!(v.scroll_to(100, 60.0, 760.0), -60.0);
        // Row 120 is at 800..840, past the bottom at 760: down by 80.
        assert_eq!(v.scroll_to(120, 60.0, 760.0), 80.0);
        // Far away rows come in at the nearest edge.
        assert_eq!(v.scroll_to(0, 60.0, 760.0), -4060.0);
        assert_eq!(v.scroll_to(9999, 60.0, 760.0), 395_240.0);
    }

    proptest! {
        #[test]
        fn drawn_rows_cover_the_view(top in -1e6f64..2e3, height in 1f64..3e3, rows in 0usize..50_000) {
            let v = View { top, height, row: 38.0 };
            let drawn = v.rows(rows);
            prop_assert!(drawn.start <= drawn.end && drawn.end <= rows);
            prop_assert!(drawn.len() <= (height / 38.0).ceil() as usize + 2 * OVERSCAN + 1);
            for i in 0..rows.min(200_000) {
                let start = top + i as f64 * 38.0;
                if start + 38.0 > 0.0 && start < height {
                    prop_assert!(drawn.contains(&i), "row {i} shows but isn't drawn");
                }
            }
        }
    }
}
