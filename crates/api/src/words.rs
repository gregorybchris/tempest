//! Numbers in words, as the clients' sentences use them.

/// `count` of a noun, with an `s` unless it is one: "1 peer", "3 peers".
pub fn count(count: usize, noun: &str) -> String {
    match count {
        1 => format!("1 {noun}"),
        n => format!("{n} {noun}s"),
    }
}

/// Connected peers, and how many of them through a relay: "3 peers, 1 through a relay".
pub fn peers(connections: usize, relayed: usize) -> String {
    count(connections, "peer") + &through_relays(relayed)
}

/// How many connections go through a relay, as a clause to follow a count of peers:
/// ", 2 through relays", or nothing.
pub fn through_relays(relayed: usize) -> String {
    match relayed {
        0 => String::new(),
        1 => ", 1 through a relay".to_owned(),
        n => format!(", {n} through relays"),
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
        assert_eq!(peers(1, 0), "1 peer");
        assert_eq!(peers(3, 1), "3 peers, 1 through a relay");
        assert_eq!(peers(3, 2), "3 peers, 2 through relays");
    }
}
