//! How many requests to keep open with a provider (`specs/strategies.md`, Requests and
//! pipelining; D-060).
//!
//! To keep a link busy, a requester needs about *rate × round-trip time* bytes requested at any
//! moment. The session measures each provider's rate and round-trip time and keeps a multiple
//! of that product requested, so a provider that could send faster gets more requests until the
//! link, not the pipeline, is the limit.

use std::collections::VecDeque;
use std::time::Duration;

use crate::config::RequestSettings;
use crate::event::InFlightReason;

/// A provider's round-trip time, as estimated by the session or reported by the driver.
///
/// The session's own samples are the times from sending a request to its response. They include
/// the provider's queueing and sending time, so the estimate is the smallest sample seen. A
/// driver that knows the transport's round-trip time reports it instead
/// ([`Session::handle_rtt`](crate::Session::handle_rtt)), and then the session's samples are
/// ignored.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RttEstimate {
    rtt: Option<Duration>,
    reported: bool,
}

impl RttEstimate {
    /// The estimate, once there is one.
    pub fn get(self) -> Option<Duration> {
        self.rtt
    }

    /// Adds the time from sending a request to its response.
    pub fn sample(&mut self, elapsed: Duration) {
        if !self.reported {
            self.rtt = Some(self.rtt.map_or(elapsed, |rtt| rtt.min(elapsed)));
        }
    }

    /// Replaces the estimate with the driver's.
    pub fn report(&mut self, rtt: Duration) {
        self.rtt = Some(rtt);
        self.reported = true;
    }
}

/// A provider's delivery rate, measured per response as in BBR: the bytes received from the
/// provider between sending a request and receiving its response, divided by that time.
///
/// While the pipeline limits the rate, each sample covers exactly one round trip's worth of
/// responses, so it tracks the current rate without the lag of a sliding window. The estimate
/// is the largest of the last few samples, so that a response that arrives alone, such as the
/// first, doesn't shrink the pipeline.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeliveryRate {
    samples: VecDeque<f64>,
}

impl DeliveryRate {
    /// How many recent samples the estimate covers.
    pub const SAMPLES: usize = 8;

    /// The estimate in bytes per second, or 0 before any sample.
    pub fn get(&self) -> f64 {
        self.samples.iter().copied().fold(0.0, f64::max)
    }

    /// Adds a sample: `bytes` received over `elapsed`. Samples over no time are ignored.
    pub fn sample(&mut self, bytes: u64, elapsed: Duration) {
        if elapsed.is_zero() {
            return;
        }
        if self.samples.len() == Self::SAMPLES {
            self.samples.pop_front();
        }
        self.samples.push_back(bytes as f64 / elapsed.as_secs_f64());
    }
}

/// What is known about a provider when sizing its pipeline.
#[derive(Debug, Clone, Copy)]
pub struct Provider {
    /// The recent rate of receiving from it, in bytes per second.
    pub rate: f64,
    /// Its round-trip time, if known.
    pub rtt: Option<Duration>,
    /// The size of a full request, in bytes.
    pub request_bytes: u64,
    /// The most requests it allows open, less headroom.
    pub limit: u32,
    /// Whether it is snubbing this peer.
    pub snubbed: bool,
}

