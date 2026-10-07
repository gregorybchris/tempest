//! Tempest's engine: the long-running part of the client that does every transfer (D-079).
//!
//! - [`config`]: the YAML config file, with session settings as a profile plus overrides.
//! - [`store`]: saved state in SQLite: transfers, verified chunks, categories, settings and
//!   known peers.
//! - [`Engine`]: a handle to the engine task, which owns one [`Node`](tempest_net::Node), LAN
//!   discovery and a tracker client, and runs transfers through the queue.
//! - [`events`]: status snapshots and decision events as a numbered, resumable stream.
//! - [`inspect`]: what the UIs show inside a transfer: files, peers and a chunk map.
//! - [`manifest`]: building a bundle from a file or folder.
//! - [`api`]: the HTTP API over an [`Engine`] (D-080), and a client for it.
//! - [`schedule`]: the weekly schedule for alternative speed limits.

pub mod api;
pub mod config;
mod engine;
pub mod events;
pub mod inspect;
pub mod manifest;
mod queue;
pub mod schedule;
mod status;
pub mod store;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Display;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tempest_protocol::BundleId;

pub use config::EngineConfig;
pub use engine::{AddOptions, Engine, EngineError, SettingsView, ShareOptions};
pub use events::{EngineEvent, Numbered, Subscription, SubscriptionError};
pub use queue::QueueMove;
pub use status::{STALL_AFTER, Status};
pub use store::Kind;

/// A transfer as the UIs show it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransferInfo {
    /// The bundle ID, which identifies the transfer.
    #[serde(serialize_with = "display", deserialize_with = "parse")]
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

impl TransferInfo {
    /// Fills in the fields computed from the others: progress, the time left and the ratio.
    pub(crate) fn with_derived(mut self) -> Self {
        self.progress = match self.chunks {
            0 => 0.0,
            chunks => self.have as f64 / chunks as f64,
        };
        let remaining = self
            .size
            .map(|size| size - (size as f64 * self.progress) as u64);
        self.eta = match (&self.status, remaining) {
            (Status::Downloading, Some(remaining)) if self.download_rate > 0 => {
                Some(remaining.div_ceil(self.download_rate))
            }
            _ => None,
        };
        self.ratio = ratio(self.uploaded, self.downloaded, self.size);
        self
    }
}

/// Bytes sent per byte received, or per byte of the bundle for a transfer that received none.
pub(crate) fn ratio(uploaded: u64, downloaded: u64, size: Option<u64>) -> Option<f64> {
    let base = match downloaded {
        0 => size.unwrap_or(0),
        downloaded => downloaded,
    };
    (base > 0).then(|| uploaded as f64 / base as f64)
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
