//! Tempest's simulation results UI (`specs/simulation.md`): experiments and their runs,
//! comparisons of variants with confidence intervals, and checks against the golden
//! baselines, read from the results API that `tempest-sim web` serves beside it.
//!
//! UI logic lives in plain modules with tests (`route`, `present`, `keys`, `request`), and the
//! components stay thin, as the Leptos guidelines in `specs/interfaces.md` ask.

mod app;
mod baselines;
mod compare;
mod experiments;
mod keys;
mod present;
mod request;
mod route;
mod runs;
mod views;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(app::App);
}
