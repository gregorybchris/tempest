//! The event queue: events in time order, and in the order they were scheduled at equal times.

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;

use tempest_core::Time;

/// Future events, keyed by (time, sequence number).
#[derive(Debug)]
pub struct Queue<E> {
    heap: BinaryHeap<Reverse<Item<E>>>,
    next_seq: u64,
}

#[derive(Debug)]
struct Item<E> {
    time: Time,
    seq: u64,
    event: E,
}

impl<E> PartialEq for Item<E> {
    fn eq(&self, other: &Self) -> bool {
        (self.time, self.seq) == (other.time, other.seq)
    }
}

impl<E> Eq for Item<E> {}

impl<E> PartialOrd for Item<E> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<E> Ord for Item<E> {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.time, self.seq).cmp(&(other.time, other.seq))
    }
}

impl<E> Default for Queue<E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E> Queue<E> {
    /// An empty queue.
    pub fn new() -> Self {
        Self {
            heap: BinaryHeap::new(),
            next_seq: 0,
        }
    }

    /// Schedules `event` at `time`, after everything already scheduled at that time.
    pub fn push(&mut self, time: Time, event: E) {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.heap.push(Reverse(Item { time, seq, event }));
    }

    /// The time of the next event.
    pub fn peek_time(&self) -> Option<Time> {
        self.heap.peek().map(|Reverse(item)| item.time)
    }

    /// Takes the next event.
    pub fn pop(&mut self) -> Option<(Time, E)> {
        self.heap.pop().map(|Reverse(item)| (item.time, item.event))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use proptest::prelude::*;

    use super::*;

    proptest! {
        #[test]
        fn events_come_out_by_time_then_scheduling_order(times in prop::collection::vec(0u64..20, 0..200)) {
            let mut queue = Queue::new();
            for (i, &t) in times.iter().enumerate() {
                queue.push(Time::ZERO + Duration::from_secs(t), i);
            }
            let mut expected: Vec<(u64, usize)> = times.iter().copied().zip(0..).collect();
            expected.sort();
            let mut out = Vec::new();
            while let Some((time, i)) = queue.pop() {
                out.push((time.since_epoch().as_secs(), i));
            }
            prop_assert_eq!(out, expected);
        }
    }
}
