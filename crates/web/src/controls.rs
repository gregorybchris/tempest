//! Transfer controls: each sends one request and says how it went in a toast. Requests return
//! no state of their own; the change shows when its event arrives (D-189).

use std::future::Future;

use leptos::prelude::*;
use leptos::task::spawn_local;
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

    /// Copies a transfer's link.
    pub fn copy_link(self, transfer: &TransferInfo) {
        let link = transfer.link.clone();
        spawn_local(async move {
            let promise = window().navigator().clipboard().write_text(&link);
            match JsFuture::from(promise).await.is_ok() {
                true => self.say(Tone::Neutral, "Copied the link"),
                false => self.say(Tone::Bad, "The browser wouldn't copy the link."),
            }
        });
    }
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
