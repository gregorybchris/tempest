//! Byte counts, durations and speed limits as the terminal clients show them and read them
//! back: compact, in thousands, so `2.5M` typed means what `2.5M` shown means.

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

/// A duration in at most two units, so columns stay narrow: `45s`, `3m`, `1h05m`, `2d3h`.
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
    let share = if share.is_nan() {
        0.0
    } else {
        share.clamp(0.0, 1.0)
    };
    let percent = (share * 100.0).floor();
    format!("{percent:.0}%")
}

/// A speed limit as settings show it: `2.0M/s`, or `none`.
pub fn limit(bytes_per_second: Option<u64>) -> String {
    bytes_per_second.map_or_else(|| "none".to_owned(), |rate| format!("{}/s", bytes(rate)))
}

/// A speed limit as typed: a number of bytes per second with an optional unit (`K`, `M`, `G`,
/// `T`, in thousands as clients show them, with or without `B` and `/s`), or `none` or nothing
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

/// A speed limit as a prompt starts with it: exact, in the largest unit that keeps it whole,
/// or empty for none.
pub fn limit_input(limit: Option<u64>) -> String {
    match limit {
        None => String::new(),
        Some(rate) if rate % 1_000_000_000 == 0 => format!("{}G", rate / 1_000_000_000),
        Some(rate) if rate % 1_000_000 == 0 => format!("{}M", rate / 1_000_000),
        Some(rate) if rate % 1000 == 0 => format!("{}K", rate / 1000),
        Some(rate) => rate.to_string(),
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
    }

    #[test]
    fn durations_use_two_units_at_most() {
        assert_eq!(duration(45), "45s");
        assert_eq!(duration(180), "3m");
        assert_eq!(duration(3900), "1h05m");
        assert_eq!(duration(2 * 86_400 + 3 * 3600 + 59), "2d3h");
    }

    #[test]
    fn percentages_reach_100_only_when_complete() {
        assert_eq!(percent(0.999), "99%");
        assert_eq!(percent(1.0), "100%");
        assert_eq!(percent(-1.0), "0%");
        assert_eq!(percent(f64::NAN), "0%");
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

    #[test]
    fn prompts_start_with_the_exact_limit() {
        assert_eq!(limit_input(None), "");
        assert_eq!(limit_input(Some(2_000_000)), "2M");
        assert_eq!(limit_input(Some(1_500_000)), "1500K");
        assert_eq!(limit_input(Some(1234)), "1234");
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
        fn durations_fit_six_characters_below_a_hundred_days(s in 0u64..8_640_000) {
            prop_assert!(duration(s).len() <= 6, "{}", duration(s));
        }

        #[test]
        fn byte_counts_fit_five_characters_below_a_petabyte(n in 0u64..999_000_000_000_000) {
            prop_assert!(bytes(n).chars().count() <= 5, "{}", bytes(n));
        }

        /// A prompt's text reads back as exactly the limit.
        #[test]
        fn prompts_read_back(rate in 1u64..1_000_000_000_000_000 / 1000) {
            prop_assert_eq!(parse_limit(&limit_input(Some(rate))), Ok(Some(rate)));
        }
    }
}
