//! Progress output: byte counts and a status line that redraws in place on a terminal.

use std::io::{IsTerminal, Write, stderr};
use std::time::Duration;

/// `bytes` in binary units with one decimal, such as `1.5 MiB`.
pub fn bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// `elapsed` as `1.5 s`, `2m 05s` or `1h 02m`.
pub fn duration(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    match seconds {
        0..60 => format!("{:.1} s", elapsed.as_secs_f64()),
        60..3600 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h {:02}m", seconds / 3600, seconds / 60 % 60),
    }
}

/// The number of peers connected, and how many of them through a relay.
pub fn peers(connections: usize, relayed: usize) -> String {
    let noun = if connections == 1 { "peer" } else { "peers" };
    match relayed {
        0 => format!("{connections} {noun}"),
        relayed => format!("{connections} {noun} ({relayed} through relays)"),
    }
}

/// A line on stderr that each update replaces. It shows nothing unless stderr is a terminal.
pub struct StatusLine {
    enabled: bool,
    shown: bool,
}

impl StatusLine {
    pub fn new() -> Self {
        Self {
            enabled: stderr().is_terminal(),
            shown: false,
        }
    }

    /// Replaces the line with `text`.
    pub fn show(&mut self, text: &str) {
        if self.enabled {
            // Return to the line's start, write, and clear whatever the last text left over.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_use_binary_units() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1023), "1023 B");
        assert_eq!(bytes(1024), "1.0 KiB");
        assert_eq!(bytes(1536 * 1024), "1.5 MiB");
        assert_eq!(bytes(u64::MAX), "16.0 EiB");
    }

    #[test]
    fn relayed_peers_are_counted_apart() {
        assert_eq!(peers(1, 0), "1 peer");
        assert_eq!(peers(3, 2), "3 peers (2 through relays)");
    }

    #[test]
    fn durations_grow_coarser() {
        assert_eq!(duration(Duration::from_millis(1500)), "1.5 s");
        assert_eq!(duration(Duration::from_secs(125)), "2m 05s");
        assert_eq!(duration(Duration::from_secs(3720)), "1h 02m");
    }
}
