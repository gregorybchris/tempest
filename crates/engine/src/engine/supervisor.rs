//! A supervisor per running bundle.

use tempest_core::Event;
use tempest_net::{Announcer, BundleHandle};
use tempest_protocol::BundleId;
use tokio::sync::broadcast::error::{RecvError, TryRecvError};
use tokio::sync::{mpsc, watch};

use super::{ANNOUNCE_STOP_TIMEOUT, Internal};

/// Waits until `stop` is set or its sender is dropped.
async fn stopped(stop: &mut watch::Receiver<bool>) {
    let _ = stop.wait_for(|stop| *stop).await;
}

/// Owns a running bundle: forwards its decision events, takes checkpoints when asked, stops it
/// when told, and reports how it ended.
pub(super) struct Supervisor {
    pub id: BundleId,
    pub generation: u64,
    pub handle: BundleHandle,
    pub announcers: Vec<Announcer>,
    pub internal: mpsc::Sender<Internal>,
}

impl Supervisor {
    pub async fn run(
        mut self,
        mut stop: watch::Receiver<bool>,
        mut checkpoint: mpsc::Receiver<()>,
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
