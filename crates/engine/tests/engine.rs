//! Engines on loopback: sharing, downloading, the queue, the transfer controls, and resuming
//! after a restart.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;
use std::time::Duration;

use serde_json::json;
use tempest_core::EventKind;
use tempest_engine::config::{LanSettings, PeerSettings, QueueSettings};
use tempest_engine::{
    AddOptions, Engine, EngineConfig, EngineError, EngineEvent, Kind, QueueMove, ShareOptions,
    Status, TransferInfo,
};
use tempest_protocol::{BundleId, Hash, Link};

/// How long anything here may take.
const LIMIT: Duration = Duration::from_secs(60);

/// An engine on 127.0.0.1 with a port the system picks and LAN discovery off, keeping its state
/// in `dir`.
fn config(dir: &Path) -> EngineConfig {
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
async fn wait_for(
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
fn contents(size: usize, seed: u64) -> Vec<u8> {
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
fn nobodys_link(seed: u8) -> String {
    Link::new(Hash::new([seed; 32])).to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_download_stops_restarts_and_resumes_to_completion() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("album");
    fs::create_dir_all(source.join("disc 2")).unwrap();
    let files = [
        ("one.flac", contents(3 << 20, 1)),
        ("disc 2/two.flac", contents(5 << 20, 2)),
        ("notes.txt", b"liner notes".to_vec()),
    ];
    for (path, data) in &files {
        fs::write(source.join(path), data).unwrap();
    }

    let sharer = Engine::open(config(&root.path().join("a"))).await.unwrap();
    let share = sharer
        .add_path(&source, ShareOptions::default())
        .await
        .unwrap();
    assert_eq!(share.kind, Kind::Share);
    let canonical = fs::canonicalize(&source).unwrap();
    assert_eq!(share.content_dir.as_deref(), Some(canonical.as_path()));
    let id = share.id;
    wait_for(&sharer, id, "sharing", |t| t.status == Status::Sharing).await;

    // The downloader asks for one chunk at a time, so it is still early on when it stops.
    let b = root.path().join("b");
    let downloader = Engine::open(config(&b)).await.unwrap();
    let mut events = downloader.subscribe(None).await.unwrap();
    let options = AddOptions {
        stopped: true,
        tags: BTreeSet::from(["music".to_owned()]),
        ..AddOptions::default()
    };
    let added = downloader.add_link(&share.link, options).await.unwrap();
    assert_eq!(added.status, Status::Stopped);
    let slow = BTreeMap::from([
        ("requests.chunks".to_owned(), json!(1)),
        ("requests.adaptive".to_owned(), json!(false)),
        ("requests.initial_in_flight".to_owned(), json!(1)),
        ("requests.min_in_flight".to_owned(), json!(1)),
    ]);
    downloader.set_transfer_settings(id, slow).await.unwrap();
    downloader.start(id).await.unwrap();

    // The stream carries snapshots and the session's decisions, numbered in order. Stop as soon
    // as the first data arrives.
    let (mut snapshots, mut received, mut last) = (0, 0, 0);
    while snapshots == 0 || received == 0 {
        let event = tokio::time::timeout(LIMIT, events.recv()).await.unwrap();
        let event = event.expect("a quiet stream never lags");
        assert!(event.id > last, "event IDs increase");
        last = event.id;
        match &event.event {
            EngineEvent::Transfer(info) if info.id == id => snapshots += 1,
            EngineEvent::Decision { id: of, event } if *of == id => {
                received += usize::from(matches!(event.kind, EventKind::Received { .. }));
            }
            _ => {}
        }
    }
    downloader.stop(id).await.unwrap();
    let stopped = wait_for(&downloader, id, "stopped", |t| t.status == Status::Stopped).await;
    assert_eq!(stopped.name, "album");
    let content_dir = b.join("downloads").join("album");
    assert_eq!(stopped.content_dir.as_deref(), Some(content_dir.as_path()));

    // After a restart the transfer is where it was: stopped, part of the way, with its tags.
    downloader.shutdown().await;
    drop(downloader);
    let downloader = Engine::open(config(&b)).await.unwrap();
    let restored = downloader.get(id).await.unwrap();
    assert_eq!(restored.status, Status::Stopped);
    assert!(
        0 < restored.have && restored.have < restored.chunks,
        "stopped part of the way: {restored:#?}"
    );
    assert_eq!(restored.tags, BTreeSet::from(["music".to_owned()]));

    downloader
        .set_transfer_settings(id, BTreeMap::new())
        .await
        .unwrap();
    downloader.start(id).await.unwrap();
    let resumed = wait_for(&downloader, id, "running again", |t| {
        matches!(t.status, Status::Downloading | Status::Sharing)
    })
    .await;
    assert!(resumed.have >= restored.have, "resumed, not started over");
    let done = wait_for(&downloader, id, "complete", |t| t.status == Status::Sharing).await;
    assert_eq!(done.have, done.chunks);
    assert!(done.completed.is_some());
    for (path, data) in &files {
        assert_eq!(&fs::read(content_dir.join(path)).unwrap(), data, "{path}");
    }

    // Restarted again, a complete download shares without downloading anything.
    downloader.shutdown().await;
    let downloader = Engine::open(config(&b)).await.unwrap();
    let again = wait_for(&downloader, id, "sharing", |t| t.status == Status::Sharing).await;
    assert_eq!(again.have, again.chunks);

    downloader.shutdown().await;
    sharer.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_queue_runs_transfers_in_order_up_to_its_limits() {
    let root = tempfile::tempdir().unwrap();
    let mut config = config(root.path());
    config.queue = QueueSettings {
        max_active_downloads: Some(1),
        max_active_uploads: None,
    };
    let engine = Engine::open(config).await.unwrap();
    let first = engine
        .add_link(&nobodys_link(1), AddOptions::default())
        .await
        .unwrap();
    let second = engine
        .add_link(&nobodys_link(2), AddOptions::default())
        .await
        .unwrap();
    let third = engine
        .add_link(&nobodys_link(3), AddOptions::default())
        .await
        .unwrap();
    assert_eq!(first.status, Status::Downloading);
    assert_eq!(second.status, Status::Queued);
    assert_eq!(third.queue_position, 2);

    // Moving the third to the top gives it the slot.
    let order = engine
        .move_in_queue(third.id, QueueMove::Top)
        .await
        .unwrap();
    let ids: Vec<_> = order.iter().map(|t| t.id).collect();
    assert_eq!(ids, [third.id, first.id, second.id]);
    wait_for(&engine, third.id, "running", |t| {
        t.status == Status::Downloading
    })
    .await;
    wait_for(&engine, first.id, "queued", |t| t.status == Status::Queued).await;

    // Force-starting takes no slot; stopping frees one for the next in line.
    let forced = engine.force_start(second.id).await.unwrap();
    assert!(forced.forced);
    wait_for(&engine, second.id, "forced", |t| {
        t.status == Status::Downloading
    })
    .await;
    engine.stop(third.id).await.unwrap();
    wait_for(&engine, third.id, "stopped", |t| {
        t.status == Status::Stopped
    })
    .await;
    wait_for(&engine, first.id, "promoted", |t| {
        t.status == Status::Downloading
    })
    .await;

    engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn transfers_are_renamed_tagged_categorized_and_removed() {
    let root = tempfile::tempdir().unwrap();
    let engine = Engine::open(config(root.path())).await.unwrap();
    let films = root.path().join("films");
    engine.set_category_folder("films", &films).await.unwrap();
    let options = AddOptions {
        category: Some("films".into()),
        stopped: true,
        ..AddOptions::default()
    };
    let added = engine.add_link(&nobodys_link(1), options).await.unwrap();
    assert_eq!(added.save_folder, films);
    assert!(matches!(
        engine
            .add_link(&nobodys_link(1), AddOptions::default())
            .await,
        Err(EngineError::AlreadyAdded(_))
    ));

    let renamed = engine
        .rename(added.id, Some("  Holiday  ".into()))
        .await
        .unwrap();
    assert_eq!(renamed.name, "Holiday");
    let tags = BTreeSet::from(["a".to_owned(), " b ".to_owned()]);
    let tagged = engine.set_tags(added.id, tags).await.unwrap();
    assert_eq!(
        tagged.tags,
        BTreeSet::from(["a".to_owned(), "b".to_owned()])
    );
    assert!(
        engine
            .set_tags(added.id, BTreeSet::from([" ".to_owned()]))
            .await
            .is_err()
    );

    // A category's folder applies to new adds only.
    let elsewhere = root.path().join("elsewhere");
    engine
        .set_category_folder("films", &elsewhere)
        .await
        .unwrap();
    assert_eq!(engine.get(added.id).await.unwrap().save_folder, films);
    assert!(matches!(
        engine.set_category(added.id, Some("music".into())).await,
        Err(EngineError::UnknownCategory(_))
    ));
    engine.remove_category("films").await.unwrap();
    assert_eq!(engine.get(added.id).await.unwrap().category, None);

    let bad = BTreeMap::from([("upload.nonsense".to_owned(), json!(1))]);
    assert!(engine.set_transfer_settings(added.id, bad).await.is_err());

    // Everything is saved.
    engine.shutdown().await;
    let engine = Engine::open(config(root.path())).await.unwrap();
    let saved = engine.get(added.id).await.unwrap();
    assert_eq!(saved.name, "Holiday");
    assert_eq!(saved.tags.len(), 2);
    assert_eq!(saved.status, Status::Stopped);

    engine.remove(added.id, false).await.unwrap();
    assert!(engine.get(added.id).await.is_none());
    assert!(matches!(
        engine.remove(added.id, false).await,
        Err(EngineError::NotFound(_))
    ));
    engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_with_files_deletes_only_the_bundle() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("folder");
    fs::create_dir_all(source.join("sub")).unwrap();
    fs::write(source.join("sub/a.txt"), "a").unwrap();
    fs::write(source.join("b.txt"), "b").unwrap();

    let sharer = Engine::open(config(&root.path().join("a"))).await.unwrap();
    let share = sharer
        .add_path(&source, ShareOptions::default())
        .await
        .unwrap();
    let downloader = Engine::open(config(&root.path().join("b"))).await.unwrap();
    let download = downloader
        .add_link(&share.link, AddOptions::default())
        .await
        .unwrap();
    let done = wait_for(&downloader, download.id, "complete", |t| {
        t.status == Status::Sharing
    })
    .await;
    let content_dir = done.content_dir.unwrap();
    // Something of the user's, next to the bundle's files.
    fs::write(content_dir.join("sub/mine.txt"), "mine").unwrap();

    downloader.remove(download.id, true).await.unwrap();
    assert!(!content_dir.join("b.txt").exists());
    assert!(!content_dir.join("sub/a.txt").exists());
    assert!(content_dir.join("sub/mine.txt").exists());

    // Removing a share keeping its files leaves them alone.
    sharer.remove(share.id, false).await.unwrap();
    assert!(source.join("b.txt").exists());

    downloader.shutdown().await;
    sharer.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn one_engine_at_a_time_uses_a_data_directory() {
    let root = tempfile::tempdir().unwrap();
    let engine = Engine::open(config(root.path())).await.unwrap();
    for second in [
        Engine::open(config(root.path())).await,
        Engine::open_held(config(root.path())).await,
    ] {
        assert!(matches!(second, Err(EngineError::Busy(_))), "{second:?}");
    }
    engine.shutdown().await;
    // Shutting down releases the directory.
    Engine::open_held(config(root.path()))
        .await
        .unwrap()
        .shutdown()
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_held_engine_changes_state_but_runs_nothing() {
    let root = tempfile::tempdir().unwrap();
    let held = Engine::open_held(config(root.path())).await.unwrap();
    let added = held
        .add_link(&nobodys_link(1), AddOptions::default())
        .await
        .unwrap();
    let id = added.id;
    // It wants to run, but waits for an engine that runs transfers.
    assert_eq!(added.status, Status::Queued);
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(held.get(id).await.unwrap().status, Status::Queued);
    held.shutdown().await;

    let engine = Engine::open(config(root.path())).await.unwrap();
    wait_for(&engine, id, "running", |t| {
        matches!(t.status, Status::Downloading | Status::Stalled { .. })
    })
    .await;
    engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_changed_through_the_engine_persist_over_the_file() {
    let root = tempfile::tempdir().unwrap();
    let engine = Engine::open(config(root.path())).await.unwrap();
    for seed in 1..=3 {
        engine
            .add_link(&nobodys_link(seed), AddOptions::default())
            .await
            .unwrap();
    }
    let running =
        |list: Vec<TransferInfo>| list.iter().filter(|t| t.status != Status::Queued).count();
    assert_eq!(running(engine.list().await), 3);

    // A new queue limit applies at once.
    let changes = BTreeMap::from([
        ("queue.max_active_downloads".to_owned(), json!(1)),
        ("lan.port".to_owned(), json!(5555)),
    ]);
    let view = engine.change_settings(changes).await.unwrap();
    assert_eq!(view.settings.queue.max_active_downloads, Some(1));
    assert_eq!(view.restart_required, ["lan"]);
    assert_eq!(running(engine.list().await), 1);

    // Invalid changes change nothing, and the data directory is the file's alone.
    for bad in [
        ("queue.max_active_downloads", json!(0)),
        ("peers.nope", json!(1)),
        ("data_dir", json!("/elsewhere")),
    ] {
        let changes = BTreeMap::from([(bad.0.to_owned(), bad.1.clone())]);
        assert!(engine.change_settings(changes).await.is_err(), "{bad:?}");
    }
    assert_eq!(engine.settings().await.unwrap(), view);
    engine.shutdown().await;

    // They outlive a restart, and resetting one brings back the file's value.
    let engine = Engine::open(config(root.path())).await.unwrap();
    let view = engine.settings().await.unwrap();
    assert_eq!(view.settings.lan.port, 5555);
    assert!(view.restart_required.is_empty());
    assert_eq!(running(engine.list().await), 1);
    let view = engine
        .reset_setting("queue.max_active_downloads")
        .await
        .unwrap();
    assert_eq!(view.settings.queue.max_active_downloads, Some(3));
    assert_eq!(
        view.changed,
        BTreeMap::from([("lan.port".to_owned(), json!(5555))])
    );
    assert_eq!(running(engine.list().await), 3);
    engine.shutdown().await;
}
