//! Transfer controls: each sends one request and says how it went in a toast. Requests return
//! no state of their own; the change shows when its event arrives (D-189).

use std::future::Future;
use std::path::Path;

use leptos::prelude::*;
use leptos::task::spawn_local;
use tempest_api::requests::{QueuePlace, QueueStep};
use tempest_api::{Status, TransferInfo};
use tempest_ui_kit::components::{Toasts, Tone};
use wasm_bindgen_futures::JsFuture;

use crate::api;
use crate::request::RequestError;

/// Sends transfer commands. It is `Copy`.
#[derive(Clone, Copy)]
pub struct Controls {
    toasts: Toasts,
}

impl Controls {
    /// Controls that report to `toasts`.
    pub fn new(toasts: Toasts) -> Self {
        Self { toasts }
    }

    /// Runs `request`, then says `done`, or what went wrong.
    pub fn run<T>(
        self,
        request: impl Future<Output = Result<T, RequestError>> + 'static,
        done: String,
    ) {
        spawn_local(async move {
            match request.await {
                Ok(_) => self.toasts.push(Tone::Neutral, done),
                Err(error) => self.toasts.push(Tone::Bad, error.to_string()),
            }
        });
    }

    /// Says `text`.
    pub fn say(self, tone: Tone, text: impl Into<String>) {
        self.toasts.push(tone, text);
    }

    /// Stops a transfer that runs or waits to, and starts one that doesn't.
    pub fn start_stop(self, transfer: &TransferInfo) {
        let (id, name) = (transfer.id, transfer.name.clone());
        if runs(&transfer.status) {
            self.run(api::stop(id), format!("Stopped {name}"));
        } else {
            self.run(api::start(id), format!("Started {name}"));
        }
    }

    /// Starts a transfer regardless of the queue.
    pub fn force_start(self, transfer: &TransferInfo) {
        let name = transfer.name.clone();
        self.run(
            api::force_start(transfer.id),
            format!("Started {name}, ignoring the queue"),
        );
    }

    /// Checks a transfer's data again.
    pub fn recheck(self, transfer: &TransferInfo) {
        let name = transfer.name.clone();
        self.run(api::recheck(transfer.id), format!("Checking {name}"));
    }

    /// Moves a transfer in the queue.
    pub fn queue(self, transfer: &TransferInfo, step: QueueStep) {
        let name = transfer.name.clone();
        let done = match step {
            QueueStep::Top => format!("{name} is first in the queue"),
            QueueStep::Up => format!("Moved {name} up the queue"),
            QueueStep::Down => format!("Moved {name} down the queue"),
            QueueStep::Bottom => format!("{name} is last in the queue"),
        };
        self.run(api::queue(transfer.id, QueuePlace::Step(step)), done);
    }

    /// Shows a transfer's files in the file manager of the engine's machine.
    pub fn reveal(self, transfer: &TransferInfo) {
        let name = transfer.name.clone();
        self.run(
            api::reveal(transfer.id),
            format!("Showed {name} in the file manager"),
        );
    }

    /// Copies the path of the folder a transfer's files are in.
    pub fn copy_path(self, transfer: &TransferInfo) {
        self.copy_as(folder_of(transfer).display().to_string(), "path");
    }

    /// Copies a transfer's link.
    pub fn copy_link(self, transfer: &TransferInfo) {
        self.copy(transfer.link.clone());
    }

    /// Copies `link`.
    pub fn copy(self, link: String) {
        self.copy_as(link, "link");
    }

    /// Copies `text`, calling it `what` in the toast.
    pub fn copy_as(self, text: String, what: &'static str) {
        spawn_local(async move {
            let promise = window().navigator().clipboard().write_text(&text);
            match JsFuture::from(promise).await.is_ok() {
                true => self.say(Tone::Neutral, format!("Copied the {what}")),
                false => self.say(Tone::Bad, format!("The browser wouldn't copy the {what}.")),
            }
        });
    }
}

/// The folder a transfer's files are in, or will be in.
pub fn folder_of(transfer: &TransferInfo) -> &Path {
    transfer
        .content_dir
        .as_deref()
        .unwrap_or(&transfer.save_folder)
}

/// Whether a transfer runs or waits to, so that its control stops it.
pub fn runs(status: &Status) -> bool {
    matches!(
        status,
        Status::Downloading
            | Status::Sharing
            | Status::Stalled { .. }
            | Status::Queued
            | Status::Checking
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_or_waiting_transfers_stop_and_others_start() {
        assert!(runs(&Status::Downloading));
        assert!(runs(&Status::Queued));
        assert!(!runs(&Status::Stopped));
        assert!(!runs(&Status::Completed));
        assert!(!runs(&Status::Errored {
            reason: String::new()
        }));
    }
}
