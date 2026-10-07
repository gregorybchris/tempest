//! Display formatting.

/// Formats a byte count with decimal units, e.g. `6.40 GB`.
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else if value >= 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

/// Formats a rate in bytes per second, e.g. `84.2 MB/s`. Zero is `—`.
pub fn rate(bytes_per_sec: f64) -> String {
    if bytes_per_sec < 1.0 {
        return "—".to_string();
    }
    format!("{}/s", bytes(bytes_per_sec as u64))
}

/// Formats a duration in seconds, e.g. `3m 12s`, `2h 05m`.
pub fn duration(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}h {m:02}m")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}

/// Formats a fraction in 0..=1 as a percentage, e.g. `63.4%`.
pub fn percent(fraction: f64) -> String {
    format!("{:.1}%", fraction * 100.0)
}

/// Shortens a 64-character hex ID to its first 8 characters.
pub fn short_id(hex: &str) -> &str {
    &hex[..8.min(hex.len())]
}

/// Formats a chunk index with thousands separators, e.g. `41,216`.
pub fn count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Formats a time offset relative to now, e.g. `-4m`, `now`.
pub fn ago(offset_ms: i64) -> String {
    let secs = (-offset_ms).max(0) / 1000;
    if secs == 0 {
        "now".to_string()
    } else if secs % 60 == 0 {
        format!("-{}m", secs / 60)
    } else {
        format!("-{secs}s")
    }
}

/// Formats a simulation time as a clock, e.g. `14:03:27`. The simulation
/// starts at 14:00:00.
pub fn clock(t_ms: u64) -> String {
    let total = 14 * 3600 + t_ms / 1000;
    format!(
        "{:02}:{:02}:{:02}",
        (total / 3600) % 24,
        (total / 60) % 60,
        total % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(bytes(6_400_000_000), "6.40 GB");
        assert_eq!(bytes(48_000_000_000), "48.0 GB");
        assert_eq!(rate(84_200_000.0), "84.2 MB/s");
        assert_eq!(duration(192), "3m 12s");
        assert_eq!(count(41216), "41,216");
        assert_eq!(ago(-240_000), "-4m");
        assert_eq!(clock(207_000), "14:03:27");
    }
}
