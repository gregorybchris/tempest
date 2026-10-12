//! How numbers read in the web UIs: sizes, rates, durations and counts.

/// A byte count in decimal units, with three significant figures from a kilobyte up: `812 B`,
/// `6.40 GB`, `48.0 GB`.
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 7] = ["B", "KB", "MB", "GB", "TB", "PB", "EB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 999.5 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{} {}", three_figures(value), UNITS[unit])
    }
}

/// A rate in bytes per second, such as `84.2 MB/s`. Under one byte a second reads `–`.
pub fn rate(bytes_per_sec: f64) -> String {
    if bytes_per_sec.is_nan() || bytes_per_sec < 1.0 {
        return "–".to_owned();
    }
    format!("{}/s", bytes(bytes_per_sec as u64))
}

/// A duration in whole seconds, in its two largest units: `45s`, `3m 12s`, `2h 05m`, `4d 03h`.
pub fn duration(secs: u64) -> String {
    let (d, h, m, s) = (secs / 86_400, secs / 3600 % 24, secs / 60 % 60, secs % 60);
    if d > 0 {
        format!("{d}d {h:02}h")
    } else if h > 0 {
        format!("{h}h {m:02}m")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}

/// A fraction as a percentage with one decimal, such as `63.4%`. It never rounds up to `100.0%`
/// until the fraction is whole, so an unfinished transfer never looks done.
pub fn percent(fraction: f64) -> String {
    // The nudge keeps binary rounding, as in 0.29 * 1000 = 289.99…, from losing a tenth.
    let mut tenths = (fraction.clamp(0.0, 1.0) * 1000.0 + 1e-9).floor() / 10.0;
    if fraction < 1.0 {
        tenths = tenths.min(99.9);
    }
    format!("{tenths:.1}%")
}

/// A whole number with thousands separators, such as `41,216`.
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

/// A number in at most three significant figures with a metric suffix, for axes and compact
/// tables: `0.25`, `12.5`, `1.20k`, `48.0M`.
pub fn compact(value: f64) -> String {
    if !value.is_finite() {
        return "–".to_owned();
    }
    let sign = if value < 0.0 { "-" } else { "" };
    let mut v = value.abs();
    if v == 0.0 {
        return "0".to_owned();
    }
    let mut suffix = 0;
    const SUFFIXES: [&str; 5] = ["", "k", "M", "G", "T"];
    while v >= 999.5 && suffix < SUFFIXES.len() - 1 {
        v /= 1000.0;
        suffix += 1;
    }
    let digits = if suffix == 0 && v.fract() == 0.0 {
        format!("{v:.0}")
    } else if v < 0.1 {
        // Two significant figures, so small values don't read as zero.
        let decimals = (1.0 - v.log10().floor()) as usize;
        format!("{v:.decimals$}")
    } else {
        three_figures(v)
    };
    format!("{sign}{digits}{}", SUFFIXES[suffix])
}

/// A time before now, given as a negative offset in milliseconds: `now`, `-30s`, `-4m`,
/// `-1m 30s`.
pub fn ago(offset_ms: i64) -> String {
    let secs = (-offset_ms).max(0) / 1000;
    match (secs / 60, secs % 60) {
        (0, 0) => "now".to_owned(),
        (0, s) => format!("-{s}s"),
        (m, 0) => format!("-{m}m"),
        (m, s) => format!("-{m}m {s}s"),
    }
}

/// A moment in milliseconds since the Unix epoch in the browser's time zone, with the zone:
/// `Jul 11, 2025, 7:18 PM PDT`. Only works in a browser.
pub fn datetime(millis: i64) -> String {
    let options = js_sys::Object::new();
    for (key, value) in [
        ("year", "numeric"),
        ("month", "short"),
        ("day", "numeric"),
        ("hour", "numeric"),
        ("minute", "2-digit"),
        ("timeZoneName", "short"),
    ] {
        let _ = js_sys::Reflect::set(&options, &key.into(), &value.into());
    }
    js_sys::Date::new(&(millis as f64).into())
        .to_locale_string("en-US", &options)
        .into()
}

/// The first eight characters of a hex identifier.
pub fn short_id(hex: &str) -> &str {
    hex.get(..8).unwrap_or(hex)
}

/// `value` in three significant figures, for values from 1 to under 1,000.
fn three_figures(value: f64) -> String {
    if value >= 99.95 {
        format!("{value:.0}")
    } else if value >= 9.995 {
        format!("{value:.1}")
    } else {
        format!("{value:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn formats_examples() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(812), "812 B");
        assert_eq!(bytes(999_499), "999 KB");
        assert_eq!(bytes(999_500), "1.00 MB");
        assert_eq!(bytes(6_400_000_000), "6.40 GB");
        assert_eq!(bytes(48_000_000_000), "48.0 GB");
        assert_eq!(rate(84_200_000.0), "84.2 MB/s");
        assert_eq!(rate(0.0), "–");
        assert_eq!(rate(f64::NAN), "–");
        assert_eq!(duration(45), "45s");
        assert_eq!(duration(192), "3m 12s");
        assert_eq!(duration(7_500), "2h 05m");
        assert_eq!(duration(356_400), "4d 03h");
        assert_eq!(percent(0.6345), "63.4%");
        assert_eq!(percent(0.9999), "99.9%");
        assert_eq!(percent(0.29), "29.0%");
        assert_eq!(percent(1.0), "100.0%");
        assert_eq!(count(41_216), "41,216");
        assert_eq!(count(1_000_000), "1,000,000");
        assert_eq!(compact(0.25), "0.25");
        assert_eq!(compact(40.0), "40");
        assert_eq!(compact(1_200.0), "1.20k");
        assert_eq!(compact(-48_000_000.0), "-48.0M");
        assert_eq!(compact(0.004), "0.0040");
        assert_eq!(compact(0.0005), "0.00050");
        assert_eq!(compact(0.1), "0.10");
        assert_eq!(ago(0), "now");
        assert_eq!(ago(-240_000), "-4m");
        assert_eq!(ago(-90_000), "-1m 30s");
        assert_eq!(short_id("0123456789abcdef"), "01234567");
        assert_eq!(short_id("abc"), "abc");
    }

    proptest! {
        #[test]
        fn bytes_has_at_most_three_figures(n in any::<u64>()) {
            let text = bytes(n);
            let digits = text.chars().filter(char::is_ascii_digit).count();
            prop_assert!(n < 1000 || digits <= 3, "{text}");
        }

        #[test]
        fn count_round_trips(n in any::<u64>()) {
            prop_assert_eq!(count(n).replace(',', "").parse::<u64>().unwrap(), n);
        }

        #[test]
        fn percent_never_overstates(f in 0.0f64..1.0) {
            let shown: f64 = percent(f).trim_end_matches('%').parse().unwrap();
            prop_assert!(shown <= f * 100.0 + 1e-6);
            prop_assert!(shown < 100.0);
        }
    }
}
