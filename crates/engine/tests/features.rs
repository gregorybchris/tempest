//! The engine's v1 features beyond the basic controls: speed and sharing limits, rechecking,
//! moving storage and notifications.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::json;
use tempest_engine::{
    AddOptions, Engine, EngineError, EngineEvent, ShareOptions, Status, Subscription, TransferInfo,
};

use common::{config, contents, wait_for};

const MIB: usize = 1 << 20;

/// A folder `album` under `root` with two files, and their contents.
fn album(root: &Path) -> (PathBuf, Vec<(&'static str, Vec<u8>)>) {
    let source = root.join("album");
    fs::create_dir_all(&source).unwrap();
    let files = vec![
        ("one.bin", contents(MIB + 12_345, 1)),
        ("two.bin", contents(MIB, 2)),
    ];
    for (path, data) in &files {
        fs::write(source.join(path), data).unwrap();
    }
    (source, files)
}

fn assert_files(dir: &Path, files: &[(&str, Vec<u8>)]) {
    for (path, data) in files {
        assert!(&fs::read(dir.join(path)).unwrap() == data, "{path} differs");
    }
}

/// The statuses in the transfer snapshots `events` has waiting.
async fn statuses(events: &mut Subscription) -> Vec<Status> {
    let mut statuses = Vec::new();
    let wait = Duration::from_millis(50);
    while let Ok(Ok(event)) = tokio::time::timeout(wait, events.recv()).await {
        if let EngineEvent::Transfer(info) = &event.event {
            statuses.push(info.status.clone());
        }
    }
    statuses
}

/// Shares `source` from engine `a` and downloads it to completion in engine `b`.
async fn share_and_download(a: &Engine, b: &Engine, source: &Path) -> (TransferInfo, TransferInfo) {
    let share = a.add_path(source, ShareOptions::default()).await.unwrap();
    wait_for(a, share.id, "sharing", |t| t.status == Status::Sharing).await;
    b.add_link(&share.link, AddOptions::default())
        .await
        .unwrap();
    let done = wait_for(b, share.id, "downloaded", |t| t.status == Status::Sharing).await;
    (share, done)
}

#[tokio::test(flavor = "multi_thread")]
async fn speed_limits_slow_transfers_and_sharing_limits_end_them() {
    let root = tempfile::tempdir().unwrap();
    let (source, files) = album(root.path());
    let a = Engine::open(config(&root.path().join("a"))).await.unwrap();
    let b = Engine::open(config(&root.path().join("b"))).await.unwrap();

    // Every upload from `a` together at 1 MiB/s: the 2 MiB album takes over a second and a half.
    let limit = BTreeMap::from([("limits.upload".to_owned(), json!(MIB))]);
    a.change_settings(limit).await.unwrap();
    // Alternative limits can be switched on by hand.
    let toggle = BTreeMap::from([("limits.alternative.enabled".to_owned(), json!(true))]);
    let view = b.change_settings(toggle).await.unwrap();
    assert!(view.alternative_limits_active);
    assert!(!a.settings().await.unwrap().alternative_limits_active);

    let share = a.add_path(&source, ShareOptions::default()).await.unwrap();
    let id = share.id;
    // The share stops once it has sent the bundle once.
    let ratio = BTreeMap::from([("sharing.ratio".to_owned(), json!(1))]);
    let configured = a.set_transfer_settings(id, ratio).await.unwrap();
    assert_eq!(configured.settings["sharing.ratio"], json!(1));
    wait_for(&a, id, "sharing", |t| t.status == Status::Sharing).await;

    let started = Instant::now();
    let options = AddOptions::default();
    b.add_link(&share.link, options).await.unwrap();
    // The download removes itself after sharing for a second, keeping its files.
    let time = BTreeMap::from([
        ("sharing.time".to_owned(), json!("1s")),
        ("sharing.action".to_owned(), json!("remove")),
    ]);
    b.set_transfer_settings(id, time).await.unwrap();
    let mut events = b.subscribe(None).await.unwrap();
    let removed = async {
        loop {
            let event = events.recv().await.unwrap();
            if matches!(event.event, EngineEvent::Removed { id: gone } if gone == id) {
                return started.elapsed();
            }
        }
    };
    let took = tokio::time::timeout(common::LIMIT, removed).await.unwrap();
    assert!(took > Duration::from_millis(1500), "{took:?}");
    assert_files(&root.path().join("b/downloads/album"), &files);

    let done = wait_for(&a, id, "completed", |t| t.status == Status::Completed).await;
    assert!(done.ratio.unwrap() >= 1.0);
    assert!(done.seeding_time >= 1, "{done:#?}");

    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_recheck_finds_damage_and_downloads_it_again() {
    let root = tempfile::tempdir().unwrap();
    let (source, files) = album(root.path());
    let a = Engine::open(config(&root.path().join("a"))).await.unwrap();
    let b_dir = root.path().join("b");
    let b = Engine::open(config(&b_dir)).await.unwrap();
    let (_, done) = share_and_download(&a, &b, &source).await;
    let id = done.id;
    let downloaded = done.downloaded;

    // Damage a chunk in the middle of a file, keeping its size.
    let damaged = b_dir.join("downloads/album/one.bin");
    let mut bytes = fs::read(&damaged).unwrap();
    bytes[MIB / 2] ^= 0xff;
    fs::write(&damaged, &bytes).unwrap();

    let mut events = b.subscribe(None).await.unwrap();
    b.recheck(id).await.unwrap();
    let repaired = wait_for(&b, id, "repaired", |t| {
        t.status == Status::Sharing && t.downloaded > downloaded
    })
    .await;
    let seen = statuses(&mut events).await;
    assert!(seen.contains(&Status::Checking), "{seen:?}");
    assert_eq!(repaired.have, repaired.chunks);
    // A file that fails its check is fetched again: the manifest holds only each file's root,
    // so nothing finds which of its chunks is damaged. The other file isn't.
    let again = repaired.downloaded - downloaded;
    assert!(again < (MIB + MIB / 2) as u64, "{again}");
    assert_files(&b_dir.join("downloads/album"), &files);

    // The verified chunks are recorded: after a restart it shares at once.
    b.shutdown().await;
    let b = Engine::open(config(&b_dir)).await.unwrap();
    let again = wait_for(&b, id, "sharing", |t| t.status == Status::Sharing).await;
    assert_eq!(again.have, again.chunks);

    // A transfer with no data yet has nothing to check.
    let empty = b
        .add_link(&common::nobodys_link(7), AddOptions::default())
        .await
        .unwrap();
    assert!(matches!(
        b.recheck(empty.id).await,
        Err(EngineError::NoData(_))
    ));

    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn storage_moves_and_a_failed_move_leaves_it_in_place() {
    let root = tempfile::tempdir().unwrap();
    let (source, files) = album(root.path());
    let a = Engine::open(config(&root.path().join("a"))).await.unwrap();
    let b = Engine::open(config(&root.path().join("b"))).await.unwrap();
    let (share, done) = share_and_download(&a, &b, &source).await;
    let id = done.id;

    // A download's bundle folder moves into the new folder, and it shares from there.
    let elsewhere = root.path().join("elsewhere");
    let mut events = b.subscribe(None).await.unwrap();
    b.move_storage(id, &elsewhere).await.unwrap();
    let moved = wait_for(&b, id, "moved", |t| {
        t.status == Status::Sharing && t.save_folder == elsewhere
    })
    .await;
    assert_eq!(moved.content_dir, Some(elsewhere.join("album")));
    assert_files(&elsewhere.join("album"), &files);
    assert!(!root.path().join("b/downloads/album").exists());
    let seen = statuses(&mut events).await;
    assert!(seen.contains(&Status::Moving), "{seen:?}");

    // Something already at the destination stops the move; nothing changes on disk.
    let blocked = root.path().join("blocked");
    fs::create_dir_all(blocked.join("album")).unwrap();
    b.move_storage(id, &blocked).await.unwrap();
    let failed = wait_for(&b, id, "failed to move", |t| {
        matches!(t.status, Status::Errored { .. })
    })
    .await;
    let Status::Errored { reason } = &failed.status else {
        unreachable!()
    };
    assert!(reason.contains("already exists"), "{reason}");
    assert_eq!(failed.save_folder, elsewhere);
    assert_files(&elsewhere.join("album"), &files);
    assert_eq!(fs::read_dir(blocked.join("album")).unwrap().count(), 0);
    b.start(id).await.unwrap();
    wait_for(&b, id, "sharing again", |t| t.status == Status::Sharing).await;

    // A share's entries move into the new folder, which becomes their base.
    let base = root.path().join("new base");
    a.move_storage(share.id, &base).await.unwrap();
    let rebased = wait_for(&a, share.id, "moved", |t| {
        t.status == Status::Sharing && t.save_folder == base
    })
    .await;
    assert_eq!(rebased.content_dir.as_deref(), Some(base.as_path()));
    assert_files(&base, &files);
    assert!(!source.join("one.bin").exists());

    a.shutdown().await;
    b.shutdown().await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn notifications_run_on_completion_and_on_error() {
    let root = tempfile::tempdir().unwrap();
    let (source, _) = album(root.path());
    let log = root.path().join("notifications.log");
    let notifying = |dir: &Path| {
        let mut config = config(dir);
        let script = format!("env | grep ^TEMPEST_ | sort >> '{}'", log.display());
        config.notifications.command = vec!["sh".into(), "-c".into(), script];
        config
    };
    let a = Engine::open(notifying(&root.path().join("a")))
        .await
        .unwrap();
    let b = Engine::open(notifying(&root.path().join("b")))
        .await
        .unwrap();
    let (share, _) = share_and_download(&a, &b, &source).await;

    // The share fails once its files are gone.
    a.stop(share.id).await.unwrap();
    wait_for(&a, share.id, "stopped", |t| t.status == Status::Stopped).await;
    fs::remove_file(source.join("two.bin")).unwrap();
    a.start(share.id).await.unwrap();
    wait_for(&a, share.id, "errored", |t| {
        matches!(t.status, Status::Errored { .. })
    })
    .await;

    let read = || fs::read_to_string(&log).unwrap_or_default();
    let deadline = Instant::now() + common::LIMIT;
    while !(read().contains("TEMPEST_EVENT=completed") && read().contains("TEMPEST_EVENT=errored"))
    {
        assert!(Instant::now() < deadline, "notifications: {}", read());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let text = read();
    let id = format!("TEMPEST_ID={}", share.id);
    assert_eq!(text.matches(&id).count(), 2, "{text}");
    assert!(text.contains("TEMPEST_NAME=album"), "{text}");
    assert!(text.contains("TEMPEST_KIND=download"), "{text}");
    assert!(text.contains("TEMPEST_KIND=share"), "{text}");
    assert!(text.contains("TEMPEST_ERROR="), "{text}");
    let content = root.path().join("b/downloads/album");
    assert!(
        text.contains(&format!("TEMPEST_CONTENT_DIR={}", content.display())),
        "{text}"
    );

    a.shutdown().await;
    b.shutdown().await;
}
