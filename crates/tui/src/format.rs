//! Numbers as the TUI shows them: compact, so columns stay narrow.

pub use tempest_api::units::{
    bytes, clock, date, duration, limit, limit_input, parse_limit, percent, thousands,
};
pub use tempest_api::words::{count, duration as duration_words};

/// A rate in bytes per second, or a dash for none.
pub fn rate(bytes_per_second: u64) -> String {
    match bytes_per_second {
        0 => "–".to_owned(),
        rate => bytes(rate),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_are_bytes_or_a_dash() {
        assert_eq!(rate(0), "–");
        assert_eq!(rate(9_800_000), "9.8M");
    }
}
