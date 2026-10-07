//! Byte counters with a recent rate.

use std::collections::VecDeque;
use std::time::Duration;

use crate::time::Time;

/// Counts bytes transferred, in total and over a sliding window.
#[derive(Debug, Clone)]
pub struct Meter {
    window: Duration,
    total: u64,
    recent: VecDeque<(Time, u64)>,
    recent_bytes: u64,
}

impl Meter {
    /// A meter whose rate covers the last `window`.
    pub fn new(window: Duration) -> Self {
        Self {
            window,
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
    pub fn rate(&mut self, now: Time) -> f64 {
        self.recent(now) as f64 / self.window.as_secs_f64()
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
        let mut meter = Meter::new(Duration::from_secs(10));
        meter.record(at(0), 100);
        meter.record(at(5), 200);
        meter.record(at(5), 50);
        assert_eq!(meter.rate(at(5)), 35.0);
        assert_eq!(meter.recent(at(10)), 250);
        assert_eq!(meter.recent(at(15)), 0);
        assert_eq!(meter.total(), 350);
    }
}
