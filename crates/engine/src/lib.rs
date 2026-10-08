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
//! - [`reveal`]: showing a path in the system's file manager.
//! - [`schedule`]: the weekly schedule for alternative speed limits.

pub mod api;
pub mod config;
mod engine;
pub mod events;
pub mod inspect;
pub mod manifest;
mod queue;
pub mod reveal;
pub mod schedule;
mod status;
pub mod store;

pub use config::EngineConfig;
pub use engine::{AddOptions, Engine, EngineError, SettingsView, ShareOptions};
pub use events::{EngineEvent, Numbered, Subscription, SubscriptionError};
pub use queue::QueueMove;
pub use status::{STALL_AFTER, Status};
pub use tempest_api::{Kind, TransferInfo};

/// `info` with the fields computed from the others filled in: progress, the time left and the
/// ratio.
pub(crate) fn with_derived(mut info: TransferInfo) -> TransferInfo {
    info.progress = match info.chunks {
        0 => 0.0,
        chunks => info.have as f64 / chunks as f64,
    };
    let remaining = info
        .size
        .map(|size| size - (size as f64 * info.progress) as u64);
    info.eta = match (&info.status, remaining) {
        (Status::Downloading, Some(remaining)) if info.download_rate > 0 => {
            Some(remaining.div_ceil(info.download_rate))
        }
        _ => None,
    };
    info.ratio = ratio(info.uploaded, info.downloaded, info.size);
    info
}

/// Bytes sent per byte received, or per byte of the bundle for a transfer that received none.
pub(crate) fn ratio(uploaded: u64, downloaded: u64, size: Option<u64>) -> Option<f64> {
    let base = match downloaded {
        0 => size.unwrap_or(0),
        downloaded => downloaded,
    };
    (base > 0).then(|| uploaded as f64 / base as f64)
}
