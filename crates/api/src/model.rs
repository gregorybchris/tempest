//! What a client knows about the engine, kept up to date from its event stream.

use std::collections::{HashMap, VecDeque};

use tempest_protocol::BundleId;

use crate::TransferInfo;
use crate::events::{ApiEvent, DecisionInfo};

/// How many decision events are kept per transfer.
pub const DECISIONS_KEPT: usize = 1000;

/// How many rate samples are kept: one a second, for ten minutes.
pub const SAMPLES_KEPT: usize = 600;

/// Whether the client is following the engine's events.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Connection {
    /// Opening the event stream.
    #[default]
    Connecting,
    /// Following the event stream.
    Live,
    /// The stream broke off, and the client is trying again.
    Lost {
        /// Why, in a sentence.
        error: String,
    },
}

/// Download and upload rates over time, oldest first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct History {
    samples: VecDeque<(u64, u64)>,
}

impl History {
    fn push(&mut self, down: u64, up: u64) {
        if self.samples.len() == SAMPLES_KEPT {
            self.samples.pop_front();
        }
        self.samples.push_back((down, up));
    }

    /// The samples, oldest first, as (download, upload) in bytes per second.
    pub fn samples(&self) -> impl ExactSizeIterator<Item = (u64, u64)> + '_ {
        self.samples.iter().copied()
    }
}

/// A transfer's decision events, newest last.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Decisions {
    /// The events kept.
    pub events: VecDeque<DecisionInfo>,
    /// How many were dropped, by the engine because they came too fast or here because there
    /// were too many to keep.
    pub missed: u64,
}

/// The engine's transfers and what has been seen of them: their decision events, and their
/// rates sampled once a second (D-194).
#[derive(Debug, Clone, Default)]
pub struct Model {
    /// Every transfer, in queue order.
    transfers: Vec<TransferInfo>,
    decisions: HashMap<BundleId, Decisions>,
    histories: HashMap<BundleId, History>,
    total: History,
    /// Whether the event stream is being followed.
    pub connection: Connection,
}

impl Model {
    /// A model that has seen nothing yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Every transfer, in queue order.
    pub fn transfers(&self) -> &[TransferInfo] {
        &self.transfers
    }

    /// The transfer `id`, if the engine has it.
    pub fn transfer(&self, id: &BundleId) -> Option<&TransferInfo> {
        self.transfers.iter().find(|transfer| transfer.id == *id)
    }

    /// The decision events seen for `id`.
    pub fn decisions(&self, id: &BundleId) -> Option<&Decisions> {
        self.decisions.get(id)
    }

    /// The rates of `id` over time.
    pub fn history(&self, id: &BundleId) -> Option<&History> {
        self.histories.get(id)
    }

    /// The rates of every transfer together over time.
    pub fn total_history(&self) -> &History {
        &self.total
    }

    /// Every transfer's download and upload rates added up.
    pub fn total_rates(&self) -> (u64, u64) {
        self.transfers.iter().fold((0, 0), |(down, up), transfer| {
            (down + transfer.download_rate, up + transfer.upload_rate)
        })
    }

    /// Takes in an event from the engine.
    pub fn apply(&mut self, event: ApiEvent) {
        match event {
            ApiEvent::Snapshot(transfers) | ApiEvent::Reset(transfers) => {
                self.transfers = transfers;
                self.sort();
                let ids: Vec<BundleId> = self.transfers.iter().map(|t| t.id).collect();
                self.decisions.retain(|id, _| ids.contains(id));
                self.histories.retain(|id, _| ids.contains(id));
            }
            ApiEvent::Transfer(transfer) => {
                match self.transfers.iter_mut().find(|t| t.id == transfer.id) {
                    Some(known) => *known = *transfer,
                    None => self.transfers.push(*transfer),
                }
                self.sort();
            }
            ApiEvent::Removed(removed) => {
                self.transfers.retain(|t| t.id != removed.id);
                self.decisions.remove(&removed.id);
                self.histories.remove(&removed.id);
            }
            ApiEvent::Decision(decision) => {
                let decisions = self.decisions.entry(decision.id).or_default();
                if decisions.events.len() == DECISIONS_KEPT {
                    decisions.events.pop_front();
                    decisions.missed += 1;
                }
                decisions.events.push_back(decision);
            }
            ApiEvent::DecisionsMissed(missed) => {
                self.decisions.entry(missed.id).or_default().missed += missed.count;
            }
        }
    }

