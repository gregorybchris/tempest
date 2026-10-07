//! Time as the core sees it: whatever the driver says it is.

use std::fmt;
use std::ops::{Add, AddAssign};
use std::time::Duration;

/// A point in time, as nanoseconds since an epoch the driver chooses.
///
/// The core never reads a clock. Every input carries the current time, and the core asks to be
/// woken at a later [`Time`]. The simulator counts from the start of a run; a real driver counts
/// from when it started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Time(u64);

impl Time {
    /// The epoch.
    pub const ZERO: Self = Self(0);

    /// The time `nanos` nanoseconds after the epoch.
    pub const fn from_nanos(nanos: u64) -> Self {
        Self(nanos)
    }

    /// The time `duration` after the epoch, saturating at the largest representable time.
    pub fn from_duration(duration: Duration) -> Self {
        Self(u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX))
    }

    /// Nanoseconds since the epoch.
    pub const fn as_nanos(self) -> u64 {
        self.0
    }

    /// The time since the epoch.
    pub const fn since_epoch(self) -> Duration {
        Duration::from_nanos(self.0)
    }

    /// The time from `earlier` to `self`, or zero if `earlier` is later.
    pub const fn since(self, earlier: Self) -> Duration {
        Duration::from_nanos(self.0.saturating_sub(earlier.0))
    }
}

impl Add<Duration> for Time {
    type Output = Self;

    /// Saturates at the largest representable time.
    fn add(self, rhs: Duration) -> Self {
        Self(self.0.saturating_add(Self::from_duration(rhs).0))
    }
}

impl AddAssign<Duration> for Time {
    fn add_assign(&mut self, rhs: Duration) {
        *self = *self + rhs;
    }
}

impl fmt::Display for Time {
    /// Seconds since the epoch, to the microsecond.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.6}s", self.since_epoch().as_secs_f64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_saturates() {
        let t = Time::from_nanos(u64::MAX - 1);
        assert_eq!(t + Duration::from_secs(1), Time::from_nanos(u64::MAX));
        assert_eq!(Time::ZERO.since(t), Duration::ZERO);
        assert_eq!(
            (Time::ZERO + Duration::from_millis(1500)).since(Time::ZERO),
            Duration::from_millis(1500)
        );
        assert_eq!(Time::from_nanos(1_500_000_000).to_string(), "1.500000s");
    }
}
