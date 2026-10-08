//! Tempest's tracker website (`specs/interfaces.md`): search the bundles a tracker's users
//! chose to list, narrow by category and tag, sort by relevance, health, votes or downloads,
//! and, signed in with a Tempest key, vote, comment and edit one's own listings. The tracker
//! serves it beside the API it reads.
//!
//! UI logic lives in plain modules with tests (`route`, `present`, `keys`, `request`), and the
//! components stay thin, as the Leptos guidelines in `specs/interfaces.md` ask.

mod app;
mod browse;
mod edit;
mod keys;
mod listing;
mod me;
mod present;
mod request;
mod route;
mod signin;
mod views;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(app::App);
}
