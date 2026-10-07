//! Numbers as the TUI shows them: compact, so columns stay narrow.

/// A byte count in at most four characters plus a unit letter: `812M`, `4.2G`, `37K`, `900B`.
pub fn bytes(n: u64) -> String {
    const UNITS: [char; 5] = ['B', 'K', 'M', 'G', 'T'];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 999.5 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    let unit_letter = UNITS[unit];
    if unit == 0 {
        format!("{n}{unit_letter}")
    } else if value < 9.95 {
        format!("{value:.1}{unit_letter}")
    } else {
        format!("{value:.0}{unit_letter}")
    }
}

/// A rate in bytes per second, or a dash for none.
pub fn rate(bytes_per_second: u64) -> String {
    match bytes_per_second {
        0 => "–".to_owned(),
        rate => bytes(rate),
    }
}

/// A duration in at most two units: `45s`, `3m`, `1h05m`, `2d3h`.
pub fn duration(seconds: u64) -> String {
    let (days, hours, minutes) = (seconds / 86_400, seconds / 3600 % 24, seconds / 60 % 60);
    if days > 0 {
        format!("{days}d{hours}h")
    } else if hours > 0 {
        format!("{hours}h{minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{seconds}s")
    }
}

/// A duration as words, rounded: "less than a minute", "3 minutes", "about 2 hours".
pub fn duration_words(seconds: u64) -> String {
    let plural = |n: u64, unit: &str| match n {
        1 => format!("1 {unit}"),
        n => format!("{n} {unit}s"),
    };
    match seconds {
        0..60 => "less than a minute".to_owned(),
        60..3600 => plural(seconds.div_ceil(60), "minute"),
        3600..86_400 => plural((seconds + 1800) / 3600, "hour"),
        _ => plural((seconds + 43_200) / 86_400, "day"),
    }
}

/// A share from 0 to 1 as a whole percentage, never rounding up to 100% before it is complete.
pub fn percent(share: f64) -> String {
    let percent = (share.clamp(0.0, 1.0) * 100.0).floor();
    format!("{percent:.0}%")
}

/// A whole number with commas between thousands: `256,348`.
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut text = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            text.push(',');
        }
        text.push(digit);
    }
    text
}

/// A time on a session's clock, in seconds since it started, to a tenth of a second:
/// `0:07.3`, `12:03.0`, `1:02:03.5`.
pub fn clock(seconds: f64) -> String {
    let tenths = (seconds.max(0.0) * 10.0).floor() as u64;
    let (whole, tenth) = (tenths / 10, tenths % 10);
    let (hours, minutes, seconds) = (whole / 3600, whole / 60 % 60, whole % 60);
    match hours {
        0 => format!("{minutes}:{seconds:02}.{tenth}"),
        hours => format!("{hours}:{minutes:02}:{seconds:02}.{tenth}"),
    }
}

/// `count` of a noun, with an `s` unless it is one: "1 peer", "3 peers".
pub fn count(count: usize, noun: &str) -> String {
    match count {
        1 => format!("1 {noun}"),
        n => format!("{n} {noun}s"),
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn bytes_are_compact() {
        let cases = [
            (0, "0B"),
            (900, "900B"),
            (999, "999B"),
            (1000, "1.0K"),
            (37_400, "37K"),
            (812_000_000, "812M"),
            (999_600_000, "1.0G"),
            (4_200_000_000, "4.2G"),
            (9_960_000_000, "10G"),
            (3_000_000_000_000_000, "3000T"),
        ];
        for (n, text) in cases {
            assert_eq!(bytes(n), text, "{n}");
        }
        assert_eq!(rate(0), "–");
        assert_eq!(rate(9_800_000), "9.8M");
    }

    #[test]
    fn durations_use_two_units_at_most() {
        assert_eq!(duration(45), "45s");
        assert_eq!(duration(180), "3m");
        assert_eq!(duration(3900), "1h05m");
        assert_eq!(duration(2 * 86_400 + 3 * 3600 + 59), "2d3h");
        assert_eq!(duration_words(59), "less than a minute");
        assert_eq!(duration_words(61), "2 minutes");
        assert_eq!(duration_words(3600), "1 hour");
        assert_eq!(duration_words(3 * 86_400), "3 days");
    }

    #[test]
    fn thousands_are_separated() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(256_348), "256,348");
        assert_eq!(thousands(12_345_678), "12,345,678");
    }

    #[test]
    fn clocks_count_from_the_session_start() {
        assert_eq!(clock(7.36), "0:07.3");
        assert_eq!(clock(723.0), "12:03.0");
        assert_eq!(clock(3723.5), "1:02:03.5");
        assert_eq!(clock(-1.0), "0:00.0");
    }

    #[test]
    fn percentages_reach_100_only_when_complete() {
        assert_eq!(percent(0.999), "99%");
        assert_eq!(percent(1.0), "100%");
        assert_eq!(percent(-1.0), "0%");
    }

    proptest! {
        #[test]
        fn byte_counts_fit_five_characters_below_a_petabyte(n in 0u64..999_000_000_000_000) {
            prop_assert!(bytes(n).chars().count() <= 5, "{}", bytes(n));
        }

        #[test]
        fn durations_fit_six_characters_below_a_hundred_days(s in 0u64..8_640_000) {
            prop_assert!(duration(s).len() <= 6, "{}", duration(s));
        }
    }
}
