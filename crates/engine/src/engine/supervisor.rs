//! A supervisor per running bundle.

use tempest_core::Event;
use tempest_net::{AnnounceConfig, Announcer, BundleHandle, TrackerClient};
use tempest_protocol::BundleId;
use tempest_protocol::tracker::ListingRequest;
use tokio::sync::broadcast::error::{RecvError, TryRecvError};
use tokio::sync::{mpsc, oneshot, watch};

use super::{ANNOUNCE_STOP_TIMEOUT, Internal};

/// Waits until `stop` is set or its sender is dropped.
async fn stopped(stop: &mut watch::Receiver<bool>) {
    let _ = stop.wait_for(|stop| *stop).await;
}

/// A change to the trackers a running bundle is published on.
#[derive(Debug)]
pub(super) enum ListingChange {
    /// Announce there with `listing` from now on.
    Publish(String),
    /// Announce there without it, or stop announcing there unless `keep`. `settled` is told
    /// once no announce with the listing can still reach the tracker.
    Unpublish {
        tracker: String,
        keep: bool,
        settled: oneshot::Sender<()>,
    },
}

/// Owns a running bundle: forwards its decision events, takes checkpoints when asked, has
/// trackers list it while it is published, stops it when told, and reports how it ended.
pub(super) struct Supervisor {
    pub id: BundleId,
    pub generation: u64,
    pub handle: BundleHandle,
    pub announcers: Vec<Announcer>,
    /// For announcing to trackers it is published on from now on.
    pub tracker: TrackerClient,
    pub announce: AnnounceConfig,
    /// The bundle's listing request.
    pub listing: ListingRequest,
    pub internal: mpsc::Sender<Internal>,
}

impl Supervisor {
    pub async fn run(
        mut self,
        mut stop: watch::Receiver<bool>,
        mut checkpoint: mpsc::Receiver<()>,
        mut listing: mpsc::UnboundedReceiver<ListingChange>,
    ) {
        let mut status = self.handle.status.clone();
        let mut events_open = true;
        loop {
            tokio::select! {
                // A dropped sender means the engine let go of the transfer.
                () = stopped(&mut stop) => {
                    self.handle.shutdown();
                    break;
                }
                Some(()) = checkpoint.recv() => self.checkpoint().await,
                Some(change) = listing.recv() => match change {
                    ListingChange::Publish(tracker) => self.publish(tracker),
                    ListingChange::Unpublish { tracker, keep, settled } => {
                        self.unpublish(&tracker, keep, settled);
                    }
                },
                event = self.handle.events.recv(), if events_open => match event {
                    Ok(event) => self.forward(event).await,
                    Err(RecvError::Lagged(missed)) => {
                        let message = Internal::Decisions {
                            id: self.id,
                            events: Vec::new(),
                            missed,
                        };
                        let _ = self.internal.send(message).await;
                    }
                    Err(RecvError::Closed) => events_open = false,
                },
                // The bundle stopped by itself: done lingering, or storage failed.
                changed = status.changed() => if changed.is_err() {
                    break;
                },
            }
        }
        let stopping: tokio::task::JoinSet<()> =
            self.announcers.into_iter().map(Announcer::stop).collect();
        let _ = tokio::time::timeout(ANNOUNCE_STOP_TIMEOUT, stopping.join_all()).await;
        let result = self.handle.finished().await;
        let _ = self
            .internal
            .send(Internal::Finished {
                id: self.id,
                generation: self.generation,
                result,
            })
            .await;
    }

    /// Forwards `first` and every event already waiting after it, in one message.
    async fn forward(&mut self, first: Event) {
        let mut events = vec![first];
        let mut missed = 0;
        loop {
            match self.handle.events.try_recv() {
                Ok(event) => events.push(event),
                Err(TryRecvError::Lagged(count)) => missed += count,
                Err(TryRecvError::Empty | TryRecvError::Closed) => break,
            }
        }
        let message = Internal::Decisions {
            id: self.id,
            events,
            missed,
        };
        let _ = self.internal.send(message).await;
    }

    /// Announces with `listing` to `tracker`, starting to announce there if it didn't.
    fn publish(&mut self, tracker: String) {
        match self.announcers.iter().find(|a| a.tracker() == tracker) {
            Some(announcer) => announcer.set_listing(Some(self.listing)),
            None => self.announcers.push(Announcer::spawn(
                self.tracker.clone(),
                tracker,
                self.announce.clone(),
                &self.handle,
                Some(self.listing),
            )),
        }
    }

    /// Announces to `tracker` without `listing`, or stops announcing there unless `keep`, then
    /// tells `settled`.
    fn unpublish(&mut self, tracker: &str, keep: bool, settled: oneshot::Sender<()>) {
        let Some(index) = self.announcers.iter().position(|a| a.tracker() == tracker) else {
            let _ = settled.send(());
            return;
        };
        // Even `stopped` would carry the listing otherwise.
        self.announcers[index].set_listing(None);
        let done: std::pin::Pin<Box<dyn Future<Output = ()> + Send>> = match keep {
            true => Box::pin(self.announcers[index].listing_settled()),
            false => Box::pin(self.announcers.remove(index).stop()),
        };
        tokio::spawn(async move {
            done.await;
            let _ = settled.send(());
        });
    }

    async fn checkpoint(&self) {
        let Some(storage) = self.handle.storage().await else {
            return;
        };
        if let Ok(checkpoint) = storage.checkpoint().await {
            let message = Internal::Checkpoint {
                id: self.id,
                generation: self.generation,
                checkpoint,
            };
            let _ = self.internal.send(message).await;
        }
    }
}
