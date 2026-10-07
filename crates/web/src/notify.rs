//! Telling the user when a transfer completes or fails: a toast on the page, and a browser
//! notification when they switched those on. The switch is this browser's, since the
//! permission it needs is too.

use std::collections::HashMap;

use leptos::prelude::*;
use leptos::task::spawn_local;
use tempest_api::{Status, TransferInfo};
use tempest_protocol::BundleId;
use tempest_ui_kit::components::Tone;
use wasm_bindgen_futures::JsFuture;

use crate::controls::Controls;

/// Where this browser keeps the switch.
const STORAGE_KEY: &str = "tempest.notifications";

/// Something worth telling the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// A download completed.
    Completed {
        /// Its name.
        name: String,
    },
    /// A transfer stopped on an error.
    Failed {
        /// Its name.
        name: String,
        /// Why.
        reason: String,
    },
}

impl Notice {
    /// A title and a sentence.
    pub fn text(&self) -> (String, String) {
        match self {
            Self::Completed { name } => (
                format!("{name} is complete"),
                "Every chunk is on disk and checked.".to_owned(),
            ),
            Self::Failed { name, reason } => (format!("{name} stopped"), reason.clone()),
        }
    }

    fn tone(&self) -> Tone {
        match self {
            Self::Completed { .. } => Tone::Down,
            Self::Failed { .. } => Tone::Bad,
        }
    }
}

/// What a transfer's notices depend on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seen {
    complete: bool,
    errored: bool,
}

impl Seen {
    /// What `transfer` shows now.
    pub fn of(transfer: &TransferInfo) -> Self {
        Self {
            complete: transfer.completed.is_some(),
            errored: matches!(transfer.status, Status::Errored { .. }),
        }
    }
}

/// What changed between `before` and `after` that is worth a notice. Transfers not seen
/// before, such as everything when the page opens, give none.
pub fn notices(before: &HashMap<BundleId, Seen>, after: &[TransferInfo]) -> Vec<Notice> {
    let mut notices = Vec::new();
    for transfer in after {
        let Some(was) = before.get(&transfer.id) else {
            continue;
        };
        let now = Seen::of(transfer);
        if now.complete && !was.complete {
            notices.push(Notice::Completed {
                name: transfer.name.clone(),
            });
        }
        if let Status::Errored { reason } = &transfer.status
            && !was.errored
        {
            notices.push(Notice::Failed {
                name: transfer.name.clone(),
                reason: reason.clone(),
            });
        }
    }
    notices
}

/// Whether this browser can show notifications at all.
pub fn supported() -> bool {
    js_sys::Reflect::has(&window(), &"Notification".into()).unwrap_or(false)
}

/// Whether the user switched notifications on in this browser, and still allows them.
pub fn enabled() -> bool {
    let stored = window()
        .local_storage()
        .ok()
        .flatten()
        .and_then(|s| s.get_item(STORAGE_KEY).ok().flatten());
    supported()
        && stored.as_deref() == Some("on")
        && web_sys::Notification::permission() == web_sys::NotificationPermission::Granted
}

fn remember(on: bool) {
    if let Some(storage) = window().local_storage().ok().flatten() {
        let _ = storage.set_item(STORAGE_KEY, if on { "on" } else { "off" });
    }
}

/// Switches notifications on or off, asking the browser's permission the first time, and sets
/// `switch` to the outcome.
pub fn switch(on: bool, switch: RwSignal<bool>, controls: Controls) {
    if !on {
        remember(false);
        switch.set(false);
        return;
    }
    if !supported() {
        controls.say(Tone::Bad, "This browser can't show notifications.");
        return;
    }
    spawn_local(async move {
        let granted = match web_sys::Notification::request_permission() {
            Ok(promise) => JsFuture::from(promise)
                .await
                .ok()
                .and_then(|p| p.as_string())
                .is_some_and(|p| p == "granted"),
            Err(_) => false,
        };
        remember(granted);
        switch.set(granted);
        match granted {
            true => controls.say(
                Tone::Neutral,
                "Notifications on: you'll hear when a transfer completes or fails.",
            ),
            false => controls.say(
                Tone::Bad,
                "The browser blocked notifications. Allow them in the site's settings.",
            ),
        }
    });
}

/// Tells the user about `notice`: a toast, and a notification when `browser` is on.
pub fn tell(notice: &Notice, browser: bool, controls: Controls) {
    let (title, body) = notice.text();
    controls.say(notice.tone(), format!("{title}. {body}"));
    if browser {
        let options = web_sys::NotificationOptions::new();
        options.set_body(&body);
        options.set_tag(&title);
        let _ = web_sys::Notification::new_with_options(&title, &options);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completing_and_failing_are_noticed_once() {
        let mut download = TransferInfo::example(1, "film", 0);
        download.status = Status::Downloading;
        let mut other = TransferInfo::example(2, "music", 1);
        other.status = Status::Downloading;
        let before: HashMap<_, _> = [&download, &other]
            .iter()
            .map(|t| (t.id, Seen::of(t)))
            .collect();

        let mut done = download.clone();
        done.status = Status::Sharing;
        done.completed = Some(1);
        let mut failed = other.clone();
        failed.status = Status::Errored {
            reason: "The disk is full.".to_owned(),
        };
        let new = TransferInfo::example(3, "new", 2);
        let after = [done.clone(), failed.clone(), new];
        assert_eq!(
            notices(&before, &after),
            [
                Notice::Completed {
                    name: "film".to_owned()
                },
                Notice::Failed {
                    name: "music".to_owned(),
                    reason: "The disk is full.".to_owned()
                }
            ]
        );
        // Nothing new: nothing to say.
        let seen: HashMap<_, _> = after.iter().map(|t| (t.id, Seen::of(t))).collect();
        assert!(notices(&seen, &after).is_empty());
        // The first look says nothing.
        assert!(notices(&HashMap::new(), &after).is_empty());
    }
}
