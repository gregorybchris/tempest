//! When to renew a port mapping and when to try again after a failure, apart from any I/O.

use std::time::Duration;

use tokio::time::Instant;

/// The lifetime asked for: RFC 6886's recommendation, which PCP and UPnP-IGD routers take too.
pub const REQUESTED_LIFETIME: Duration = Duration::from_secs(2 * 60 * 60);
/// How often to ask again for a permanent mapping, which a router forgets when it restarts.
const PERMANENT_REFRESH: Duration = Duration::from_secs(30 * 60);
/// The shortest wait before renewing.
const MIN_RENEW: Duration = Duration::from_secs(1);
/// The wait after the first failure to map, doubled after each further one.
const MIN_RETRY: Duration = Duration::from_secs(60);
/// The longest wait between attempts to map.
const MAX_RETRY: Duration = Duration::from_secs(60 * 60);

/// A mapping the router granted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lease {
    /// When the router granted it.
    pub granted: Instant,
    /// How long it lasts, or `None` if it lasts until removed.
    pub lifetime: Option<Duration>,
}

impl Lease {
    /// When it runs out, unless renewed.
    pub fn expires(&self) -> Option<Instant> {
        self.lifetime.map(|lifetime| self.granted + lifetime)
    }

    /// Whether it has run out by `now`.
    pub fn expired(&self, now: Instant) -> bool {
        self.expires().is_some_and(|expires| expires <= now)
    }

    /// When to renew it: halfway through its lifetime.
    pub fn renew_at(&self) -> Instant {
        match self.lifetime {
            Some(lifetime) => self.granted + (lifetime / 2).max(MIN_RENEW),
            None => self.granted + PERMANENT_REFRESH,
        }
    }

    /// When to try renewing again after a renewal failed at `now`: halfway to its expiry, so
    /// several tries fit before it runs out. `None` once it has run out.
    pub fn retry_at(&self, now: Instant) -> Option<Instant> {
        if self.expired(now) {
            return None;
        }
        let wait = match self.expires() {
            Some(expires) => ((expires - now) / 2).max(MIN_RENEW),
            None => MIN_RETRY,
        };
        Some(now + wait)
    }
}

/// The waits between attempts to map that fail.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Backoff {
    failures: u32,
}

impl Backoff {
    /// An attempt succeeded: the next failure waits the shortest time again.
    pub fn reset(&mut self) {
        self.failures = 0;
    }

    /// An attempt failed at `now`: when to try again.
    pub fn failed(&mut self, now: Instant) -> Instant {
        let wait = MIN_RETRY.saturating_mul(1 << self.failures.min(16));
        self.failures = self.failures.saturating_add(1);
        now + wait.min(MAX_RETRY)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn secs(secs: u64) -> Duration {
        Duration::from_secs(secs)
    }

    #[test]
    fn a_lease_is_renewed_halfway() {
        let now = Instant::now();
        let lease = Lease {
            granted: now,
            lifetime: Some(secs(7200)),
        };
        assert_eq!(lease.renew_at(), now + secs(3600));
        assert_eq!(lease.expires(), Some(now + secs(7200)));
        assert!(!lease.expired(now + secs(7199)));
        assert!(lease.expired(now + secs(7200)));
    }

    #[test]
    fn short_leases_wait_at_least_a_second() {
        let now = Instant::now();
        let lease = Lease {
            granted: now,
            lifetime: Some(secs(1)),
        };
        assert_eq!(lease.renew_at(), now + secs(1));
    }

    #[test]
    fn permanent_leases_are_refreshed_and_never_expire() {
        let now = Instant::now();
        let lease = Lease {
            granted: now,
            lifetime: None,
        };
        assert_eq!(lease.renew_at(), now + PERMANENT_REFRESH);
        assert!(!lease.expired(now + secs(1_000_000)));
        assert_eq!(lease.retry_at(now), Some(now + MIN_RETRY));
    }

    #[test]
    fn failed_renewals_retry_halfway_to_expiry() {
        let now = Instant::now();
        let lease = Lease {
            granted: now,
            lifetime: Some(secs(7200)),
        };
        let failed = now + secs(3600);
        assert_eq!(lease.retry_at(failed), Some(failed + secs(1800)));
        assert_eq!(lease.retry_at(now + secs(7200)), None);
    }

    #[test]
    fn failures_back_off_up_to_an_hour() {
        let now = Instant::now();
        let mut backoff = Backoff::default();
        let waits: Vec<Duration> = (0..8).map(|_| backoff.failed(now) - now).collect();
        assert_eq!(waits, [60, 120, 240, 480, 960, 1920, 3600, 3600].map(secs));
        backoff.reset();
        assert_eq!(backoff.failed(now), now + MIN_RETRY);
    }

    proptest! {
        #[test]
        fn renewal_comes_before_expiry(lifetime in 2u64..1_000_000) {
            let now = Instant::now();
            let lease = Lease { granted: now, lifetime: Some(secs(lifetime)) };
            prop_assert!(lease.renew_at() < lease.expires().unwrap());
            prop_assert!(lease.renew_at() > now);
        }

        #[test]
        fn retries_stay_within_a_live_lease(lifetime in 4u64..1_000_000, elapsed in 0u64..1_000_000) {
            let now = Instant::now();
            let lease = Lease { granted: now, lifetime: Some(secs(lifetime)) };
            let failed = now + secs(elapsed);
            match lease.retry_at(failed) {
                None => prop_assert!(lease.expired(failed)),
                Some(retry) => {
                    prop_assert!(retry > failed);
                    // Only the last second of a lease may push a retry past its end.
                    let left = lease.expires().unwrap() - failed;
                    prop_assert!(left <= MIN_RENEW || retry < lease.expires().unwrap());
                }
            }
        }

        #[test]
        fn backoff_grows_and_is_capped(failures in 1usize..64) {
            let now = Instant::now();
            let mut backoff = Backoff::default();
            let mut last = Duration::ZERO;
            for _ in 0..failures {
                let wait = backoff.failed(now) - now;
                prop_assert!(wait >= last && wait <= MAX_RETRY);
                last = wait;
            }
        }
    }
}
