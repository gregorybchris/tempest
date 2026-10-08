//! Following the engine's event stream with the browser's `EventSource` (D-080).
//!
//! A browser reconnects an `EventSource` on its own, at its own pace, and gives up for good on
//! an answer that isn't a stream. So the page closes a broken stream itself and opens a new one
//! after a backoff, resuming after the last numbered event it saw with `?after=`, since a new
//! `EventSource` can't send `Last-Event-ID`.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use leptos::prelude::{TimeoutHandle, set_timeout_with_handle};
use tempest_api::{ApiEvent, PREFIX};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{EventSource, MessageEvent};

/// The events the engine names.
const EVENTS: [&str; 6] = [
    "snapshot",
    "reset",
    "transfer",
    "removed",
    "decision",
    "decisions_missed",
];

/// Whether the page is following the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Link {
    /// Opening the stream.
    #[default]
    Connecting,
    /// Following it.
    Live,
    /// It broke off; the page tries again after `retry_in`.
    Lost {
        /// How long until the next try.
        retry_in: Duration,
    },
}

/// How long to wait before each try to reconnect: doubling from a second, up to fifteen.
#[derive(Debug, Clone, Default)]
pub struct Backoff {
    failures: u32,
}

impl Backoff {
    /// The first wait.
    pub const FIRST: Duration = Duration::from_secs(1);
    /// The longest wait.
    pub const MOST: Duration = Duration::from_secs(15);

    /// The wait after one more failure.
    pub fn next_wait(&mut self) -> Duration {
        let wait = Self::FIRST
            .saturating_mul(1 << self.failures.min(8))
            .min(Self::MOST);
        self.failures += 1;
        wait
    }

    /// Starts over, after a success.
    pub fn reset(&mut self) {
        self.failures = 0;
    }
}

/// The stream's URL, resuming after event `after` if given.
pub fn url(after: Option<u64>) -> String {
    match after {
        Some(after) => format!("{PREFIX}/events?after={after}"),
        None => format!("{PREFIX}/events"),
    }
}

/// The stream being followed. Cloning it gives another handle to the same stream.
#[derive(Clone)]
pub struct Follower(Rc<RefCell<Inner>>);

struct Inner {
    source: Option<EventSource>,
    // Kept alive while the source can call them.
    listeners: Vec<Closure<dyn FnMut(MessageEvent)>>,
    on_open: Option<Closure<dyn FnMut()>>,
    on_error: Option<Closure<dyn FnMut()>>,
    last_id: Option<u64>,
    backoff: Backoff,
    retry: Option<TimeoutHandle>,
    on_event: Rc<dyn Fn(ApiEvent)>,
    on_link: Rc<dyn Fn(Link)>,
}

impl Follower {
    /// Follows the engine's events, calling `on_event` with each and `on_link` when the
    /// connection changes.
    pub fn start(on_event: impl Fn(ApiEvent) + 'static, on_link: impl Fn(Link) + 'static) -> Self {
        let follower = Self(Rc::new(RefCell::new(Inner {
            source: None,
            listeners: Vec::new(),
            on_open: None,
            on_error: None,
            last_id: None,
            backoff: Backoff::default(),
            retry: None,
            on_event: Rc::new(on_event),
            on_link: Rc::new(on_link),
        })));
        follower.connect();
        follower
    }

    /// Closes the stream for good, such as when the page signs out.
    pub fn stop(&self) {
        if let Some(retry) = self.0.borrow_mut().retry.take() {
            retry.clear();
        }
        self.close();
    }

    /// Tries again now rather than after the backoff.
    pub fn retry_now(&self) {
        if let Some(retry) = self.0.borrow_mut().retry.take() {
            retry.clear();
            self.connect();
        }
    }

    fn connect(&self) {
        self.close();
        let on_link = self.0.borrow().on_link.clone();
        on_link(Link::Connecting);
        let after = self.0.borrow().last_id;
        let Ok(source) = EventSource::new(&url(after)) else {
            self.broke();
            return;
        };
        let mut listeners = Vec::new();
        for name in EVENTS {
            let inner = Rc::downgrade(&self.0);
            let listener = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
                let Some(inner) = inner.upgrade() else { return };
                if let Ok(id) = e.last_event_id().parse::<u64>() {
                    inner.borrow_mut().last_id = Some(id);
                }
                let data = e.data().as_string().unwrap_or_default();
                // An event this page doesn't know, or can't read, is skipped.
                if let Ok(Some(event)) = ApiEvent::decode(name, &data) {
                    let on_event = inner.borrow().on_event.clone();
                    on_event(event);
                }
            });
            let _ =
                source.add_event_listener_with_callback(name, listener.as_ref().unchecked_ref());
            listeners.push(listener);
        }
        let opened = self.clone();
        let on_open = Closure::<dyn FnMut()>::new(move || {
            opened.0.borrow_mut().backoff.reset();
            let on_link = opened.0.borrow().on_link.clone();
            on_link(Link::Live);
        });
        let broken = self.clone();
        let on_error = Closure::<dyn FnMut()>::new(move || broken.broke());
        source.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        source.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        let mut inner = self.0.borrow_mut();
        inner.source = Some(source);
        inner.listeners = listeners;
        inner.on_open = Some(on_open);
        inner.on_error = Some(on_error);
    }

    /// Closes the stream and tries again after the backoff.
    fn broke(&self) {
        let wait = self.0.borrow_mut().backoff.next_wait();
        if let Some(source) = &self.0.borrow().source {
            source.close();
        }
        let on_link = self.0.borrow().on_link.clone();
        on_link(Link::Lost { retry_in: wait });
        let again = self.clone();
        let retry = set_timeout_with_handle(
            move || {
                again.0.borrow_mut().retry = None;
                again.connect();
            },
            wait,
        )
        .ok();
        self.0.borrow_mut().retry = retry;
    }

    fn close(&self) {
        let mut inner = self.0.borrow_mut();
        if let Some(source) = inner.source.take() {
            source.set_onopen(None);
            source.set_onerror(None);
            source.close();
        }
        // The closures can't be dropped while one of them runs, as `broke` may; the next
        // `connect` replaces them.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_double_up_to_a_limit_and_start_over() {
        let mut backoff = Backoff::default();
        let waits: Vec<u64> = (0..7).map(|_| backoff.next_wait().as_secs()).collect();
        assert_eq!(waits, [1, 2, 4, 8, 15, 15, 15]);
        backoff.reset();
        assert_eq!(backoff.next_wait(), Backoff::FIRST);
        for _ in 0..100 {
            assert!(backoff.next_wait() <= Backoff::MOST);
        }
    }

    #[test]
    fn resuming_asks_for_events_after_the_last_seen() {
        assert_eq!(url(None), "/v1/events");
        assert_eq!(url(Some(41)), "/v1/events?after=41");
    }
}
