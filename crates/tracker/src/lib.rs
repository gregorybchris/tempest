//! Tempest's tracker: the peer-list service of `protocol/trackers.md`.
//!
//! - [`swarms`]: who is in each swarm, and the rules for announces, without I/O.
//! - [`server`]: the HTTP endpoints over those swarms.
//! - [`catalog`]: the optional catalog of bundles their sharers chose to list, with accounts,
//!   votes and comments, served beside the peer-list routes.
//! - [`limits`]: the catalog's rate limits per client IP.
//! - [`fetcher`]: fetches pending listings' manifests from their swarms, which lists them.
//! - [`http`]: serving the routes with a connection cap and timeouts.
//!
//! The tracker serves plain HTTP. HTTPS comes from a reverse proxy in front of it (see
//! [`Tracker::behind_proxy`]).

pub mod catalog;
pub mod fetcher;
pub mod http;
pub mod limits;
pub mod server;
pub mod swarms;

pub use catalog::{Catalog, CatalogConfig};
pub use fetcher::FetcherConfig;
pub use http::HttpLimits;
pub use limits::{CatalogLimits, Rate};
pub use server::Tracker;
pub use swarms::{Refusal, Swarms, TrackerConfig};

/// The tracker's default TCP port (D-126).
pub const DEFAULT_PORT: u16 = 4777;
