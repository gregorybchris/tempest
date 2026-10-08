//! What the page knows about the engine: the shared [`Model`] fed by the event stream, and a
//! signal per transfer for the list's rows.
//!
//! Events arrive in bursts, one per changed transfer every half second, so they are applied
//! together a moment later. The rows follow the Leptos guidelines in `specs/interfaces.md`:
//! after each batch one pass copies changed transfers into their own signals, so a row redraws
//! only when its transfer changed, and no row subscribes to the shared model.

use std::collections::HashMap;
use std::time::Duration;

use leptos::prelude::*;
use tempest_api::model::Model;
use tempest_api::{ApiEvent, TransferInfo};
use tempest_protocol::BundleId;

use crate::notify::{self, Notice, Seen};
use crate::stream::Link;

/// How long events wait to be applied together.
const BATCH: Duration = Duration::from_millis(100);

/// The page's view of the engine. It is `Copy`; every copy is the same store.
#[derive(Clone, Copy)]
pub struct Store {
    /// Every transfer, their decision events and their rates over time.
    pub model: RwSignal<Model>,
    /// Whether the event stream is being followed.
    pub link: RwSignal<Link>,
    /// Whether the engine has sent its transfers since the page opened.
    pub loaded: RwSignal<bool>,
    /// What the last batch of events had worth telling the user, if anything.
    pub notices: RwSignal<Vec<Notice>>,
    /// Fires after each second's rates are sampled. Only the charts track it: sampling
    /// changes the model's histories, not its transfers, so it doesn't notify the model.
    pub sampled: Trigger,
    rows: StoredValue<HashMap<BundleId, RwSignal<TransferInfo>>>,
    pending: StoredValue<Vec<ApiEvent>>,
}

impl Store {
    /// A store that has seen nothing yet.
    pub fn new() -> Self {
        Self {
            model: RwSignal::new(Model::new()),
            link: RwSignal::new(Link::Connecting),
            loaded: RwSignal::new(false),
            notices: RwSignal::new(Vec::new()),
            sampled: Trigger::new(),
            rows: StoredValue::new(HashMap::new()),
            pending: StoredValue::new(Vec::new()),
        }
    }

    /// Takes in an event, to be applied with the others that arrive in the next moment.
    pub fn push(self, event: ApiEvent) {
        let first = self.pending.try_update_value(|pending| {
            pending.push(event);
            pending.len() == 1
        });
        if first == Some(true) {
            set_timeout(move || self.flush(), BATCH);
        }
    }

    fn flush(self) {
        let Some(events) = self.pending.try_update_value(std::mem::take) else {
            // The page is gone.
            return;
        };
        let snapshot = events
            .iter()
            .any(|e| matches!(e, ApiEvent::Snapshot(_) | ApiEvent::Reset(_)));
        // Rows are ready before anything that reads the model hears of the change.
        let mut before = HashMap::new();
        let mut notices = Vec::new();
        self.model.update_untracked(|model| {
            before.extend(model.transfers().iter().map(|t| (t.id, Seen::of(t))));
            for event in events {
                model.apply(event);
            }
            notices = notify::notices(&before, model.transfers());
        });
        self.sync_rows();
        self.model.notify();
        if !notices.is_empty() {
            self.notices.set(notices);
        }
        if snapshot && !self.loaded.get_untracked() {
            self.loaded.set(true);
        }
    }

    /// Records every transfer's rates; called once a second (D-194).
    pub fn sample(self) {
        if self.model.try_update_untracked(Model::sample).is_some() {
            self.sampled.notify();
        }
    }

    /// The signal of transfer `id`'s row, while the engine has it.
    pub fn row(self, id: BundleId) -> Option<RwSignal<TransferInfo>> {
        self.rows.with_value(|rows| rows.get(&id).copied())
    }

    /// Copies each changed transfer into its row's signal, making signals for new transfers
    /// and dropping those of removed ones.
    fn sync_rows(self) {
        self.model.with_untracked(|model| {
            self.rows.update_value(|rows| {
                rows.retain(|id, row| {
                    let kept = model.transfer(id).is_some();
                    if !kept {
                        row.dispose();
                    }
                    kept
                });
                for transfer in model.transfers() {
                    match rows.get(&transfer.id) {
                        Some(row) => {
                            if row.with_untracked(|known| known != transfer) {
                                row.set(transfer.clone());
                            }
                        }
                        None => {
                            rows.insert(transfer.id, RwSignal::new(transfer.clone()));
                        }
                    }
                }
            });
        });
    }
}

impl Default for Store {
    fn default() -> Self {
        Self::new()
    }
}
