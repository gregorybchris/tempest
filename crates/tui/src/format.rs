//! Numbers as the TUI shows them: compact, so columns stay narrow.

pub use tempest_api::words::{count, duration as duration_words};

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

/// A moment in milliseconds since the Unix epoch as its date in UTC: `2026-10-07`.
pub fn date(millis: i64) -> String {
    // Howard Hinnant's days-to-civil algorithm, on days since 1970-01-01.
    let days = millis.div_euclid(86_400_000) + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// A speed limit as settings show it: `2.0M/s`, or `none`.
pub fn limit(bytes_per_second: Option<u64>) -> String {
    bytes_per_second.map_or_else(|| "none".to_owned(), |rate| format!("{}/s", bytes(rate)))
}

/// A speed limit as typed: a number of bytes per second with an optional unit (`K`, `M`, `G`,
/// `T`, in thousands as the TUI shows them, with or without `B` and `/s`), or `none` or nothing
/// for no limit.
pub fn parse_limit(text: &str) -> Result<Option<u64>, String> {
    let text = text.trim();
    if text.is_empty() || text.eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    let invalid = || format!("{text:?} is not a speed such as 500K or 2.5M");
    let lower = text.to_ascii_lowercase();
    let unit_part = lower
        .trim_end_matches("/s")
        .trim_end_matches('b')
        .trim_end();
    let (number, scale) = match unit_part.chars().last() {
        Some('k') => (&unit_part[..unit_part.len() - 1], 1e3),
        Some('m') => (&unit_part[..unit_part.len() - 1], 1e6),
        Some('g') => (&unit_part[..unit_part.len() - 1], 1e9),
        Some('t') => (&unit_part[..unit_part.len() - 1], 1e12),
        _ => (unit_part, 1.0),
    };
    let number: f64 = number.trim().parse().map_err(|_| invalid())?;
    let rate = (number * scale).round();
    if !rate.is_finite() || rate < 1.0 || rate > u64::MAX as f64 {
        return Err("A limit is at least 1 byte per second; type none for no limit.".to_owned());
    }
    Ok(Some(rate as u64))
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

    #[test]
    fn dates_are_utc_days() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(1_791_331_200_000), "2026-10-07");
        assert_eq!(date(951_782_400_000), "2000-02-29");
        assert_eq!(date(-1), "1969-12-31");
    }

    #[test]
    fn limits_are_typed_with_units() {
        assert_eq!(parse_limit(""), Ok(None));
        assert_eq!(parse_limit(" None "), Ok(None));
        assert_eq!(parse_limit("500"), Ok(Some(500)));
        assert_eq!(parse_limit("500K"), Ok(Some(500_000)));
        assert_eq!(parse_limit("2.5m"), Ok(Some(2_500_000)));
        assert_eq!(parse_limit("1.5 MB/s"), Ok(Some(1_500_000)));
        assert_eq!(parse_limit("1G"), Ok(Some(1_000_000_000)));
        for bad in ["fast", "0", "-1M", "0.1", "2X", "M"] {
            assert!(parse_limit(bad).is_err(), "{bad}");
        }
        assert_eq!(limit(None), "none");
        assert_eq!(limit(Some(2_000_000)), "2.0M/s");
    }

    proptest! {
        /// A limit reads back as what it shows, to the precision shown.
        #[test]
        fn shown_limits_read_back(rate in 1u64..1_000_000_000_000) {
            let shown = bytes(rate);
            let read = parse_limit(&shown).unwrap().unwrap();
            prop_assert_eq!(bytes(read), shown);
        }

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
