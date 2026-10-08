//! Per-IP rate limits on the catalog's actions.
//!
//! [`Limiter`] is plain data with no I/O, like [`crate::swarms::Swarms`]: the caller passes the
//! time and the client's address. Each action and client has a token bucket: up to `burst`
//! requests at once, then one per `every`. IPv6 clients are grouped by their /64, which one host
//! usually controls whole.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use tempest_protocol::tracker::ErrorCode;

use crate::swarms::Refusal;

/// What a client is doing, each with its own limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    /// Asking for a sign-in code.
    Challenge,
    /// Signing a code in, or polling for its session.
    SignIn,
    /// Reading the catalog: searching, and viewing listings, comments and accounts.
    Browse,
    /// Asking, in an announce, for a bundle the catalog doesn't know yet to be listed.
    Publish,
    /// Editing or withdrawing a listing, editing an account or deleting a comment.
    Edit,
    /// Voting on a listing.
    Vote,
    /// Commenting on a listing.
    Comment,
    /// A download counted from a `completed` announce.
    Download,
}

/// A token bucket: up to `burst` requests at once, refilled at one per `every`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rate {
    /// The most requests allowed at once.
    pub burst: u32,
    /// How long it takes to earn one more.
    pub every: Duration,
}

impl Rate {
    /// `burst` at once, then one per `every_secs` seconds.
    pub const fn new(burst: u32, every_secs: u64) -> Self {
        Self {
            burst,
            every: Duration::from_secs(every_secs),
        }
    }
}

/// The catalog's rate limits per client IP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogLimits {
    /// The limit on each action. An action without one is unlimited.
    pub rates: HashMap<Action, Rate>,
    /// The most clients tracked across all actions, which bounds the limiter's memory. New
    /// clients are refused with `unavailable` beyond it until old ones go idle.
    pub max_clients: usize,
}

impl Default for CatalogLimits {
    fn default() -> Self {
        let rates = [
            (Action::Challenge, Rate::new(10, 30)),
            // A browser polls every few seconds while a code waits.
            (Action::SignIn, Rate::new(60, 1)),
            (Action::Browse, Rate::new(120, 1)),
            // Sharers publish many bundles at once; refused ones are asked again later.
            (Action::Publish, Rate::new(100, 30)),
            (Action::Edit, Rate::new(30, 10)),
            (Action::Vote, Rate::new(60, 5)),
            (Action::Comment, Rate::new(10, 30)),
            (Action::Download, Rate::new(50, 60)),
        ];
        Self {
            rates: rates.into_iter().collect(),
            max_clients: 100_000,
        }
    }
}

impl CatalogLimits {
    /// No limits at all.
    pub fn none() -> Self {
        Self {
            rates: HashMap::new(),
            ..Self::default()
        }
    }
}

impl Action {
    /// Every action, for parsing and listing them.
    pub const ALL: [Self; 8] = [
        Self::Challenge,
        Self::SignIn,
        Self::Browse,
        Self::Publish,
        Self::Edit,
        Self::Vote,
        Self::Comment,
        Self::Download,
    ];

    /// The action's name, as `--catalog-limit` takes it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Challenge => "challenge",
            Self::SignIn => "sign-in",
            Self::Browse => "browse",
            Self::Publish => "publish",
            Self::Edit => "edit",
            Self::Vote => "vote",
            Self::Comment => "comment",
            Self::Download => "download",
        }
    }
}

impl std::str::FromStr for Action {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|action| action.name() == text)
            .ok_or_else(|| {
                let names: Vec<_> = Self::ALL.iter().map(|action| action.name()).collect();
                format!("unknown action {text:?}; one of {}", names.join(", "))
            })
    }
}

/// How often the limiter may sweep idle clients while it is full.
const SWEEP_EVERY: Duration = Duration::from_secs(1);

/// How long a full limiter asks new clients to wait.
const FULL_RETRY: Duration = Duration::from_secs(60);

/// The rate limiter: a bucket per action and client.
#[derive(Debug)]
pub struct Limiter {
    limits: CatalogLimits,
    /// When each bucket is full again (the GCRA's theoretical arrival time). A bucket whose
    /// time has passed is idle and can be forgotten.
    buckets: HashMap<(Action, IpAddr), Instant>,
    last_sweep: Option<Instant>,
}

/// The address a client is limited by: IPv4, or the /64 of IPv6.
pub(crate) fn client_key(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(v6) => {
            let prefix = u128::from(v6) & !((1u128 << 64) - 1);
            IpAddr::V6(prefix.into())
        }
        v4 => v4,
    }
}

impl Limiter {
    /// A limiter that has seen no clients yet.
    pub fn new(limits: CatalogLimits) -> Self {
        Self {
            limits,
            buckets: HashMap::new(),
            last_sweep: None,
        }
    }

