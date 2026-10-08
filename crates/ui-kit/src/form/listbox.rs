//! The logic behind [`Select`](super::Select)'s listbox, apart from the DOM: moving the highlight, matching
//! typed text and placing the popup.

/// How far a page key moves the highlight.
pub const PAGE: usize = 10;
/// How long typed letters keep adding to one query, in milliseconds.
pub const TYPE_AHEAD_MS: f64 = 700.0;
/// The popup's distance from its button, in pixels.
pub const GAP: f64 = 4.0;
/// The least room the popup leaves at the viewport's edges, in pixels.
pub const MARGIN: f64 = 8.0;
/// The tallest the popup grows before it scrolls, in pixels.
pub const MAX_HEIGHT: f64 = 320.0;

/// A move of the highlight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    /// Down one.
    Next,
    /// Up one.
    Previous,
    /// To the first option.
    First,
    /// To the last option.
    Last,
    /// Down a page.
    PageDown,
    /// Up a page.
    PageUp,
}

/// The highlighted option after `to`, from `active`, among `len` options. Moves stop at
/// the ends rather than wrapping, as native selects do.
pub fn moved(active: Option<usize>, len: usize, to: Move) -> Option<usize> {
    let last = len.checked_sub(1)?;
    Some(match (to, active) {
        (Move::First, _) => 0,
        (Move::Last, _) => last,
        (Move::Next, None) | (Move::PageDown, None) => 0,
        (Move::Previous, None) | (Move::PageUp, None) => last,
        (Move::Next, Some(i)) => (i + 1).min(last),
        (Move::Previous, Some(i)) => i.saturating_sub(1).min(last),
        (Move::PageDown, Some(i)) => (i + PAGE).min(last),
        (Move::PageUp, Some(i)) => i.saturating_sub(PAGE).min(last),
    })
}

/// The letters typed in quick succession, which together pick an option.
#[derive(Clone, Debug, Default)]
pub struct TypeAhead {
    query: String,
    at: f64,
}

impl TypeAhead {
    /// Adds `c`, typed at `now` (in milliseconds), and returns the query. A pause longer
    /// than [`TYPE_AHEAD_MS`] starts a new one.
    pub fn push(&mut self, c: char, now: f64) -> &str {
        if !self.active(now) {
            self.query.clear();
        }
        self.query.extend(c.to_lowercase());
        self.at = now;
        &self.query
    }

    /// Whether a query is under way at `now`, so a space adds to it rather than choosing.
    pub fn active(&self, now: f64) -> bool {
        !self.query.is_empty() && now - self.at <= TYPE_AHEAD_MS
    }
}

/// The option whose name starts with `query`, ignoring case, searching from `from` and
/// wrapping around. A query of one letter, or of one letter repeated, moves on to the next
/// match after `from`, so pressing a letter again cycles through the names it starts.
pub fn matching(options: &[(String, String)], query: &str, from: Option<usize>) -> Option<usize> {
    let len = options.len();
    let mut letters = query.chars();
    let first = letters.next()?;
    let repeated = letters.all(|c| c == first);
    let (query, start) = match repeated {
        true => (first.to_string(), from.map_or(0, |i| i + 1)),
        false => (query.to_owned(), from.unwrap_or(0)),
    };
    (0..len)
        .map(|k| (start + k) % len)
        .find(|&i| options[i].1.trim_start().to_lowercase().starts_with(&query))
}

/// A rectangle in viewport pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    /// Its left edge.
    pub left: f64,
    /// Its top edge.
    pub top: f64,
    /// Its width.
    pub width: f64,
    /// Its height.
    pub height: f64,
}

/// Where the popup goes, in viewport pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// Its left edge.
    pub left: f64,
    /// Its top edge.
    pub top: f64,
    /// At least the button's width.
    pub min_width: f64,
    /// The most it may grow before scrolling.
    pub max_height: f64,
    /// Whether it opens above the button.
    pub above: bool,
}

/// Places a popup of natural `size` (width and height) against `button`, inside a
/// `viewport` (width and height): below it, unless it doesn't fit there and there is more
/// room above. It is shifted left to stay on screen.
pub fn place(button: Rect, size: (f64, f64), viewport: (f64, f64)) -> Placement {
    let (width, height) = (size.0.max(button.width), size.1.min(MAX_HEIGHT));
    let below = viewport.1 - (button.top + button.height) - GAP - MARGIN;
    let above_room = button.top - GAP - MARGIN;
    let above = height > below && above_room > below;
    let max_height = if above { above_room } else { below }.clamp(0.0, MAX_HEIGHT);
    let shown = height.min(max_height);
    let top = match above {
        true => button.top - GAP - shown,
        false => button.top + button.height + GAP,
    };
    let left = button
        .left
        .min(viewport.0 - MARGIN - width)
        .max(MARGIN.min(button.left));
    Placement {
        left,
        top,
        min_width: button.width,
        max_height,
        above,
    }
}

