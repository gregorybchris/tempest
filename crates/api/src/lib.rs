//! The engine API's types, and what Tempest's clients make of them.
//!
//! The engine serves these types and its clients read them, so they build for WebAssembly as
//! well as natively: they depend on serde and the protocol's types only.
//!
//! - The API's data: [`TransferInfo`] and its [`Status`], [`inspect`] for what is inside a
//!   transfer, [`requests`] for request bodies, [`events`] for the event stream, [`ApiError`]
//!   and [`ui`] for the clients' shared settings.
//! - What clients make of it, shared by the TUI and the web UI (D-119): [`model`] follows the
//!   event stream, [`finding`] filters and sorts (D-192), [`columns`] decides which columns fit
//!   (D-138), [`explain`] says what a transfer is doing in plain language, and [`prefs`] reads
//!   and writes the `ui` settings.

pub mod columns;
mod error;
pub mod events;
pub mod explain;
pub mod finding;
pub mod inspect;
pub mod model;
pub mod prefs;
pub mod requests;
mod status;
pub mod ui;
pub mod words;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Display;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tempest_protocol::BundleId;

pub use error::ApiError;
pub use events::ApiEvent;
pub use requests::PREFIX;
pub use status::Status;

/// A transfer as the UIs show it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransferInfo {
    /// The bundle ID, which identifies the transfer.
    #[serde(serialize_with = "crate::display", deserialize_with = "crate::parse")]
    pub id: BundleId,
    /// The name to show: one the user chose, else the bundle's, else the link's hint.
    pub name: String,
    /// Download or share.
    pub kind: Kind,
    /// What it is doing.
    pub status: Status,
    /// The bundle's total size in bytes, once the manifest is known.
    pub size: Option<u64>,
    /// Chunks stored.
    pub have: u64,
    /// The bundle's chunk count, once the manifest is known; 0 before.
    pub chunks: u64,
    /// The share of chunks stored, from 0 to 1.
    pub progress: f64,
    /// Seconds until a download completes at its current rate, while it is downloading.
    pub eta: Option<u64>,
    /// Bytes sent per byte received, or per byte of the bundle for one that received none.
    pub ratio: Option<f64>,
    /// Payload bytes received, over every run.
    pub downloaded: u64,
    /// Payload bytes sent, over every run.
    pub uploaded: u64,
    /// How long it has shared while complete, over every run, in seconds.
    pub seeding_time: u64,
    /// Bytes per second received, recently.
    pub download_rate: u64,
    /// Bytes per second sent, recently.
    pub upload_rate: u64,
    /// Open or opening connections.
    pub connections: usize,
    /// How many of the connections go through a relay, which is usually slower.
    pub relayed: usize,
    /// Peers known, connected or not.
    pub peers: usize,
    /// When it was added, in milliseconds since the Unix epoch.
    pub added: i64,
    /// When it completed, in milliseconds since the Unix epoch.
    pub completed: Option<i64>,
    /// Its place in the queue, from 0.
    pub queue_position: u32,
    /// It runs regardless of the queue.
    pub forced: bool,
    /// Its tags.
    pub tags: BTreeSet<String>,
    /// Its category.
    pub category: Option<String>,
    /// The folder a download's bundle folder is made in; a share's base folder.
    pub save_folder: PathBuf,
    /// The folder holding the bundle's entries, once known.
    pub content_dir: Option<PathBuf>,
    /// A `tempest:` link to the bundle.
    pub link: String,
    /// Its own settings, by dotted key, over the engine's.
    pub settings: BTreeMap<String, serde_json::Value>,
}

/// Whether a transfer downloads a bundle or shares one made here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Added from a link, downloading into a save folder.
    Download,
    /// Made from a file or folder here, shared from where it is.
    Share,
}

impl Kind {
    /// Its name: `download` or `share`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Download => "download",
            Self::Share => "share",
        }
    }
}

#[cfg(any(test, feature = "testing"))]
impl TransferInfo {
    /// A stopped download of nothing yet, named `name`, at `position` in the queue, with an ID
    /// made of `seed`.
    pub fn example(seed: u8, name: &str, position: u32) -> Self {
        Self {
            id: tempest_protocol::Hash::new([seed; 32]),
            name: name.to_owned(),
            kind: Kind::Download,
            status: Status::Stopped,
            size: None,
            have: 0,
            chunks: 0,
            progress: 0.0,
            eta: None,
            ratio: None,
            downloaded: 0,
            uploaded: 0,
            seeding_time: 0,
            download_rate: 0,
            upload_rate: 0,
            connections: 0,
            relayed: 0,
            peers: 0,
            added: 0,
            completed: None,
            queue_position: position,
            forced: false,
            tags: BTreeSet::new(),
            category: None,
            save_folder: PathBuf::from("/downloads"),
            content_dir: None,
            link: String::new(),
            settings: BTreeMap::new(),
        }
    }
}

pub(crate) fn display<S: Serializer>(
    value: &impl Display,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.collect_str(value)
}

pub(crate) fn parse<'de, D: Deserializer<'de>, T>(deserializer: D) -> Result<T, D::Error>
where
    T: FromStr<Err: Display>,
{
    let text = String::deserialize(deserializer)?;
    text.parse().map_err(serde::de::Error::custom)
}
