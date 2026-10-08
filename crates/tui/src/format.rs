//! Numbers as the TUI shows them: compact, so columns stay narrow.

pub use tempest_api::units::{
    bytes, clock, date, duration, limit, limit_input, parse_limit, percent,
};
pub use tempest_api::words::{count, duration as duration_words};

/// A rate in bytes per second, or a dash for none.
pub fn rate(bytes_per_second: u64) -> String {
    match bytes_per_second {
        0 => "–".to_owned(),
        rate => bytes(rate),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_are_bytes_or_a_dash() {
        assert_eq!(rate(0), "–");
        assert_eq!(rate(9_800_000), "9.8M");
    }

    #[test]
    fn thousands_are_separated() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(256_348), "256,348");
        assert_eq!(thousands(12_345_678), "12,345,678");
    }
}
