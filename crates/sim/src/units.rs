//! Quantities as scenario files write them: sizes (`4MiB`), rates (`20Mbps`), fractions (`0.5%`)
//! and durations (`10s`).
//!
//! Each type deserializes from a string with a unit, or from a bare number in the base unit.

use std::fmt;
use std::time::Duration;

use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};
use tempest_core::config::{format_duration, parse_duration};

/// A number of bytes, written as `16384`, `64KiB`, `4MiB` or `1.5GB`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct ByteSize(pub u64);

/// A data rate in bytes per second, written in bits (`20Mbps`) or bytes (`2MB/s`), or
/// `unlimited`.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Rate(f64);

/// A fraction from 0 to 1, written as `0.5%` or `0.005`.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct Fraction(f64);

const SIZE_UNITS: [(&str, f64); 11] = [
    ("B", 1.0),
    ("kB", 1e3),
    ("KB", 1e3),
    ("MB", 1e6),
    ("GB", 1e9),
    ("TB", 1e12),
    ("KiB", 1024.0),
    ("MiB", 1024.0 * 1024.0),
    ("GiB", 1024.0 * 1024.0 * 1024.0),
    ("TiB", 1024.0 * 1024.0 * 1024.0 * 1024.0),
    ("", 1.0),
];

const BIT_RATE_UNITS: [(&str, f64); 4] = [
    ("bps", 1.0 / 8.0),
    ("kbps", 1e3 / 8.0),
    ("Mbps", 1e6 / 8.0),
    ("Gbps", 1e9 / 8.0),
];

/// Splits `text` into its leading number and the unit after it.
fn split_number(text: &str) -> Result<(f64, &str), String> {
    let text = text.trim();
    let split = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let value: f64 = number
        .parse()
        .map_err(|_| format!("{text:?} does not start with a number"))?;
    Ok((value, unit.trim()))
}

impl ByteSize {
    /// Reads a size such as `4MiB`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let (value, unit) = split_number(text)?;
        let scale = SIZE_UNITS
            .iter()
            .find(|(name, _)| *name == unit)
            .map(|(_, scale)| *scale)
            .ok_or_else(|| {
                format!("{text:?} has an unknown unit: use B, kB, MB, GB, TB, KiB, MiB, GiB or TiB")
            })?;
        let bytes = value * scale;
        if bytes.fract() != 0.0 || bytes >= u64::MAX as f64 {
            return Err(format!("{text:?} is not a whole number of bytes"));
        }
        Ok(Self(bytes as u64))
    }
}

impl Rate {
    /// No limit.
    pub const UNLIMITED: Self = Self(f64::INFINITY);

    /// A rate of `bytes_per_second`, which must be above 0.
    pub fn bytes_per_second(bytes_per_second: f64) -> Self {
        assert!(bytes_per_second > 0.0, "rates are positive");
        Self(bytes_per_second)
    }

    /// The rate in bytes per second, infinite if unlimited.
    pub fn get(self) -> f64 {
        self.0
    }

    /// Reads a rate such as `20Mbps`, `2MB/s` or `unlimited`.
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.trim() == "unlimited" {
            return Ok(Self::UNLIMITED);
        }
        let (value, unit) = split_number(text)?;
        let scale = if let Some(bytes) = unit.strip_suffix("/s") {
            SIZE_UNITS
                .iter()
                .find(|(name, _)| !name.is_empty() && *name == bytes)
                .map(|(_, scale)| *scale)
        } else {
            BIT_RATE_UNITS
                .iter()
                .find(|(name, _)| *name == unit)
                .map(|(_, scale)| *scale)
        };
        let scale = scale.ok_or_else(|| {
            format!("{text:?} needs a unit such as bps, kbps, Mbps, Gbps, kB/s, MB/s or MiB/s")
        })?;
        let rate = value * scale;
        if !(rate.is_finite() && rate > 0.0) {
            return Err(format!("{text:?} is not a rate above 0"));
        }
        Ok(Self(rate))
    }
}

impl Default for Rate {
    fn default() -> Self {
        Self::UNLIMITED
    }
}

impl fmt::Display for Rate {
    /// In megabits per second, the unit links are usually sold in.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_infinite() {
            f.write_str("unlimited")
        } else {
            write!(f, "{}Mbps", self.0 * 8.0 / 1e6)
        }
    }
}

impl Fraction {
    /// The fraction as a number from 0 to 1.
    pub fn get(self) -> f64 {
        self.0
    }

    /// Wraps `value`, which must be from 0 to 1.
    pub fn new(value: f64) -> Result<Self, String> {
        if (0.0..=1.0).contains(&value) {
            Ok(Self(value))
        } else {
            Err(format!("{value} is not a fraction from 0 to 1"))
        }
    }

    /// Reads a fraction such as `0.5%` or `0.005`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let (value, unit) = split_number(text)?;
        match unit {
            "%" => Self::new(value / 100.0),
            "" => Self::new(value),
            _ => Err(format!("{text:?} is not a fraction such as 0.5% or 0.005")),
        }
    }
}

/// A visitor for a quantity written as a string with a unit or as a bare number.
struct Quantity<T> {
    expecting: &'static str,
    parse: fn(&str) -> Result<T, String>,
    number: fn(f64) -> Result<T, String>,
}

