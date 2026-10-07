//! The weekly schedule for alternative speed limits, in local time.
//!
//! A schedule is a window that opens at `from` on each of its days and lasts until the next
//! `to`: later the same day, or the next day if `to` is earlier. When they are equal the window
//! lasts a whole day.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Minutes in a day.
const DAY: u32 = 24 * 60;

/// A day of the week.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Weekday {
    /// Monday.
    Mon,
    /// Tuesday.
    Tue,
    /// Wednesday.
    Wed,
    /// Thursday.
    Thu,
    /// Friday.
    Fri,
    /// Saturday.
    Sat,
    /// Sunday.
    Sun,
}

impl Weekday {
    /// Every day, Monday first.
    pub const ALL: [Self; 7] = [
        Self::Mon,
        Self::Tue,
        Self::Wed,
        Self::Thu,
        Self::Fri,
        Self::Sat,
        Self::Sun,
    ];

    /// The day from its number counting from Monday as 0, wrapping around.
    pub fn from_monday(days: u32) -> Self {
        Self::ALL[(days % 7) as usize]
    }

    fn previous(self) -> Self {
        Self::from_monday(self as u32 + 6)
    }
}

/// A time of day to the minute, written `HH:MM` on a 24-hour clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimeOfDay(u32);

impl TimeOfDay {
    /// The time `hour`:`minute`, if valid.
    pub fn new(hour: u32, minute: u32) -> Option<Self> {
        (hour < 24 && minute < 60).then_some(Self(hour * 60 + minute))
    }

    /// Minutes since midnight.
    pub fn minutes(self) -> u32 {
        self.0
    }
}

impl fmt::Display for TimeOfDay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}", self.0 / 60, self.0 % 60)
    }
}

impl FromStr for TimeOfDay {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let invalid = || format!("{text:?} is not a time of day such as \"22:30\"");
        let (hour, minute) = text.split_once(':').ok_or_else(invalid)?;
        let number = |part: &str| match part.len() {
            1 | 2 if part.bytes().all(|b| b.is_ascii_digit()) => part.parse().ok(),
            _ => None,
        };
        let (Some(hour), Some(minute)) = (number(hour), number(minute)) else {
            return Err(invalid());
        };
        Self::new(hour, minute).ok_or_else(invalid)
    }
}

impl Serialize for TimeOfDay {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for TimeOfDay {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// A moment in local time, as far as a schedule cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    /// The day of the week.
    pub day: Weekday,
    /// The time of day.
    pub time: TimeOfDay,
}

impl LocalTime {
    /// The current local time.
    pub fn now() -> Self {
        use chrono::{Datelike, Timelike};

        let now = chrono::Local::now();
        Self {
            day: Weekday::from_monday(now.weekday().num_days_from_monday()),
            time: TimeOfDay::new(now.hour(), now.minute()).expect("chrono's time is valid"),
        }
    }
}

/// When alternative speed limits apply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Schedule {
    /// The days the window opens on. Every day when left out.
    #[serde(default = "every_day")]
    pub days: BTreeSet<Weekday>,
    /// When the window opens.
    pub from: TimeOfDay,
    /// When it closes.
    pub to: TimeOfDay,
}

fn every_day() -> BTreeSet<Weekday> {
    Weekday::ALL.into_iter().collect()
}