    /// Records every transfer's current rates; called once a second.
    pub fn sample(&mut self) {
        for transfer in &self.transfers {
            self.histories
                .entry(transfer.id)
                .or_default()
                .push(transfer.download_rate, transfer.upload_rate);
        }
        let (down, up) = self.total_rates();
        self.total.push(down, up);
    }

    fn sort(&mut self) {
        self.transfers
            .sort_by_key(|transfer| transfer.queue_position);
    }
}

#[cfg(test)]
mod tests {
    use tempest_protocol::Hash;

    use super::*;
    use crate::events::{MissedInfo, RemovedInfo};

    fn transfer(seed: u8, name: &str, position: u32) -> TransferInfo {
        TransferInfo::example(seed, name, position)
    }

    fn decision(seed: u8, text: &str) -> ApiEvent {
        ApiEvent::Decision(DecisionInfo {
            id: Hash::new([seed; 32]),
            time: 0.0,
            kind: "requested".to_owned(),
            reason: None,
            text: text.to_owned(),
        })
    }

    fn names(model: &Model) -> Vec<&str> {
        model.transfers().iter().map(|t| t.name.as_str()).collect()
    }

    #[test]
    fn transfers_follow_snapshots_changes_and_removals_in_queue_order() {
        let mut model = Model::new();
        model.apply(ApiEvent::Snapshot(vec![
            transfer(2, "b", 1),
            transfer(1, "a", 0),
        ]));
        assert_eq!(names(&model), ["a", "b"]);

        let mut moved = transfer(1, "a", 2);
        moved.download_rate = 10;
        model.apply(ApiEvent::Transfer(Box::new(moved)));
        model.apply(ApiEvent::Transfer(Box::new(transfer(3, "c", 0))));
        assert_eq!(names(&model), ["c", "b", "a"]);
        assert_eq!(model.total_rates(), (10, 0));

        model.apply(ApiEvent::Removed(RemovedInfo {
            id: Hash::new([2; 32]),
        }));
        assert_eq!(names(&model), ["c", "a"]);
        model.apply(ApiEvent::Reset(vec![transfer(4, "d", 0)]));
        assert_eq!(names(&model), ["d"]);
    }

    #[test]
    fn decisions_are_kept_per_transfer_up_to_a_limit() {
        let mut model = Model::new();
        model.apply(ApiEvent::Snapshot(vec![transfer(1, "a", 0)]));
        for n in 0..DECISIONS_KEPT + 5 {
            model.apply(decision(1, &n.to_string()));
        }
        model.apply(ApiEvent::DecisionsMissed(MissedInfo {
            id: Hash::new([1; 32]),
            count: 3,
        }));
        let decisions = model.decisions(&Hash::new([1; 32])).unwrap();
        assert_eq!(decisions.events.len(), DECISIONS_KEPT);
        assert_eq!(decisions.events.front().unwrap().text, "5");
        assert_eq!(decisions.missed, 8);

        // What belongs to transfers that are gone goes with them.
        model.apply(ApiEvent::Snapshot(vec![]));
        assert!(model.decisions(&Hash::new([1; 32])).is_none());
    }

    #[test]
    fn samples_keep_the_last_ten_minutes() {
        let mut model = Model::new();
        let mut busy = transfer(1, "a", 0);
        busy.download_rate = 5;
        busy.upload_rate = 2;
        model.apply(ApiEvent::Snapshot(vec![busy, transfer(2, "b", 1)]));
        for _ in 0..SAMPLES_KEPT + 1 {
            model.sample();
        }
        let history = model.history(&Hash::new([1; 32])).unwrap();
        assert_eq!(history.samples().len(), SAMPLES_KEPT);
        assert_eq!(history.samples().last(), Some((5, 2)));
        assert_eq!(model.total_history().samples().last(), Some((5, 2)));
    }
}
