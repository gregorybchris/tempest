//! Tempest's engine: the long-running part of the client that does every transfer (D-079).
//!
//! - [`config`]: the YAML config file, with session settings as a profile plus overrides.
//! - [`store`]: saved state in SQLite: transfers, verified chunks, categories, settings and
//!   known peers.
//! - [`Engine`]: a handle to the engine task, which owns one [`Node`](tempest_net::Node), LAN
//!   discovery and a tracker client, and runs transfers through the queue.
//! - [`events`]: status snapshots and decision events as a numbered, resumable stream.
//! - [`manifest`]: building a bundle from a file or folder.
//!
//! The HTTP API (D-080) is a layer over [`Engine`]; nothing here depends on it.

pub mod config;
mod engine;
pub mod events;
pub mod manifest;
mod queue;
mod status;
pub mod store;

use std::collections::BTreeSet;
use std::fmt::Display;
use std::path::PathBuf;

use serde::{Serialize, Serializer};
use tempest_protocol::BundleId;

pub use config::EngineConfig;
pub use engine::{AddOptions, Engine, EngineError, ShareOptions};
pub use events::{EngineEvent, Numbered, Subscription, SubscriptionError};
pub use queue::QueueMove;
pub use status::{STALL_AFTER, Status};
pub use store::Kind;

/// A transfer as the UIs show it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TransferInfo {
    /// The bundle ID, which identifies the transfer.
    #[serde(serialize_with = "display")]
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
    /// Payload bytes received, over every run.
    pub downloaded: u64,
    /// Payload bytes sent, over every run.
    pub uploaded: u64,
    /// Bytes per second received, recently.
    pub download_rate: u64,
    /// Bytes per second sent, recently.
    pub upload_rate: u64,
    /// Open or opening connections.
    pub connections: usize,
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
}

fn display<S: Serializer>(value: &impl Display, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_str(value)
}
