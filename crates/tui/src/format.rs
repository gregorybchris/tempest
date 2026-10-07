//! Numbers as the TUI shows them: compact, so columns stay narrow.

pub use tempest_api::units::{bytes, limit, limit_input, parse_limit};
pub use tempest_api::words::{count, duration as duration_words};

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

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn rates_are_bytes_or_a_dash() {
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

    proptest! {
        #[test]
        fn durations_fit_six_characters_below_a_hundred_days(s in 0u64..8_640_000) {
            prop_assert!(duration(s).len() <= 6, "{}", duration(s));
        }
    }
}
