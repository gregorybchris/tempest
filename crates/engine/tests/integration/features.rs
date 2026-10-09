//! The engine's v1 features beyond the basic controls: speed and sharing limits, rechecking,
//! moving storage, notifications and a home relay.

use crate::common;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use tempest_api::SharingStop;
use tempest_engine::store::{Kind, Store, TransferRow};
use tempest_engine::{
    AddOptions, Engine, EngineError, EngineEvent, ShareOptions, Status, Subscription, TransferInfo,
};
use tempest_net::{Identity, RelayStatus};
use tempest_protocol::relay::RelayHost;
use tempest_protocol::wire::Encode;
use tempest_protocol::{Entry, FileInfo, Hash, Link, Manifest, RelayIdentity};
use tempest_relay::{Relay, RelayConfig};

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
    assert_eq!(done.stopped_by, Some(SharingStop::Ratio { ratio: 1.0 }));
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
async fn a_share_recheck_never_changes_its_files() {
    let root = tempfile::tempdir().unwrap();
    let (source, _) = album(root.path());
    let a = Engine::open(config(&root.path().join("a"))).await.unwrap();
    let share = a.add_path(&source, ShareOptions::default()).await.unwrap();
    wait_for(&a, share.id, "sharing", |t| t.status == Status::Sharing).await;

    // The user edits both files: one keeps its size, the other shrinks.
    let edited = contents(MIB + 12_345, 9);
    fs::write(source.join("one.bin"), &edited).unwrap();
    fs::write(source.join("two.bin"), b"short").unwrap();
    a.recheck(share.id).await.unwrap();
    let failed = wait_for(&a, share.id, "failed", |t| {
        matches!(t.status, Status::Errored { .. })
    })
    .await;
    let Status::Errored { reason } = &failed.status else {
        unreachable!()
    };
    assert!(reason.contains("has changed"), "{reason}");
    assert!(fs::read(source.join("one.bin")).unwrap() == edited);
    assert_eq!(fs::read(source.join("two.bin")).unwrap(), b"short");
    a.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn names_in_either_unicode_form_are_shared() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("accents");
    fs::create_dir_all(&source).unwrap();
    // "é" decomposed, as macOS's older file systems store names. Manifests hold them composed.
    let data = contents(MIB, 3);
    fs::write(source.join("e\u{301}.bin"), &data).unwrap();
    fs::write(source.join("plain.bin"), b"plain").unwrap();
    let a = Engine::open(config(&root.path().join("a"))).await.unwrap();
    let b_dir = root.path().join("b");
    let b = Engine::open(config(&b_dir)).await.unwrap();
    share_and_download(&a, &b, &source).await;
    assert!(fs::read(b_dir.join("downloads/accents/\u{e9}.bin")).unwrap() == data);
    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_move_a_crash_interrupted_is_undone_at_the_next_start() {
    let root = tempfile::tempdir().unwrap();
    let (source, files) = album(root.path());
    let a_dir = root.path().join("a");
    let a = Engine::open(config(&a_dir)).await.unwrap();
    let share = a.add_path(&source, ShareOptions::default()).await.unwrap();
    let id = share.id;
    wait_for(&a, id, "sharing", |t| t.status == Status::Sharing).await;
    a.shutdown().await;

    // A crash in the middle of moving the share: the move was recorded and one file renamed,
    // and the record still names the old folder.
    let dir = share.content_dir.unwrap();
    let elsewhere = root.path().join("elsewhere");
    let mut store = Store::open(&a_dir.join("data/state.db")).unwrap();
    store.begin_move(&id, &elsewhere).unwrap();
    drop(store);
    fs::create_dir_all(elsewhere.join("album")).unwrap();
    fs::rename(dir.join("one.bin"), elsewhere.join("album/one.bin")).unwrap();

    let a = Engine::open(config(&a_dir)).await.unwrap();
    let back = wait_for(&a, id, "sharing", |t| t.status == Status::Sharing).await;
    assert_eq!(back.content_dir, Some(dir.clone()));
    assert_files(&dir, &files);
    assert!(!elsewhere.join("album/one.bin").exists());
    a.shutdown().await;
    let store = Store::open(&a_dir.join("data/state.db")).unwrap();
    assert!(store.moves().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_saved_transfer_too_large_to_store_fails_when_it_starts() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("a");
    fs::create_dir_all(dir.join("data")).unwrap();
    // A manifest may declare up to 64 TiB, far over what the engine stores.
    let info = FileInfo {
        size: u64::from(u32::MAX) * 16_384,
        root: Hash::new([1; 32]),
        executable: false,
        modified: 0,
    };
    let manifest = Manifest::new("huge", "", 0, "", vec![Entry::file("huge", info)]).unwrap();
    let id = manifest.bundle_id();
    let downloads = dir.join("downloads");
    let row = TransferRow {
        id,
        kind: Kind::Download,
        link: Link::new(id).to_string(),
        manifest: Some(manifest.to_bytes()),
        name: None,
        save_folder: downloads.clone(),
        content_dir: Some(downloads.clone()),
        wanted: true,
        forced: false,
        done: false,
        stopped_by: None,
        error: None,
        added: 0,
        completed: None,
        queue_position: 0,
        tags: Default::default(),
        category: None,
        settings: BTreeMap::new(),
        uploaded: 0,
        downloaded: 0,
        have: 0,
        chunks: u64::from(u32::MAX),
        seeding_time: 0,
        publish: Default::default(),
    };
    Store::open(&dir.join("data/state.db"))
        .unwrap()
        .save_transfer(&row)
        .unwrap();

    let engine = Engine::open(config(&dir)).await.unwrap();
    let failed = wait_for(&engine, id, "failed", |t| {
        matches!(t.status, Status::Errored { .. })
    })
    .await;
    let Status::Errored { reason } = &failed.status else {
        unreachable!()
    };
    assert!(reason.contains("more than"), "{reason}");
    assert!(!downloads.join("huge").exists());
    engine.shutdown().await;
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

    // A share keeps its folder name, which becomes its base in the new folder.
    let base = root.path().join("new base");
    a.move_storage(share.id, &base).await.unwrap();
    let rebased = wait_for(&a, share.id, "moved", |t| {
        t.status == Status::Sharing && t.save_folder == base
    })
    .await;
    assert_eq!(rebased.content_dir, Some(base.join("album")));
    assert_files(&base.join("album"), &files);
    assert!(!source.join("one.bin").exists());

    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_single_file_lands_in_the_save_folder_and_moves_without_a_folder() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("report.bin");
    let data = contents(MIB + 5, 3);
    fs::write(&source, &data).unwrap();
    let a = Engine::open(config(&root.path().join("a"))).await.unwrap();
    let b = Engine::open(config(&root.path().join("b"))).await.unwrap();
    let (_, done) = share_and_download(&a, &b, &source).await;
    let saved = done.save_folder.clone();
    assert_eq!(done.content_dir.as_deref(), Some(saved.as_path()));
    assert!(fs::read(saved.join("report.bin")).unwrap() == data);

    let elsewhere = root.path().join("elsewhere");
    b.move_storage(done.id, &elsewhere).await.unwrap();
    let moved = wait_for(&b, done.id, "moved", |t| {
        t.status == Status::Sharing && t.save_folder == elsewhere
    })
    .await;
    assert_eq!(moved.content_dir.as_deref(), Some(elsewhere.as_path()));
    assert!(fs::read(elsewhere.join("report.bin")).unwrap() == data);
    assert!(!saved.join("report.bin").exists());

    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_single_file_download_fails_where_the_name_is_taken() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("report.bin");
    fs::write(&source, contents(MIB, 4)).unwrap();
    let a = Engine::open(config(&root.path().join("a"))).await.unwrap();
    let b = Engine::open(config(&root.path().join("b"))).await.unwrap();
    let share = a.add_path(&source, ShareOptions::default()).await.unwrap();
    wait_for(&a, share.id, "sharing", |t| t.status == Status::Sharing).await;
    let taken = root.path().join("taken");
    fs::create_dir_all(&taken).unwrap();
    fs::write(taken.join("report.bin"), "mine").unwrap();
    let options = AddOptions {
        save_folder: Some(taken.clone()),
        ..AddOptions::default()
    };
    b.add_link(&share.link, options).await.unwrap();
    let failed = wait_for(&b, share.id, "errored", |t| {
        matches!(t.status, Status::Errored { .. })
    })
    .await;
    let Status::Errored { reason } = &failed.status else {
        unreachable!()
    };
    assert!(reason.contains("report.bin"), "{reason}");
    assert_eq!(
        fs::read_to_string(taken.join("report.bin")).unwrap(),
        "mine"
    );

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

/// Waits until `engine`'s home relay status satisfies `done`.
async fn wait_for_relay(engine: &Engine, done: impl Fn(&RelayStatus) -> bool) -> RelayStatus {
    let wait = async {
        loop {
            let status = engine.relay();
            if done(&status) {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    tokio::time::timeout(common::LIMIT, wait).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_configured_relay_lends_the_address_in_share_links() {
    let root = tempfile::tempdir().unwrap();
    let (source, files) = album(root.path());
    let identity = Identity::generate().unwrap();
    let relay = Arc::new(Relay::bind(&identity, RelayConfig::localhost()).unwrap());
    let serving = relay.clone();
    let (_stop, stopped) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        serving
            .serve(async {
                let _ = stopped.await;
            })
            .await;
    });
    let relay_addr = relay.local_addr().unwrap();
    let relay_identity = RelayIdentity {
        key: relay.key(),
        host: RelayHost::Ip(relay_addr.ip()),
        port: relay_addr.port(),
    };

    let mut a_config = config(&root.path().join("a"));
    a_config.relays.known = vec![relay_identity];
    let a = Engine::open(a_config).await.unwrap();
    let b = Engine::open(config(&root.path().join("b"))).await.unwrap();
    let registered = |s: &RelayStatus| matches!(s, RelayStatus::Registered { .. });
    let relay_address = wait_for_relay(&a, registered).await.relay_address();
    assert!(relay_address.is_some());
    assert_eq!(b.relay(), RelayStatus::Off);

    let (share, done) = share_and_download(&a, &b, &source).await;
    let link: Link = share.link.parse().unwrap();
    assert_eq!(link.relay, relay_address);
    assert_files(done.content_dir.as_ref().unwrap(), &files);

    // Without relays the home relay is left, and links lose the relay address.
    let none = BTreeMap::from([("relays.known".to_owned(), json!([]))]);
    a.change_settings(none).await.unwrap();
    wait_for_relay(&a, |s| *s == RelayStatus::Off).await;
    let info = a.get(share.id).await.unwrap();
    let link: Link = info.link.parse().unwrap();
    assert_eq!(link.relay, None);
    a.shutdown().await;
    b.shutdown().await;
}
