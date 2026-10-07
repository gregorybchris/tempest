//! The weekly schedule for alternative speed limits, in local time. The types live in
//! [`tempest_api::schedule`], which clients share; this adds the engine's clock.

pub use tempest_api::schedule::*;

/// The current local time.
pub fn local_now() -> LocalTime {
    use chrono::{Datelike, Timelike};

    let now = chrono::Local::now();
    LocalTime {
        day: Weekday::from_monday(now.weekday().num_days_from_monday()),
        time: TimeOfDay::new(now.hour(), now.minute()).expect("chrono's time is valid"),
    }
}