    /// Takes one request of `action` from `ip` at `now`, or refuses it with `rate_limited` and
    /// when to try again.
    pub fn check(&mut self, now: Instant, action: Action, ip: IpAddr) -> Result<(), Refusal> {
        let Some(&rate) = self.limits.rates.get(&action) else {
            return Ok(());
        };
        if rate.burst == 0 {
            let refusal = Refusal::new(ErrorCode::RateLimited, "this action is turned off here");
            return Err(refusal.retry_after(FULL_RETRY));
        }
        let key = (action, client_key(ip));
        if !self.buckets.contains_key(&key) && self.buckets.len() >= self.limits.max_clients {
            self.sweep(now);
            if self.buckets.len() >= self.limits.max_clients {
                let refusal = Refusal::new(ErrorCode::Unavailable, "too many clients at once");
                return Err(refusal.retry_after(FULL_RETRY));
            }
        }
        let full_at = self.buckets.get(&key).copied().unwrap_or(now).max(now);
        // The bucket holds `burst` requests: full_at may run that far ahead of now.
        let allowed_ahead = rate.every * (rate.burst - 1);
        let ahead = full_at - now;
        if ahead > allowed_ahead {
            let refusal = Refusal::new(ErrorCode::RateLimited, "too many requests; slow down");
            return Err(refusal.retry_after(ahead - allowed_ahead));
        }
        self.buckets.insert(key, full_at + rate.every);
        Ok(())
    }

    /// Forgets idle buckets, at most once per [`SWEEP_EVERY`].
    fn sweep(&mut self, now: Instant) {
        if self
            .last_sweep
            .is_some_and(|last| now.saturating_duration_since(last) < SWEEP_EVERY)
        {
            return;
        }
        self.last_sweep = Some(now);
        self.buckets.retain(|_, full_at| *full_at > now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    fn limiter(rate: Rate) -> Limiter {
        let mut limits = CatalogLimits::none();
        limits.rates.insert(Action::Comment, rate);
        Limiter::new(limits)
    }

    #[test]
    fn a_burst_then_one_per_interval() {
        let mut limiter = limiter(Rate::new(3, 10));
        let start = Instant::now();
        let client = ip("198.51.100.1");
        for _ in 0..3 {
            limiter.check(start, Action::Comment, client).unwrap();
        }
        let refusal = limiter.check(start, Action::Comment, client).unwrap_err();
        assert_eq!(refusal.status, 429);
        assert_eq!(refusal.body.error, ErrorCode::RateLimited);
        assert_eq!(refusal.body.retry_after_secs, Some(10));
        // One more is earned per interval.
        let later = start + Duration::from_secs(10);
        limiter.check(later, Action::Comment, client).unwrap();
        assert!(limiter.check(later, Action::Comment, client).is_err());
        // Other clients and other actions have their own buckets.
        limiter
            .check(start, Action::Comment, ip("198.51.100.2"))
            .unwrap();
        limiter.check(start, Action::Vote, client).unwrap();
    }

    #[test]
    fn action_names_parse_back() {
        for action in Action::ALL {
            assert_eq!(action.name().parse::<Action>(), Ok(action));
        }
        assert!("nonsense".parse::<Action>().is_err());
    }

    #[test]
    fn ipv6_clients_share_their_slash_64() {
        let mut limiter = limiter(Rate::new(1, 60));
        let now = Instant::now();
        limiter
            .check(now, Action::Comment, ip("2001:db8::1"))
            .unwrap();
        assert!(
            limiter
                .check(now, Action::Comment, ip("2001:db8::ffff:2"))
                .is_err()
        );
        limiter
            .check(now, Action::Comment, ip("2001:db8:0:1::1"))
            .unwrap();
        // An IPv4-mapped address is its IPv4 address.
        limiter
            .check(now, Action::Comment, ip("192.0.2.1"))
            .unwrap();
        assert!(
            limiter
                .check(now, Action::Comment, ip("::ffff:192.0.2.1"))
                .is_err()
        );
    }

    #[test]
    fn a_full_limiter_forgets_idle_clients() {
        let mut limits = CatalogLimits::none();
        limits.rates.insert(Action::Comment, Rate::new(1, 10));
        limits.max_clients = 2;
        let mut limiter = Limiter::new(limits);
        let start = Instant::now();
        limiter
            .check(start, Action::Comment, ip("192.0.2.1"))
            .unwrap();
        limiter
            .check(start, Action::Comment, ip("192.0.2.2"))
            .unwrap();
        let refusal = limiter
            .check(start, Action::Comment, ip("192.0.2.3"))
            .unwrap_err();
        assert_eq!(refusal.body.error, ErrorCode::Unavailable);
        // Once the first two are idle, they make room.
        let later = start + Duration::from_secs(10);
        limiter
            .check(later, Action::Comment, ip("192.0.2.3"))
            .unwrap();
    }
}
