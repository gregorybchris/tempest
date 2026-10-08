//! The tracker catalog's API: what a tracker with a catalog serves under [`PREFIX`], beside the
//! peer-list service of `protocol/trackers.md`.
//!
//! The catalog is an application, not protocol (D-025). The tracker, the client that publishes
//! to it and the catalog's web UI share these types, so they depend on serde and `protocol`
//! only and build for WebAssembly as well as natively.
//!
//! - [`account`]: signing in with a Tempest key (D-128), and accounts.
//! - [`listing`]: listings, votes and comments.
//! - [`query`]: searching, filtering and sorting listings.
//! - [`text`]: the rules for the text people write: titles, descriptions, tags, comments and
//!   names.
//!
//! Errors are `protocol`'s [`ErrorResponse`](tempest_protocol::tracker::ErrorResponse), as the
//! peer-list service sends them, with the codes in [`codes`].

pub mod account;
pub mod listing;
pub mod query;
pub mod text;

use serde::{Deserialize, Serialize};

/// The path prefix of every catalog endpoint.
pub const PREFIX: &str = "/catalog/v1/";

/// The cookie a browser keeps its session token in. Other clients send the token as a bearer
/// token in `Authorization`.
pub const SESSION_COOKIE: &str = "tempest_catalog";

/// The URL of the catalog endpoint at `path` (relative to [`PREFIX`]) on the tracker at
/// `tracker`.
///
/// ```
/// assert_eq!(
///     tempest_catalog_api::url("https://example.org/tracker/", "listings"),
///     "https://example.org/tracker/catalog/v1/listings"
/// );
/// ```
pub fn url(tracker: &str, path: &str) -> String {
    format!("{}{PREFIX}{path}", tracker.trim_end_matches('/'))
}

/// The error codes the catalog adds to the peer-list service's.
pub mod codes {
    /// The request needs a session, and has none or an expired one (401).
    pub const UNAUTHORIZED: &str = "unauthorized";
    /// The session's account may not do this, such as edit another's listing (403).
    pub const FORBIDDEN: &str = "forbidden";
    /// The catalog requires an invite for a key's first sign-in, and none valid came (403).
    pub const INVITE_REQUIRED: &str = "invite_required";
    /// A sign-in's code is unknown, used or expired, or its signature doesn't verify (401).
    pub const SIGN_IN_FAILED: &str = "sign_in_failed";
    /// No such listing, comment or endpoint (404).
    pub const NOT_FOUND: &str = "not_found";
}

/// What a catalog is like: the answer to `GET /catalog/v1/info`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogInfo {
    /// The categories a listing can be in.
    pub categories: Vec<Category>,
    /// Whether a key's first sign-in needs an invite.
    pub invites_required: bool,
}

/// A category listings can be in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Category {
    /// Its identifier in listings and queries, such as `datasets`.
    pub slug: String,
    /// Its name for people, such as `Datasets`.
    pub name: String,
}

impl Category {
    /// A category named `name`, with `slug` as its identifier.
    pub fn new(slug: &str, name: &str) -> Self {
        Self {
            slug: slug.to_owned(),
            name: name.to_owned(),
        }
    }
}

/// The categories a catalog has unless its operator chooses others. They lead with the uses
/// Tempest is for (D-031).
pub fn default_categories() -> Vec<Category> {
    [
        ("datasets", "Datasets"),
        ("models", "Models"),
        ("software", "Software"),
        ("games", "Games"),
        ("media", "Media"),
        ("documents", "Documents"),
        ("other", "Other"),
    ]
    .into_iter()
    .map(|(slug, name)| Category::new(slug, name))
    .collect()
}