impl Schedule {
    /// Whether the window is open at `at`.
    pub fn contains(&self, at: LocalTime) -> bool {
        let length = match (self.to.0 + DAY - self.from.0) % DAY {
            0 => DAY,
            length => length,
        };
        let now = at.time.0;
        // A window is open now only if it opened today or yesterday.
        let opened_today = self.days.contains(&at.day) && now >= self.from.0;
        let since_today = now.wrapping_sub(self.from.0);
        let opened_yesterday = self.days.contains(&at.day.previous());
        let since_yesterday = now + DAY - self.from.0;
        (opened_today && since_today < length) || (opened_yesterday && since_yesterday < length)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn time(text: &str) -> TimeOfDay {
        text.parse().unwrap()
    }

    fn at(day: Weekday, text: &str) -> LocalTime {
        LocalTime {
            day,
            time: time(text),
        }
    }

    fn schedule(days: &[Weekday], from: &str, to: &str) -> Schedule {
        Schedule {
            days: days.iter().copied().collect(),
            from: time(from),
            to: time(to),
        }
    }

    #[test]
    fn times_of_day_are_read_and_written() {
        assert_eq!(time("7:05").minutes(), 7 * 60 + 5);
        assert_eq!(time("23:59").to_string(), "23:59");
        assert_eq!(time("00:00").to_string(), "00:00");
        for bad in [
            "24:00", "12:60", "12", "1:2:3", "-1:00", "ab:cd", "123:00", "",
        ] {
            assert!(bad.parse::<TimeOfDay>().is_err(), "{bad}");
        }
    }

    #[test]
    fn a_daytime_window_covers_its_hours_on_its_days() {
        use Weekday::*;
        let work = schedule(&[Mon, Tue, Wed, Thu, Fri], "09:00", "17:30");
        assert!(work.contains(at(Mon, "09:00")));
        assert!(work.contains(at(Fri, "17:29")));
        assert!(!work.contains(at(Fri, "17:30")));
        assert!(!work.contains(at(Wed, "08:59")));
        assert!(!work.contains(at(Sat, "12:00")));
    }

    #[test]
    fn a_night_window_runs_into_the_next_day() {
        use Weekday::*;
        let nights = schedule(&[Fri, Sat], "22:00", "07:00");
        assert!(nights.contains(at(Fri, "22:00")));
        assert!(nights.contains(at(Sat, "03:00")));
        assert!(nights.contains(at(Sun, "06:59")));
        assert!(!nights.contains(at(Sun, "07:00")));
        assert!(!nights.contains(at(Fri, "03:00")), "Thursday has no window");
        assert!(!nights.contains(at(Sun, "22:00")));
        // Sunday's window, opened Saturday, wraps from the week's end to its start.
        let sunday = schedule(&[Sun], "23:00", "01:00");
        assert!(sunday.contains(at(Mon, "00:30")));
    }

    #[test]
    fn equal_ends_mean_a_whole_day() {
        use Weekday::*;
        let day = schedule(&[Wed], "06:00", "06:00");
        assert!(day.contains(at(Wed, "06:00")));
        assert!(day.contains(at(Thu, "05:59")));
        assert!(!day.contains(at(Thu, "06:00")));
        assert!(!day.contains(at(Wed, "05:59")));
    }

    #[test]
    fn days_default_to_every_day() {
        let schedule: Schedule =
            serde_json::from_value(serde_json::json!({"from": "01:00", "to": "02:00"})).unwrap();
        assert_eq!(schedule.days.len(), 7);
        let text = serde_json::to_string(&schedule).unwrap();
        assert!(text.contains("\"from\":\"01:00\""), "{text}");
    }

    proptest! {
        /// A window is open for exactly its length each week, once per listed day.
        #[test]
        fn the_window_is_open_for_its_length(
            days in proptest::collection::btree_set(0u32..7, 0..=7),
            from in 0u32..DAY,
            to in 0u32..DAY,
        ) {
            let schedule = Schedule {
                days: days.iter().map(|&d| Weekday::from_monday(d)).collect(),
                from: TimeOfDay(from),
                to: TimeOfDay(to),
            };
            let length = match (to + DAY - from) % DAY { 0 => DAY, l => l };
            let open = (0..7 * DAY)
                .filter(|minute| {
                    schedule.contains(LocalTime {
                        day: Weekday::from_monday(minute / DAY),
                        time: TimeOfDay(minute % DAY),
                    })
                })
                .count() as u32;
            prop_assert_eq!(open, length * days.len() as u32);
        }
    }
}
