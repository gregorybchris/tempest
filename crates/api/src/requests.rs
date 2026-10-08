//! The API's routes' request bodies and the engine's description (`specs/api.md`).

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tempest_catalog_api::listing::ListingEdit;

/// The prefix of every route: the API's version.
pub const PREFIX: &str = "/v1";

/// `GET /v1/engine`: what the engine is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineInfo {
    /// The engine's version.
    pub version: String,
    /// This peer's ID.
    pub peer_id: String,
    /// The direct addresses this peer advertises.
    pub addresses: Vec<String>,
    /// The home relay: `off`, `connecting`, `registered` or `unavailable`.
    pub relay: String,
    /// The relay address this peer advertises, while registered with a home relay.
    pub relay_address: Option<String>,
    /// Why no relay would register this peer, while unavailable.
    pub relay_errors: Vec<String>,
    /// Port mapping: `off`, `mapping`, `mapped` or `unavailable`.
    pub port_mapping: String,
    /// The external address the router forwards to this peer, while mapped.
    pub mapped_address: Option<String>,
    /// Why no router would map the port, while unavailable.
    pub port_mapping_errors: Vec<String>,
}

/// `POST /v1/transfers`: a download from a link, or a share of a local file or folder.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AddRequest {
    /// A `tempest:` link to download. Exactly one of `link` and `path` is given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// An absolute path to share.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// A share's bundle name. Defaults to the file or folder's name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A share's description.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub comment: String,
    /// Where a download's bundle folder is made. Defaults to the category's folder, then the
    /// engine's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save_folder: Option<PathBuf>,
    /// The transfer's category.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// The transfer's tags.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub tags: BTreeSet<String>,
    /// Add a download stopped.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stopped: bool,
}

/// `PUT /v1/transfers/{id}/name`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NameRequest {
    /// The name to show, or `null` for the bundle's own.
    pub name: Option<String>,
}

/// `PUT /v1/transfers/{id}/tags`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TagsRequest {
    /// Every tag.
    pub tags: BTreeSet<String>,
}

/// `PUT /v1/transfers/{id}/category`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CategoryRequest {
    /// The category, or `null` for none.
    pub category: Option<String>,
}

/// `POST /v1/transfers/{id}/move`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoveRequest {
    /// The absolute path of the folder to move the files into.
    pub save_folder: PathBuf,
}

/// `POST /v1/transfers/{id}/queue`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueueRequest {
    /// `top`, `up`, `down`, `bottom`, or a place counting from 0.
    pub to: QueuePlace,
}

/// Where to move a transfer in the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum QueuePlace {
    /// A place, counting from 0.
    Place(u32),
    /// `top`, `up`, `down` or `bottom`.
    Step(QueueStep),
}

/// A move relative to the queue or the transfer's place in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueStep {
    /// First.
    Top,
    /// One place earlier.
    Up,
    /// One place later.
    Down,
    /// Last.
    Bottom,
}

/// A category, as `GET /v1/categories` lists it and `PUT /v1/categories/{name}` sets it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Category {
    /// Where downloads added in the category go.
    pub save_folder: PathBuf,
}

/// `POST /v1/transfers/{id}/publish`: list the transfer on a tracker's catalog (D-129).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishRequest {
    /// The tracker's URL.
    pub tracker: String,
    /// The listing's title, description, category and tags; fields left out stay as they are.
    #[serde(default)]
    pub listing: ListingEdit,
    /// An invite, for this peer's first sign-in to a catalog that requires one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite: Option<String>,
}

/// `POST /v1/catalog/sign-in`: sign this peer's key in to a tracker's catalog for whoever
/// asked for `code`, such as a browser.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogSignInRequest {
    /// The tracker's URL.
    pub tracker: String,
    /// The sign-in code the catalog showed.
    pub code: String,
    /// An invite, for this peer's first sign-in to a catalog that requires one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite: Option<String>,
}

/// `POST /v1/session`: signs in to a remote engine with its access token (D-222).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignInRequest {
    /// The engine's access token.
    pub token: String,
}

/// `GET /v1/session` and `POST /v1/session`: whether the engine wants a sign-in, and whether
/// the request has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    /// Whether requests need the access token or a session. A local engine doesn't.
    pub required: bool,
    /// Whether this request may use the API.
    pub signed_in: bool,
}
