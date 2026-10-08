//! Durations in settings, written as a number and a unit: `"250ms"`, `"10s"`, `"1.5m"`, `"2h"`.
//!
//! A bare number is read as seconds.

use std::fmt;
use std::time::Duration;

use serde::de::{self, Visitor};
use serde::{Deserializer, Serializer};

const UNITS: [(&str, u64); 6] = [
    ("h", 3_600_000_000_000),
    ("m", 60_000_000_000),
    ("s", 1_000_000_000),
    ("ms", 1_000_000),
    ("us", 1_000),
    ("ns", 1),
];

/// Writes `duration` in the largest unit that divides it exactly.
pub fn format(duration: Duration) -> String {
    let nanos = duration.as_nanos();
    if nanos == 0 {
        return "0s".to_owned();
    }
    let (unit, size) = UNITS
        .iter()
        .find(|(_, size)| nanos.is_multiple_of(u128::from(*size)))
        .expect("nanoseconds divide everything");
    format!("{}{unit}", nanos / u128::from(*size))
}

/// Reads a duration written as a non-negative number and a unit.
pub fn parse(text: &str) -> Result<Duration, String> {
    let text = text.trim();
    let split = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let size = UNITS
        .iter()
        .find(|(name, _)| *name == unit.trim())
        .map(|(_, size)| *size)
        .ok_or_else(|| format!("{text:?} needs a unit: h, m, s, ms, us or ns"))?;
    let not_number = || format!("{text:?} does not start with a number");
    let out_of_range = || format!("{text:?} is out of range");
    if let Ok(whole) = number.parse::<u64>() {
        // Exact for whole numbers, which floating point would round when large.
        return whole
            .checked_mul(size)
            .map(Duration::from_nanos)
            .ok_or_else(out_of_range);
    }
    let value: f64 = number.parse().map_err(|_| not_number())?;
    from_nanos_f64(value * size as f64).ok_or_else(out_of_range)
}

fn from_nanos_f64(nanos: f64) -> Option<Duration> {
    (nanos.is_finite() && nanos >= 0.0 && nanos < u64::MAX as f64)
        .then(|| Duration::from_nanos(nanos.round() as u64))
}

pub fn serialize<S: Serializer>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&format(*duration))
}

pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Duration, D::Error> {
    struct DurationVisitor;

    impl Visitor<'_> for DurationVisitor {
        type Value = Duration;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a duration such as \"10s\" or \"250ms\", or a number of seconds")
        }

        fn visit_str<E: de::Error>(self, text: &str) -> Result<Duration, E> {
            parse(text).map_err(E::custom)
        }

        fn visit_u64<E: de::Error>(self, seconds: u64) -> Result<Duration, E> {
            Ok(Duration::from_secs(seconds))
        }

        fn visit_i64<E: de::Error>(self, seconds: i64) -> Result<Duration, E> {
            u64::try_from(seconds)
                .map(Duration::from_secs)
                .map_err(|_| E::custom("a duration can't be negative"))
        }

        fn visit_f64<E: de::Error>(self, seconds: f64) -> Result<Duration, E> {
            from_nanos_f64(seconds * 1e9).ok_or_else(|| E::custom("duration out of range"))
        }
    }

    deserializer.deserialize_any(DurationVisitor)
}

/// `Option<Duration>` as a duration or `null`, for a setting that can be off.
pub mod optional {
    use serde::{Deserialize, Deserializer, Serializer};

    use super::Duration;

    pub fn serialize<S: Serializer>(
        duration: &Option<Duration>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match duration {
            Some(duration) => super::serialize(duration, serializer),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Duration>, D::Error> {
        #[derive(Deserialize)]
        struct Wrapped(#[serde(with = "crate::config::duration")] Duration);
        let wrapped = Option::<Wrapped>::deserialize(deserializer)?;
        Ok(wrapped.map(|Wrapped(duration)| duration))
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn parses_units_and_fractions() {
        assert_eq!(parse("250ms"), Ok(Duration::from_millis(250)));
        assert_eq!(parse("1.5m"), Ok(Duration::from_secs(90)));
        assert_eq!(parse("2h"), Ok(Duration::from_secs(7200)));
        assert_eq!(parse(" 3 s "), Ok(Duration::from_secs(3)));
        assert!(parse("10").is_err());
        assert!(parse("s").is_err());
        assert!(parse("-1s").is_err());
        assert!(parse("1e999s").is_err());
    }

    #[test]
    fn formats_in_the_largest_exact_unit() {
        assert_eq!(format(Duration::from_secs(60)), "1m");
        assert_eq!(format(Duration::from_millis(1500)), "1500ms");
        assert_eq!(format(Duration::ZERO), "0s");
    }

    proptest! {
        #[test]
        fn format_round_trips(nanos in 0..u64::MAX / 2) {
            let duration = Duration::from_nanos(nanos);
            prop_assert_eq!(parse(&format(duration)), Ok(duration));
        }
    }
}
