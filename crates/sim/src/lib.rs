//! Tempest's discrete-event simulator (`specs/simulation.md`).
//!
//! The simulator drives the same sans-IO [`tempest_core::Session`] as the real client, one per
//! simulated peer, over a simulated network in simulated time (D-068). Runs are deterministic:
//! every random choice comes from generators seeded by the run's seed, and nothing in a run reads
//! the clock or iterates a hash map, so the same seed replays the same event log.
//!
//! - [`scenario`]: scenario files, which describe the world and the experiment (D-072).
//! - [`engine`]: one run of one variant with one seed.
//! - [`network`]: the message-level network model (D-069).
//! - [`experiment`]: every variant with every seed.
//! - [`summary`]: results per variant.
//! - [`log`]: the event log and its digest.

pub mod bundle;
pub mod engine;
pub mod experiment;
pub mod log;
pub mod network;
pub mod queue;
pub mod scenario;
pub mod summary;
pub mod units;

pub use engine::{PeerOutcome, RunOptions, RunResult, run};
pub use scenario::{Scenario, ScenarioError, Variant};