/// `value` moved by `by` and kept within `min` and `max`, for a number field's buttons.
pub fn stepped(value: f64, by: f64, min: f64, max: f64) -> f64 {
    (value + by).clamp(min, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(names: &[&str]) -> Vec<(String, String)> {
        names
            .iter()
            .map(|n| (n.to_lowercase(), (*n).to_owned()))
            .collect()
    }

    #[test]
    fn moves_stop_at_the_ends() {
        assert_eq!(moved(Some(0), 3, Move::Next), Some(1));
        assert_eq!(moved(Some(2), 3, Move::Next), Some(2));
        assert_eq!(moved(Some(0), 3, Move::Previous), Some(0));
        assert_eq!(moved(None, 3, Move::Next), Some(0));
        assert_eq!(moved(None, 3, Move::Previous), Some(2));
        assert_eq!(moved(Some(1), 3, Move::First), Some(0));
        assert_eq!(moved(Some(1), 3, Move::Last), Some(2));
        assert_eq!(moved(Some(3), 30, Move::PageDown), Some(13));
        assert_eq!(moved(Some(25), 30, Move::PageDown), Some(29));
        assert_eq!(moved(Some(5), 30, Move::PageUp), Some(0));
        assert_eq!(moved(Some(1), 0, Move::Next), None);
        // An index left over from a longer list comes back inside.
        assert_eq!(moved(Some(9), 3, Move::Previous), Some(2));
    }

    #[test]
    fn typing_finds_names_and_repeats_cycle() {
        let options = named(&["Audio", "Archive", "Books", "audiobooks"]);
        assert_eq!(matching(&options, "b", None), Some(2));
        assert_eq!(matching(&options, "au", None), Some(0));
        assert_eq!(matching(&options, "au", Some(1)), Some(3));
        // One letter, typed again, goes on to the next name it starts.
        assert_eq!(matching(&options, "a", Some(0)), Some(1));
        assert_eq!(matching(&options, "aa", Some(1)), Some(3));
        assert_eq!(matching(&options, "a", Some(3)), Some(0));
        assert_eq!(matching(&options, "z", Some(0)), None);
        assert_eq!(matching(&options, "", Some(0)), None);
        assert_eq!(matching(&[], "a", None), None);
    }

    #[test]
    fn type_ahead_starts_over_after_a_pause() {
        let mut typed = TypeAhead::default();
        assert!(!typed.active(0.0));
        assert_eq!(typed.push('A', 0.0), "a");
        assert_eq!(typed.push('u', 300.0), "au");
        assert!(typed.active(900.0));
        assert!(!typed.active(1100.0));
        assert_eq!(typed.push('b', 1100.0), "b");
    }

    #[test]
    fn popups_open_below_unless_above_has_more_room() {
        let viewport = (1000.0, 800.0);
        let button = |top| Rect {
            left: 100.0,
            top,
            width: 160.0,
            height: 30.0,
        };
        let below = place(button(100.0), (120.0, 200.0), viewport);
        assert!(!below.above);
        assert_eq!(below.top, 134.0);
        assert_eq!(below.min_width, 160.0);
        assert_eq!(below.max_height, MAX_HEIGHT);

        // Near the bottom it flips, ending just above the button.
        let above = place(button(700.0), (120.0, 200.0), viewport);
        assert!(above.above);
        assert_eq!(above.top, 700.0 - GAP - 200.0);

        // Too tall for either side, it takes the roomier one and scrolls.
        let tall = place(button(300.0), (120.0, 1000.0), (1000.0, 500.0));
        assert!(tall.above);
        assert_eq!(tall.max_height, 300.0 - GAP - MARGIN);
        assert_eq!(tall.top, MARGIN);

        // It stays inside the right edge.
        let wide = place(
            Rect {
                left: 900.0,
                ..button(100.0)
            },
            (300.0, 100.0),
            viewport,
        );
        assert_eq!(wide.left, 1000.0 - MARGIN - 300.0);
    }

    #[test]
    fn steps_stay_in_range() {
        assert_eq!(stepped(4.0, 1.0, 0.0, 5.0), 5.0);
        assert_eq!(stepped(5.0, 1.0, 0.0, 5.0), 5.0);
        assert_eq!(stepped(0.5, -1.0, 0.0, 5.0), 0.0);
    }
}
