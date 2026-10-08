//! Tempest's client web UI (D-117): the transfers list, a transfer's detail and its controls,
//! sharing, the trackers and the settings, following the engine's API and event stream. The
//! engine serves it, so every request goes to the page's own origin.
//!
//! The layout is shared with the TUI (`specs/interfaces.md`, D-131, D-136 to D-139). UI logic
//! lives in plain modules with tests (`keys`, `columns`, `present`, `request`, `stream`, `rows`,
//! `edits`, `notify`), and the components stay thin.

mod api;
mod app;
mod areas;
mod columns;
mod config;
mod controls;
mod detail;
mod dialogs;
mod edits;
mod keys;
mod list;
mod notify;
mod present;
mod request;
mod rows;
mod share;
mod signin;
mod state;
mod store;
mod stream;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(app::App);
}
