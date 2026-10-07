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
//! - [`distribution`]: random draws for arrivals and session lengths.
//! - [`experiment`]: every variant with every seed.
//! - [`summary`]: each run's headline metrics, and results per variant.
//! - [`stats`]: medians with confidence intervals, and paired comparisons of variants (D-073).
//! - [`strategic`]: the strategic client's upload strategy.
//! - [`log`]: the event log and its digest.
//! - [`baseline`]: golden scenarios and their baselines, to check runs against (D-077).
//! - `results`: the DuckDB results database (D-075), with the `results` feature.
//! - `browse`: what the results UI reads from that database, with the `results` feature.

pub mod baseline;
#[cfg(feature = "results")]
pub mod browse;
pub mod bundle;
pub mod distribution;
pub mod engine;
pub mod experiment;
pub mod log;
pub mod network;
pub mod queue;
#[cfg(feature = "results")]
pub mod results;
pub mod scenario;
pub mod stats;
pub mod strategic;
pub mod summary;
pub mod units;

pub use engine::{PeerOutcome, RunOptions, RunResult, run, run_with_sink};
pub use scenario::{Scenario, ScenarioError, Variant};