/// How many requests to keep open with `provider`, and why.
pub fn requests_in_flight(settings: &RequestSettings, provider: Provider) -> (u32, InFlightReason) {
    if provider.snubbed {
        return (1, InFlightReason::Snubbed);
    }
    let (wanted, reason) = match provider.rtt {
        Some(rtt) if settings.adaptive && provider.rate > 0.0 => {
            let bytes = provider.rate * rtt.as_secs_f64() * settings.in_flight_factor;
            let requests = (bytes / provider.request_bytes.max(1) as f64).ceil();
            let rate = provider.rate as u64;
            // Saturating: an absurd product just means "as many as allowed".
            let requests = if requests >= f64::from(u32::MAX) {
                u32::MAX
            } else {
                requests as u32
            };
            (requests, InFlightReason::Measured { rate, rtt })
        }
        _ => (settings.initial_in_flight, InFlightReason::Initial),
    };
    let (wanted, reason) = if wanted < settings.min_in_flight {
        (settings.min_in_flight, InFlightReason::Minimum)
    } else {
        (wanted, reason)
    };
    if wanted > provider.limit {
        (provider.limit, InFlightReason::ProviderLimit)
    } else {
        (wanted, reason)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn provider(rate: f64, rtt_ms: Option<u64>) -> Provider {
        Provider {
            rate,
            rtt: rtt_ms.map(Duration::from_millis),
            request_bytes: 256 * 1024,
            limit: 30,
            snubbed: false,
        }
    }

    #[test]
    fn covers_rate_times_rtt() {
        let settings = RequestSettings::default();
        // 10 MB/s × 100 ms × 2 = 2 MB, or 8 requests of 256 KiB.
        let (requests, reason) = requests_in_flight(&settings, provider(10e6, Some(100)));
        assert_eq!(requests, 8);
        assert!(matches!(
            reason,
            InFlightReason::Measured {
                rate: 10_000_000,
                ..
            }
        ));
        assert_eq!(
            requests_in_flight(&settings, provider(1e9, Some(100))),
            (30, InFlightReason::ProviderLimit)
        );
        assert_eq!(
            requests_in_flight(&settings, provider(1e3, Some(1))),
            (2, InFlightReason::Minimum)
        );
        assert_eq!(
            requests_in_flight(&settings, provider(0.0, Some(100))),
            (4, InFlightReason::Initial)
        );
        let snubbed = Provider {
            snubbed: true,
            ..provider(1e9, Some(100))
        };
        assert_eq!(
            requests_in_flight(&settings, snubbed),
            (1, InFlightReason::Snubbed)
        );
        let fixed = RequestSettings {
            adaptive: false,
            ..settings
        };
        assert_eq!(
            requests_in_flight(&fixed, provider(10e6, Some(100))),
            (4, InFlightReason::Initial)
        );
    }

    #[test]
    fn delivery_rate_is_the_largest_recent_sample() {
        let mut rate = DeliveryRate::default();
        assert_eq!(rate.get(), 0.0);
        rate.sample(1000, Duration::ZERO);
        assert_eq!(rate.get(), 0.0);
        rate.sample(1000, Duration::from_secs(1));
        rate.sample(4000, Duration::from_secs(1));
        rate.sample(2000, Duration::from_secs(1));
        assert_eq!(rate.get(), 4000.0);
        for _ in 0..DeliveryRate::SAMPLES {
            rate.sample(500, Duration::from_secs(1));
        }
        assert_eq!(rate.get(), 500.0);
    }

    #[test]
    fn a_reported_rtt_replaces_samples() {
        let mut rtt = RttEstimate::default();
        rtt.sample(Duration::from_millis(80));
        rtt.sample(Duration::from_millis(120));
        assert_eq!(rtt.get(), Some(Duration::from_millis(80)));
        rtt.report(Duration::from_millis(100));
        rtt.sample(Duration::from_millis(5));
        assert_eq!(rtt.get(), Some(Duration::from_millis(100)));
    }

    proptest! {
        #[test]
        fn stays_within_limits(
            rate in 0.0..1e12f64,
            rtt in proptest::option::of(0..10_000u64),
            limit in 1..1000u32,
            snubbed: bool,
        ) {
            let settings = RequestSettings::default();
            let provider = Provider { limit, snubbed, ..provider(rate, rtt) };
            let (requests, _) = requests_in_flight(&settings, provider);
            prop_assert!(requests >= 1);
            prop_assert!(requests <= limit.max(1));
            if !snubbed {
                prop_assert!(requests >= settings.min_in_flight.min(limit));
            }
        }
    }
}