impl<T> Visitor<'_> for Quantity<T> {
    type Value = T;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.expecting)
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<T, E> {
        (self.parse)(text).map_err(E::custom)
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<T, E> {
        (self.number)(value as f64).map_err(E::custom)
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<T, E> {
        (self.number)(value as f64).map_err(E::custom)
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<T, E> {
        (self.number)(value).map_err(E::custom)
    }
}

impl<'de> Deserialize<'de> for ByteSize {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(Quantity {
            expecting: "a size such as 4MiB, or a number of bytes",
            parse: Self::parse,
            number: |value| {
                if value >= 0.0 && value.fract() == 0.0 && value < u64::MAX as f64 {
                    Ok(Self(value as u64))
                } else {
                    Err(format!("{value} is not a whole number of bytes"))
                }
            },
        })
    }
}

impl<'de> Deserialize<'de> for Rate {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(Quantity {
            expecting: "a rate such as 20Mbps or 2MB/s, a number of bytes per second, or unlimited",
            parse: Self::parse,
            number: |value| {
                if value.is_finite() && value > 0.0 {
                    Ok(Self(value))
                } else {
                    Err(format!("{value} is not a rate above 0"))
                }
            },
        })
    }
}

impl<'de> Deserialize<'de> for Fraction {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(Quantity {
            expecting: "a fraction such as 0.5% or 0.005",
            parse: Self::parse,
            number: Self::new,
        })
    }
}

impl Serialize for ByteSize {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(self.0)
    }
}

impl Serialize for Rate {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl Serialize for Fraction {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64(self.0)
    }
}

/// Serde helpers for durations written as `"10s"` or a bare number of seconds.
pub mod duration {
    use super::*;

    fn from_seconds(value: f64) -> Result<Duration, String> {
        Duration::try_from_secs_f64(value).map_err(|_| format!("{value} is not a duration"))
    }

    /// Reads a duration.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Duration, D::Error> {
        deserializer.deserialize_any(Quantity {
            expecting: "a duration such as 10s or 250ms, or a number of seconds",
            parse: parse_duration,
            number: from_seconds,
        })
    }

    /// Writes a duration with its unit.
    pub fn serialize<S: Serializer>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format_duration(*duration))
    }

    /// Serde helpers for optional durations.
    pub mod option {
        use super::*;

        /// Reads an optional duration.
        pub fn deserialize<'de, D: Deserializer<'de>>(
            deserializer: D,
        ) -> Result<Option<Duration>, D::Error> {
            super::deserialize(deserializer).map(Some)
        }

        /// Writes an optional duration.
        pub fn serialize<S: Serializer>(
            duration: &Option<Duration>,
            serializer: S,
        ) -> Result<S::Ok, S::Error> {
            match duration {
                Some(duration) => super::serialize(duration, serializer),
                None => serializer.serialize_none(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(ByteSize::parse("16384"), Ok(ByteSize(16384)));
        assert_eq!(ByteSize::parse("4MiB"), Ok(ByteSize(4 << 20)));
        assert_eq!(ByteSize::parse("1.5 GB"), Ok(ByteSize(1_500_000_000)));
        assert_eq!(ByteSize::parse("64KiB"), Ok(ByteSize(65536)));
        assert!(ByteSize::parse("4 MiBs").is_err());
        assert!(ByteSize::parse("0.5B").is_err());
        assert!(ByteSize::parse("MiB").is_err());
    }

    #[test]
    fn rates() {
        assert_eq!(Rate::parse("8bps").unwrap().get(), 1.0);
        assert_eq!(Rate::parse("20Mbps").unwrap().get(), 2.5e6);
        assert_eq!(Rate::parse("2MB/s").unwrap().get(), 2e6);
        assert_eq!(Rate::parse("1MiB/s").unwrap().get(), 1048576.0);
        assert_eq!(Rate::parse("unlimited").unwrap(), Rate::UNLIMITED);
        assert!(Rate::parse("0Mbps").is_err());
        assert!(Rate::parse("20Mb").is_err());
        assert!(Rate::parse("20/s").is_err());
        assert_eq!(Rate::parse("20Mbps").unwrap().to_string(), "20Mbps");
    }

    #[test]
    fn fractions() {
        assert_eq!(Fraction::parse("0.5%").unwrap().get(), 0.005);
        assert_eq!(Fraction::parse("0.25").unwrap().get(), 0.25);
        assert_eq!(Fraction::parse("100%").unwrap().get(), 1.0);
        assert!(Fraction::parse("101%").is_err());
        assert!(Fraction::parse("-1%").is_err());
        assert!(Fraction::parse("5 percent").is_err());
    }

    proptest! {
        #[test]
        fn whole_sizes_read_back(bytes in 0u64..1 << 50) {
            prop_assert_eq!(ByteSize::parse(&bytes.to_string()), Ok(ByteSize(bytes)));
            prop_assert_eq!(ByteSize::parse(&format!("{bytes}KiB")), Ok(ByteSize(bytes * 1024)));
        }

        #[test]
        fn bit_rates_are_an_eighth_of_byte_rates(n in 1u32..1_000_000) {
            let bits = Rate::parse(&format!("{n}kbps")).unwrap().get();
            let bytes = Rate::parse(&format!("{n}kB/s")).unwrap().get();
            prop_assert!((bits * 8.0 - bytes).abs() < 1e-6 * bytes);
        }

        #[test]
        fn parsing_never_panics(text in ".{0,12}") {
            let _ = ByteSize::parse(&text);
            let _ = Rate::parse(&text);
            let _ = Fraction::parse(&text);
        }
    }
}
