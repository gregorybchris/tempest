//! Progress output: elapsed times and a status line that redraws in place on a terminal.

use std::io::{IsTerminal, Write, stderr};
use std::time::Duration;

use unicode_width::UnicodeWidthChar;

/// `elapsed` as `1.5 s`, `2m 05s` or `1h 02m`.
pub fn duration(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    match seconds {
        0..60 => format!("{:.1} s", elapsed.as_secs_f64()),
        60..3600 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h {:02}m", seconds / 3600, seconds / 60 % 60),
    }
}

/// A line on stderr that each update replaces. It shows nothing unless stderr is a terminal
/// that can move the cursor, and it is cut to the terminal's width: a line that wrapped would
/// leave a copy of itself behind at every update.
pub struct StatusLine {
    enabled: bool,
    shown: bool,
}

impl StatusLine {
    pub fn new() -> Self {
        Self {
            enabled: stderr().is_terminal()
                && std::env::var("TERM").ok().is_none_or(|t| t != "dumb"),
            shown: false,
        }
    }

    /// Replaces the line with `text`.
    pub fn show(&mut self, text: &str) {
        if self.enabled {
            // Return to the line's start, write, and clear whatever the last text left over.
            let width = ratatui::crossterm::terminal::size().map_or(80, |(columns, _)| columns);
            let text = fit(text, usize::from(width).saturating_sub(1));
            let _ = write!(stderr(), "\r{text}\x1b[K");
            let _ = stderr().flush();
            self.shown = true;
        }
    }

    /// Removes the line, so other output starts on a clean line.
    pub fn clear(&mut self) {
        if self.shown {
            let _ = write!(stderr(), "\r\x1b[K");
            let _ = stderr().flush();
            self.shown = false;
        }
    }
}

/// `text` cut to `width` columns, ending in `…` if it was cut.
fn fit(text: &str, width: usize) -> String {
    let columns = |text: &str| text.chars().map(|c| c.width().unwrap_or(0)).sum::<usize>();
    if columns(text) <= width {
        return text.to_owned();
    }
    let mut fitted = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        fitted.push(c);
        used += w;
    }
    if width > 0 {
        fitted.push('…');
    }
    fitted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_fit_the_terminal() {
        assert_eq!(fit("short", 10), "short");
        assert_eq!(fit("much too long", 8), "much to…");
        assert_eq!(fit("日本語のファイル", 7), "日本語…");
        assert_eq!(fit("abc", 0), "");
    }

    #[test]
    fn durations_grow_coarser() {
        assert_eq!(duration(Duration::from_millis(1500)), "1.5 s");
        assert_eq!(duration(Duration::from_secs(125)), "2m 05s");
        assert_eq!(duration(Duration::from_secs(3720)), "1h 02m");
    }
}
