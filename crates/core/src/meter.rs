//! Byte counters with a recent rate.

use std::collections::VecDeque;
use std::time::Duration;

use crate::time::Time;

/// Counts bytes transferred, in total and over a sliding window.
#[derive(Debug, Clone)]
pub struct Meter {
    window: Duration,
    start: Time,
    total: u64,
    recent: VecDeque<(Time, u64)>,
    recent_bytes: u64,
}

impl Meter {
    /// A meter started at `start`, whose rate covers the last `window`.
    pub fn new(window: Duration, start: Time) -> Self {
        Self {
            window,
            start,
            total: 0,
            recent: VecDeque::new(),
            recent_bytes: 0,
        }
    }

    /// Records `bytes` transferred at `now`. Times must not decrease.
    pub fn record(&mut self, now: Time, bytes: u64) {
        self.total += bytes;
        self.recent_bytes += bytes;
        match self.recent.back_mut() {
            Some((time, sum)) if *time == now => *sum += bytes,
            _ => self.recent.push_back((now, bytes)),
        }
        self.expire(now);
    }

    /// Every byte recorded.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// Bytes recorded within the window ending at `now`.
    pub fn recent(&mut self, now: Time) -> u64 {
        self.expire(now);
        self.recent_bytes
    }

    /// The average rate over the window ending at `now`, in bytes per second.
    ///
    /// Until a whole window has passed since the start, the rate covers the time since the
    /// start, but at least a quarter of the window, so that one early transfer doesn't look like
    /// a burst of speed.
    pub fn rate(&mut self, now: Time) -> f64 {
        let span = self.window.min(now.since(self.start)).max(self.window / 4);
        self.recent(now) as f64 / span.as_secs_f64()
    }

    fn expire(&mut self, now: Time) {
        while let Some(&(time, bytes)) = self.recent.front() {
            if now.since(time) < self.window {
                break;
            }
            self.recent.pop_front();
            self.recent_bytes -= bytes;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_covers_the_window() {
        let at = |s: u64| Time::ZERO + Duration::from_secs(s);
        let mut meter = Meter::new(Duration::from_secs(10), at(0));
        meter.record(at(0), 100);
        meter.record(at(5), 200);
        meter.record(at(5), 50);
        // Half a window has passed, so the rate covers 5 s.
        assert_eq!(meter.rate(at(5)), 70.0);
        assert_eq!(meter.rate(at(9)), 350.0 / 9.0);
        assert_eq!(meter.recent(at(10)), 250);
        assert_eq!(meter.recent(at(15)), 0);
        assert_eq!(meter.total(), 350);
        meter.record(at(20), 100);
        assert_eq!(meter.rate(at(20)), 10.0);
    }

    #[test]
    fn a_young_meter_covers_at_least_a_quarter_window() {
        let at = |ms: u64| Time::ZERO + Duration::from_millis(ms);
        let mut meter = Meter::new(Duration::from_secs(4), at(0));
        meter.record(at(1), 1000);
        assert_eq!(meter.rate(at(1)), 1000.0);
    }
}
