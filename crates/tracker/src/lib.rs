//! Tempest's tracker: the peer-list service of `protocol/trackers.md`.
//!
//! - [`swarms`]: who is in each swarm, and the rules for announces, without I/O.
//! - [`server`]: the HTTP endpoints over those swarms.
//!
//! The tracker serves plain HTTP. HTTPS comes from a reverse proxy in front of it (see
//! [`Tracker::behind_proxy`]). A catalog of bundles can be added beside the peer-list routes.

pub mod server;
pub mod swarms;

pub use server::Tracker;
pub use swarms::{Refusal, Swarms, TrackerConfig};

/// The tracker's default TCP port (D-126).
pub const DEFAULT_PORT: u16 = 4777;
