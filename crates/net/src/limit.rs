//! Speed limits: token buckets that every transfer and connection they cover draw from.
//!
//! A limit counts Data payload bytes. An upload draws from its bundle's limit and the node's
//! before its Data is sent, and a download after its Data arrives, before the chunks are stored
//! and the session hears of them. Holding a response back holds its request slot, so the
//! session's pipeline shrinks to what the limit allows.
//!
//! A bucket may go into debt: a frame larger than what is saved up still goes, and later ones
//! wait until the debt is paid off. So frames of any size pass, and the rate over time stays
//! within the limit, plus at most one burst and one frame.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::time::Instant;

/// How long a bucket saves up for at its rate: the burst it allows after being idle.
pub const BURST: Duration = Duration::from_millis(250);
/// The longest a waiter sleeps before looking again, so a changed rate takes effect soon.
const RECHECK: Duration = Duration::from_millis(100);

/// A token bucket, with time passed in. Tokens are bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct TokenBucket {
    /// Bytes per second, or `None` for no limit.
    rate: Option<u64>,
    /// Bytes saved up, or owed when negative.
    tokens: f64,
    /// When `tokens` was last brought up to date.
    last: Instant,
}

impl TokenBucket {
    /// A bucket for `rate` bytes per second, starting with a full burst. A rate of 0 counts as
    /// 1.
    pub fn new(rate: Option<u64>, now: Instant) -> Self {
        let rate = rate.map(|rate| rate.max(1));
        Self {
            rate,
            tokens: rate.map_or(0.0, burst),
            last: now,
        }
    }

    /// Bytes per second, or `None` for no limit.
    pub fn rate(&self) -> Option<u64> {
        self.rate
    }

    /// Changes the rate. What is saved up or owed carries over, up to the new burst.
    pub fn set_rate(&mut self, rate: Option<u64>, now: Instant) {
        let rate = rate.map(|rate| rate.max(1));
        self.refill(now);
        self.tokens = match rate {
            Some(rate) => self.tokens.min(burst(rate)),
            None => 0.0,
        };
        self.rate = rate;
    }

    fn refill(&mut self, now: Instant) {
        if let Some(rate) = self.rate {
            let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
            self.tokens = (self.tokens + elapsed * rate as f64).min(burst(rate));
        }
        self.last = self.last.max(now);
    }

    /// Takes `bytes` if the bucket owes nothing, and returns `None`. Otherwise takes nothing and
    /// returns how long until it owes nothing.
    pub fn take(&mut self, bytes: u64, now: Instant) -> Option<Duration> {
        let rate = self.rate?;
        self.refill(now);
        // Owing less than a thousandth of a byte is rounding, not debt.
        if self.tokens > -1e-3 {
            self.tokens -= bytes as f64;
            return None;
        }
        Some(Duration::from_secs_f64(-self.tokens / rate as f64))
    }
}

/// The burst a bucket for `rate` saves up to.
fn burst(rate: u64) -> f64 {
    rate as f64 * BURST.as_secs_f64()
}

/// A speed limit shared by everything that holds a clone of it.
#[derive(Debug, Clone)]
pub struct RateLimit(Arc<Mutex<TokenBucket>>);

impl Default for RateLimit {
    fn default() -> Self {
        Self::new(None)
    }
}

impl RateLimit {
    /// A limit of `rate` bytes per second, or none.
    pub fn new(rate: Option<u64>) -> Self {
        Self(Arc::new(Mutex::new(TokenBucket::new(rate, Instant::now()))))
    }

    fn bucket(&self) -> std::sync::MutexGuard<'_, TokenBucket> {
        // A panic can't leave the bucket half-changed, so a poisoned lock is still good.
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Bytes per second, or `None` for no limit.
    pub fn rate(&self) -> Option<u64> {
        self.bucket().rate()
    }

    /// Changes the rate, for everything sharing the limit, within a tenth of a second.
    pub fn set_rate(&self, rate: Option<u64>) {
        let mut bucket = self.bucket();
        if bucket.rate() != rate.map(|rate| rate.max(1)) {
            bucket.set_rate(rate, Instant::now());
        }
    }

    /// Waits until `bytes` may pass, and counts them.
    pub async fn acquire(&self, bytes: u64) {
        loop {
            let wait = self.bucket().take(bytes, Instant::now());
            match wait {
                None => return,
                Some(wait) => tokio::time::sleep(wait.min(RECHECK)).await,
            }
        }
    }
}

