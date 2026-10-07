//! Helpers shared by the engine's tests.

#![allow(dead_code)]

use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;
use std::time::Duration;

use tempest_engine::config::{LanSettings, PeerSettings};
use tempest_engine::{Engine, EngineConfig, TransferInfo};
use tempest_protocol::{BundleId, Hash, Link};

/// How long anything here may take.
pub const LIMIT: Duration = Duration::from_secs(60);

/// An engine on 127.0.0.1 with a port the system picks and LAN discovery off, keeping its state
/// in `dir`.
pub fn config(dir: &Path) -> EngineConfig {
    EngineConfig {
        peers: PeerSettings {
            port: 0,
            bind: vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
        },
        lan: LanSettings {
            enabled: false,
            ..LanSettings::default()
        },
        data_dir: Some(dir.join("data")),
        save_folder: Some(dir.join("downloads")),
        ..EngineConfig::default()
    }
}

/// Waits until `id`'s state satisfies `done`.
pub async fn wait_for(
    engine: &Engine,
    id: BundleId,
    what: &str,
    done: impl Fn(&TransferInfo) -> bool,
) -> TransferInfo {
    let wait = async {
        loop {
            if let Some(info) = engine.get(id).await
                && done(&info)
            {
                return info;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    match tokio::time::timeout(LIMIT, wait).await {
        Ok(info) => info,
        Err(_) => panic!("timed out waiting for {what}: {:#?}", engine.get(id).await),
    }
}

/// Pseudo-random bytes, so that no two chunks are alike.
pub fn contents(size: usize, seed: u64) -> Vec<u8> {
    let mut state = seed;
    (0..size)
        .map(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 56) as u8
        })
        .collect()
}

/// A random bundle ID nobody shares.
pub fn nobodys_link(seed: u8) -> String {
    Link::new(Hash::new([seed; 32])).to_string()
}
