//! The engine's event stream: status snapshots and decision events, numbered so a client that
//! disconnects can resume where it left off (D-080).

use std::collections::VecDeque;
use std::sync::Arc;

use tempest_core::Event;
use tempest_protocol::BundleId;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;

use crate::TransferInfo;

/// Something that happened in the engine.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineEvent {
    /// A transfer was added or changed: its new state.
    Transfer(Box<TransferInfo>),
    /// A transfer was removed.
    Removed {
        /// The transfer.
        id: BundleId,
    },
    /// A transfer's session made a decision or observed something (D-006).
    Decision {
        /// The transfer.
        id: BundleId,
        /// What happened.
        event: Event,
    },
    /// A transfer's decision events came faster than the engine could pass them on, and this
    /// many were dropped.
    DecisionsMissed {
        /// The transfer.
        id: BundleId,
        /// How many.
        count: u64,
    },
}

/// An event and its number. Numbers start at 1 when the engine starts and increase by one per
/// event.
#[derive(Debug, Clone, PartialEq)]
pub struct Numbered {
    /// The event's number.
    pub id: u64,
    /// The event.
    pub event: EngineEvent,
}

/// The engine's events, kept in a bounded buffer for clients that resume, and sent live to
/// subscribers. Owned by the engine task.
#[derive(Debug)]
pub(crate) struct EventLog {
    next: u64,
    buffer: VecDeque<Arc<Numbered>>,
    capacity: usize,
    live: broadcast::Sender<Arc<Numbered>>,
}

impl EventLog {
    /// A log that keeps the last `capacity` events for replay, and lets a live subscriber fall
    /// `capacity` events behind before it misses some.
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            next: 1,
            buffer: VecDeque::with_capacity(capacity),
            capacity,
            live: broadcast::channel(capacity).0,
        }
    }

    /// Numbers `event`, keeps it and sends it to subscribers.
    pub fn push(&mut self, event: EngineEvent) {
        let numbered = Arc::new(Numbered {
            id: self.next,
            event,
        });
        self.next += 1;
        if self.buffer.len() == self.capacity {
            self.buffer.pop_front();
        }
        self.buffer.push_back(numbered.clone());
        // Nobody listening is fine.
        let _ = self.live.send(numbered);
    }

    /// A subscription to every event after `after`, or to new events only with `None`.
    pub fn subscribe(&self, after: Option<u64>) -> Subscription {
        let live = self.live.subscribe();
        let Some(after) = after else {
            return Subscription {
                replay: VecDeque::new(),
                missed: false,
                live,
            };
        };
        let oldest = self.buffer.front().map_or(self.next, |event| event.id);
        // An ID from the future is from before the engine restarted: its numbering is gone.
        let missed = after.saturating_add(1) < oldest || after >= self.next;
        let replay = self
            .buffer
            .iter()
            .filter(|event| missed || event.id > after)
            .cloned()
            .collect();
        Subscription {
            replay,
            missed,
            live,
        }
    }
}

/// Events for one subscriber: those it asked to replay, then live ones.
#[derive(Debug)]
pub struct Subscription {
    replay: VecDeque<Arc<Numbered>>,
    missed: bool,
    live: broadcast::Receiver<Arc<Numbered>>,
}

/// Why a subscription has no next event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SubscriptionError {
    /// The subscriber fell too far behind, and this many events were dropped. It can subscribe
    /// again after the last event it saw to get what is still buffered.
    #[error("missed {0} events")]
    Lagged(u64),
    /// The engine has stopped.
    #[error("the engine has stopped")]
    Closed,
}

impl Subscription {
    /// Whether events after the one asked to resume from were already dropped from the buffer,
    /// or its numbering is from before the engine restarted. Then the replay starts at the
    /// oldest event kept, and a client should refresh its view of every transfer.
    pub fn missed(&self) -> bool {
        self.missed
    }

    /// The next event.
    pub async fn recv(&mut self) -> Result<Arc<Numbered>, SubscriptionError> {
        if let Some(event) = self.replay.pop_front() {
            return Ok(event);
        }
        match self.live.recv().await {
            Ok(event) => Ok(event),
            Err(RecvError::Lagged(count)) => Err(SubscriptionError::Lagged(count)),
            Err(RecvError::Closed) => Err(SubscriptionError::Closed),
        }
    }
}

#[cfg(test)]
mod tests {
    use tempest_protocol::Hash;

    use super::*;

    fn removed(n: u8) -> EngineEvent {
        EngineEvent::Removed {
            id: Hash::new([n; 32]),
        }
    }

    async fn ids(subscription: &mut Subscription, count: usize) -> Vec<u64> {
        let mut ids = Vec::new();
        for _ in 0..count {
            ids.push(subscription.recv().await.unwrap().id);
        }
        ids
    }

    #[tokio::test]
    async fn subscribers_resume_after_the_last_event_they_saw() {
        let mut log = EventLog::new(3);
        let mut live = log.subscribe(None);
        log.push(removed(0));
        assert_eq!(ids(&mut live, 1).await, [1]);
        for n in 1..5 {
            log.push(removed(n));
        }
        // Events 3 to 5 are buffered.

        let mut resumed = log.subscribe(Some(3));
        assert!(!resumed.missed());
        log.push(removed(9));
        assert_eq!(ids(&mut resumed, 3).await, [4, 5, 6]);

        let mut late = log.subscribe(Some(1));
        assert!(late.missed(), "event 2 is gone");
        assert_eq!(ids(&mut late, 3).await, [4, 5, 6]);

        let mut restarted = log.subscribe(Some(100));
        assert!(restarted.missed());
        assert_eq!(ids(&mut restarted, 3).await, [4, 5, 6]);

        let caught_up = log.subscribe(Some(6));
        assert!(!caught_up.missed());
        assert!(caught_up.replay.is_empty());
    }

    #[tokio::test]
    async fn a_slow_subscriber_is_told_how_many_it_missed() {
        let mut log = EventLog::new(2);
        let mut slow = log.subscribe(None);
        for n in 0..5 {
            log.push(removed(n));
        }
        assert_eq!(slow.recv().await, Err(SubscriptionError::Lagged(3)));
        assert_eq!(slow.recv().await.unwrap().id, 4);
        drop(log);
        assert_eq!(slow.recv().await.unwrap().id, 5);
        assert_eq!(slow.recv().await, Err(SubscriptionError::Closed));
    }
}
