//! Numbers in words, as the clients' sentences use them.

/// `count` of a noun, with an `s` unless it is one: "1 peer", "3 peers".
pub fn count(count: usize, noun: &str) -> String {
    match count {
        1 => format!("1 {noun}"),
        n => format!("{n} {noun}s"),
    }
}

/// A duration as words, rounded: "less than a minute", "3 minutes", "about 2 hours".
pub fn duration(seconds: u64) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_round_to_one_unit() {
        assert_eq!(duration(59), "less than a minute");
        assert_eq!(duration(61), "2 minutes");
        assert_eq!(duration(3600), "1 hour");
        assert_eq!(duration(3 * 86_400), "3 days");
        assert_eq!(count(1, "peer"), "1 peer");
        assert_eq!(count(0, "peer"), "0 peers");
    }
}
