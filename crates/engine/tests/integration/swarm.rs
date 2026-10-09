//! The M5 swarm: three engines on loopback and a tracker. Each way of finding peers is the only
//! way one pair can meet, so the swarm forms only if all of them work at once:
//!
//! - the sharer and the first downloader meet through the tracker;
//! - the sharer and the second downloader meet through LAN discovery;
//! - the two downloaders meet through peer exchange, since the first has LAN discovery off and
//!   the second knows no tracker.

use crate::common;

use std::collections::BTreeSet;
use std::fs;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tempest_core::EventKind;
use tempest_engine::{AddOptions, Engine, EngineConfig, EngineEvent, ShareOptions, Status};
use tempest_protocol::{BundleId, Link, PeerId};
use tempest_tracker::{Tracker, TrackerConfig};
use tokio::net::TcpListener;

use common::{config, contents, wait_for};

/// A tracker on localhost that asks for an announce every second. Returns its URL.
async fn start_tracker() -> String {
    let config = TrackerConfig {
        interval: Duration::from_secs(1),
        min_interval: Duration::ZERO,
        ..TrackerConfig::default()
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let tracker = Tracker::new(config).unwrap();
    tokio::spawn(tracker.serve(listener, std::future::pending()));
    url
}

/// A UDP port no one else here uses, for this test's LAN discovery groups.
fn free_udp_port() -> u16 {
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.local_addr().unwrap().port()
}

/// An engine's config, with peer exchange every second and LAN discovery on `lan_port`.
fn swarm_config(dir: &std::path::Path, lan_port: Option<u16>) -> EngineConfig {
    let mut config = config(dir);
    config
        .set("settings.connections.peer_exchange_interval", json!("1s"))
        .unwrap();
    if let Some(port) = lan_port {
        config.lan.enabled = true;
        config.lan.port = port;
    }
    config
}

/// The peers `id`'s session opened connections with, and how many decision events it emitted,
/// as `engine`'s event stream tells them.
#[derive(Debug, Default)]
struct Seen {
    opened: BTreeSet<PeerId>,
    decisions: usize,
}

/// Subscribes before returning, so nothing that happens afterwards is missed.
async fn watch(engine: &Engine, id: BundleId) -> Arc<Mutex<Seen>> {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let mut events = engine.subscribe(None).await.unwrap();
    let record = seen.clone();
    tokio::spawn(async move {
        while let Ok(event) = events.recv().await {
            if let EngineEvent::Decision { id: of, event } = &event.event
                && *of == id
            {
                let mut seen = record.lock().unwrap();
                seen.decisions += 1;
                if let EventKind::Opened { peer, .. } = event.kind {
                    seen.opened.insert(peer);
                }
            }
        }
    });
    seen
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_swarm_forms_through_the_tracker_lan_discovery_and_peer_exchange() {
    let root = tempfile::tempdir().unwrap();
    let tracker = start_tracker().await;
    let lan_port = free_udp_port();

    let source = root.path().join("album");
    fs::create_dir_all(&source).unwrap();
    let files = [
        ("one.bin", contents(2 << 20, 1)),
        ("two.bin", contents(1 << 20, 2)),
    ];
    for (path, data) in &files {
        fs::write(source.join(path), data).unwrap();
    }

    // The sharer announces to the tracker and answers on the LAN. Its uploads are limited, so
    // the downloaders are still downloading when they learn of each other: several seconds,
    // against peer exchange every second.
    let mut sharer_config = swarm_config(&root.path().join("sharer"), Some(lan_port));
    sharer_config.trackers = vec![tracker.clone()];
    sharer_config.limits.upload = Some(1 << 19);
    let sharer = Engine::open(sharer_config).await.unwrap();
    let share = sharer
        .add_path(&source, ShareOptions::default())
        .await
        .unwrap();
    let id = share.id;
    wait_for(&sharer, id, "sharing", |t| t.status == Status::Sharing).await;

    // Both downloaders open before either starts, so they download at the same time.
    let first = Engine::open(swarm_config(&root.path().join("first"), None))
        .await
        .unwrap();
    let second = Engine::open(swarm_config(&root.path().join("second"), Some(lan_port)))
        .await
        .unwrap();
    let first_seen = watch(&first, id).await;
    let second_seen = watch(&second, id).await;

    // The first downloader's link names only the tracker; it doesn't use the LAN.
    let mut tracker_link = Link::new(id);
    tracker_link.trackers = vec![tracker.clone()];
    first
        .add_link(&tracker_link.to_string(), AddOptions::default())
        .await
        .unwrap();
    // The second's link names only the bundle; it finds the sharer on the LAN.
    second
        .add_link(&Link::new(id).to_string(), AddOptions::default())
        .await
        .unwrap();

    for (engine, name) in [(&first, "first"), (&second, "second")] {
        // Complete, whether still sharing or already done: once every peer is complete, a
        // downloader soon stops sharing.
        let done = wait_for(engine, id, name, |t| t.completed.is_some()).await;
        assert_eq!(done.have, done.chunks);
        let folder = root.path().join(name).join("downloads/album");
        for (path, data) in &files {
            assert!(
                &fs::read(folder.join(path)).unwrap() == data,
                "{name}: {path}"
            );
        }
    }

    // Each downloader reached the sharer, and the two downloaders reached each other.
    {
        let (first_seen, second_seen) = (first_seen.lock().unwrap(), second_seen.lock().unwrap());
        let sharer_id = sharer.peer_id();
        assert!(first_seen.opened.contains(&sharer_id), "{first_seen:?}");
        assert!(second_seen.opened.contains(&sharer_id), "{second_seen:?}");
        let met = first_seen.opened.contains(&second.peer_id())
            || second_seen.opened.contains(&first.peer_id());
        assert!(met, "the downloaders met: {first_seen:?} {second_seen:?}");
        assert!(first_seen.decisions > 0 && second_seen.decisions > 0);
    }

    for engine in [&first, &second, &sharer] {
        engine.shutdown().await;
    }
}
