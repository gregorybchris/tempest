//! Telling the user when a transfer completes or fails: a toast on the page, and a browser
//! notification when they switched those on. The switch is this browser's, since the
//! permission it needs is too.

use leptos::prelude::*;
use leptos::task::spawn_local;
pub use tempest_api::notices::{Notice, Seen, notices};
use tempest_ui_kit::components::Tone;
use wasm_bindgen_futures::JsFuture;

use crate::controls::Controls;

/// Where this browser keeps the switch.
const STORAGE_KEY: &str = "tempest.notifications";

/// How `notice` looks as a toast.
fn tone(notice: &Notice) -> Tone {
    match notice {
        Notice::Completed { .. } => Tone::Down,
        Notice::Failed { .. } => Tone::Bad,
    }
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
                "Notifications on. You will be told when a transfer completes or fails.",
            ),
            false => controls.say(
                Tone::Bad,
                "Notifications are blocked. Allow them for this site in the browser's settings.",
            ),
        }
    });
}

/// Tells the user about `notice`: a toast, and a notification when `browser` is on.
pub fn tell(notice: &Notice, browser: bool, controls: Controls) {
    let (title, body) = notice.text();
    controls.say(tone(notice), format!("{title}. {body}"));
    if browser {
        let options = web_sys::NotificationOptions::new();
        options.set_body(&body);
        options.set_tag(&title);
        let _ = web_sys::Notification::new_with_options(&title, &options);
    }
}