/// Download and upload limits: a node's, over every bundle, or one bundle's.
#[derive(Debug, Clone, Default)]
pub struct SpeedLimits {
    /// Verified Data received.
    pub download: RateLimit,
    /// Data sent.
    pub upload: RateLimit,
}

/// The limits a bundle's transfers draw from: its own and its node's.
#[derive(Debug, Clone)]
pub(crate) struct Throttle {
    pub bundle: SpeedLimits,
    pub node: SpeedLimits,
}

impl Throttle {
    /// Waits until `bytes` of Data may be sent.
    pub async fn upload(&self, bytes: u64) {
        self.bundle.upload.acquire(bytes).await;
        self.node.upload.acquire(bytes).await;
    }

    /// Waits until `bytes` of Data received may be taken in.
    pub async fn download(&self, bytes: u64) {
        self.bundle.download.acquire(bytes).await;
        self.node.download.acquire(bytes).await;
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const KIB: u64 = 1024;

    #[test]
    fn no_limit_never_waits() {
        let now = Instant::now();
        let mut bucket = TokenBucket::new(None, now);
        for _ in 0..1000 {
            assert_eq!(bucket.take(u64::MAX / 2, now), None);
        }
    }

    #[test]
    fn a_burst_passes_then_the_rate_holds() {
        let start = Instant::now();
        let mut bucket = TokenBucket::new(Some(100 * KIB), start);
        // A quarter second's worth is saved up; taking it all leaves nothing owed.
        assert_eq!(bucket.take(25 * KIB, start), None);
        // One more frame goes into debt.
        assert_eq!(bucket.take(10 * KIB, start), None);
        // The next waits until the 10 KiB are paid off: a tenth of a second.
        let wait = bucket.take(KIB, start).unwrap();
        assert!((wait.as_secs_f64() - 0.1).abs() < 1e-9, "{wait:?}");
        assert!(bucket.take(KIB, start + wait / 2).is_some());
        assert_eq!(bucket.take(KIB, start + wait), None);
    }

    #[test]
    fn a_frame_larger_than_the_burst_still_passes() {
        let start = Instant::now();
        let mut bucket = TokenBucket::new(Some(KIB), start);
        assert_eq!(bucket.take(1024 * KIB, start), None);
        let wait = bucket.take(1, start).unwrap();
        // About 1,024 seconds, less the quarter-second burst.
        assert!((wait.as_secs_f64() - 1023.75).abs() < 1e-6, "{wait:?}");
    }

    #[test]
    fn idle_time_saves_up_only_a_burst() {
        let start = Instant::now();
        let mut bucket = TokenBucket::new(Some(100 * KIB), start);
        let later = start + Duration::from_secs(3600);
        assert_eq!(bucket.take(25 * KIB, later), None);
        assert_eq!(bucket.take(1, later), None);
        assert!(bucket.take(1, later).is_some());
    }

    #[test]
    fn changing_the_rate_keeps_the_debt() {
        let start = Instant::now();
        let mut bucket = TokenBucket::new(Some(100 * KIB), start);
        assert_eq!(bucket.take(125 * KIB, start), None);
        // 100 KiB owed at 200 KiB/s is half a second.
        bucket.set_rate(Some(200 * KIB), start);
        let wait = bucket.take(1, start).unwrap();
        assert!((wait.as_secs_f64() - 0.5).abs() < 1e-9, "{wait:?}");
        bucket.set_rate(None, start);
        assert_eq!(bucket.take(1, start), None);
        // A limit set again starts with nothing saved up.
        bucket.set_rate(Some(KIB), start);
        assert_eq!(bucket.take(KIB, start), None);
        assert!(bucket.take(1, start).is_some());
    }

    proptest! {
        /// Whatever is asked for, and whenever, what passes stays within the rate times the
        /// time, plus a burst and one frame.
        #[test]
        fn what_passes_stays_within_the_rate(
            rate in 1u64..10_000_000,
            asks in proptest::collection::vec((0u64..200_000, 1u64..1_000_000), 1..200),
        ) {
            let start = Instant::now();
            let mut bucket = TokenBucket::new(Some(rate), start);
            let mut now = start;
            let mut passed = 0u64;
            let mut largest = 0u64;
            for (micros, bytes) in asks {
                now += Duration::from_micros(micros);
                if bucket.take(bytes, now).is_none() {
                    passed += bytes;
                    largest = largest.max(bytes);
                }
            }
            let elapsed = now.duration_since(start).as_secs_f64();
            let allowed = rate as f64 * elapsed + burst(rate) + largest as f64;
            prop_assert!(passed as f64 <= allowed + 1.0, "{passed} > {allowed}");
        }
    }
}
