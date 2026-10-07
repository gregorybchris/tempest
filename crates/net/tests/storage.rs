//! Bundle storage: serving a shared folder, laying out and writing a download, syncing, and the
//! safety rules for paths.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use proptest::prelude::*;
use tempest_net::storage::{load_manifest, save_manifest};
use tempest_net::{Storage, StorageConfig, StorageError};
use tempest_protocol::hash::leaf_hash;
use tempest_protocol::message::Data;
use tempest_protocol::tree::{self, CHUNK_SIZE, InvalidRequest, VerifyError, tree_width};
use tempest_protocol::{ChunkLayout, Entry, FileInfo, Hash, Manifest, RangeList};

/// A file of `size` bytes where byte `i` is `(i + seed) mod 256`.
fn contents(size: usize, seed: u8) -> Vec<u8> {
    (0..size).map(|i| (i as u8).wrapping_add(seed)).collect()
}

fn file_entry(path: &str, data: &[u8], executable: bool) -> Entry {
    Entry::file(
        path,
        FileInfo {
            size: data.len() as u64,
            root: tree::root(data),
            executable,
            modified: 1_700_000_000_000,
        },
    )
}

/// Writes `files` under `dir` and returns their manifest, with `extra` entries added.
fn make_bundle(dir: &Path, files: &[(&str, Vec<u8>)], extra: Vec<Entry>) -> Arc<Manifest> {
    let mut entries = extra;
    for (path, data) in files {
        let fs_path = dir.join(path);
        fs::create_dir_all(fs_path.parent().unwrap()).unwrap();
        fs::write(&fs_path, data).unwrap();
        entries.push(file_entry(path, data, false));
    }
    Arc::new(Manifest::new("bundle", "", 0, "test", entries).unwrap())
}

/// Every valid Request of `manifest`'s bundle, as `(start, count)`.
fn all_requests(manifest: &Manifest) -> Vec<(u32, u32)> {
    let layout = ChunkLayout::new(manifest);
    let mut requests = Vec::new();
    for file in layout.files() {
        let width = tree_width(file.chunk_count.into()) as u32;
        let mut count = 1;
        while count <= width {
            for position in (0..file.chunk_count).step_by(count as usize) {
                requests.push((file.first_chunk + position, count));
            }
            count *= 2;
        }
    }
    requests
}

/// Verifies `data` for the Request `(start, count)` of `manifest`'s bundle.
fn verify(manifest: &Manifest, start: u32, count: u32, data: &Data) -> Result<(), VerifyError> {
    let layout = ChunkLayout::new(manifest);
    let location = layout.locate(start).unwrap();
    let file = layout.files()[location.file];
    let info = manifest.entries()[file.entry].as_file().unwrap();
    tree::verify_data(
        info.size,
        &info.root,
        location.position,
        count,
        &data.proof,
        &data.chunks,
    )
}

fn hex(hash: &Hash) -> String {
    hash.to_string()
}

fn quick_syncs() -> StorageConfig {
    StorageConfig {
        sync_interval: Duration::from_millis(10),
        ..StorageConfig::default()
    }
}

#[tokio::test]
async fn shared_files_answer_the_messages_md_test_vectors() {
    let dir = tempfile::tempdir().unwrap();
    let file = (0..40_000).map(|i| i as u8).collect::<Vec<u8>>();
    let manifest = make_bundle(dir.path(), &[("test", file.clone())], vec![]);
    assert_eq!(
        hex(&manifest.entries()[0].as_file().unwrap().root),
        "cf6700fcd8e64ac06a512eac8b2f94101104f0e199c5fbfa2c4b43adb562c2f8"
    );
    let storage = Storage::open_shared(dir.path(), manifest.clone(), StorageConfig::default())
        .await
        .unwrap();

    let padding = "0000000000000000000000000000000000000000000000000000000000000000";
    let chunks_0_1 = "1b53726ec87fc16451460d18b88ed388f714a8f983e0ea37fb43620fc816e115";
    let chunk_2 = "59aef01004079909de1326890705ff0009732e28a43a5c8a8f3d7e8ed7c1af7c";
    let vectors: [(u32, u32, &[&str]); 4] = [
        (2, 1, &[padding, chunks_0_1]),
        (0, 2, &[chunk_2]),
        (2, 2, &[chunks_0_1]),
        (0, 4, &[]),
    ];
    for (start, count, proof) in vectors {
        let data = storage.read(start, count).await.unwrap();
        let proof_hex: Vec<String> = data.proof.iter().map(hex).collect();
        assert_eq!(proof_hex, proof, "start {start}, count {count}");
        let begin = start as usize * CHUNK_SIZE;
        let end = (begin + count as usize * CHUNK_SIZE).min(file.len());
        assert_eq!(data.chunks, file[begin..end]);
        verify(&manifest, start, count, &data).unwrap();
    }
    // The last chunk is short, and a range into the padding carries only the chunks that exist.
    assert_eq!(
        storage.read(2, 1).await.unwrap().chunks.len(),
        40_000 - 2 * CHUNK_SIZE
    );
    assert_eq!(storage.read(0, 4).await.unwrap().chunks.len(), 40_000);
}

#[tokio::test]
async fn shared_bundles_serve_every_valid_request() {
    let dir = tempfile::tempdir().unwrap();
    let files = [
        ("a/big", contents(5 * CHUNK_SIZE + 7, 1)),
        ("a/empty", vec![]),
        ("b", contents(CHUNK_SIZE, 2)),
        ("c", contents(1, 3)),
    ];
    let manifest = make_bundle(dir.path(), &files, vec![Entry::directory("d")]);
    let storage = Storage::open_shared(dir.path(), manifest.clone(), StorageConfig::default())
        .await
        .unwrap();
    for (start, count) in all_requests(&manifest) {
        let data = storage.read(start, count).await.unwrap();
        verify(&manifest, start, count, &data).unwrap();
    }
    // The empty file's one chunk is empty.
    let empty = storage.read(6, 1).await.unwrap();
    assert!(empty.chunks.is_empty());
    assert!(empty.proof.is_empty());

    let invalid = |r: Result<Data, StorageError>| match r {
        Err(StorageError::InvalidRequest(e)) => e,
        other => panic!("expected an invalid request, got {other:?}"),
    };
    assert_eq!(
        invalid(storage.read(1, 2).await),
        InvalidRequest::Misaligned
    );
    assert_eq!(
        invalid(storage.read(0, 16).await),
        InvalidRequest::PastEndOfTree
    );
    assert_eq!(
        invalid(storage.read(9, 1).await),
        InvalidRequest::OutsideBundle
    );
    assert!(matches!(
        storage.write(0, 1, Data::default()).await,
        Err(StorageError::ReadOnly)
    ));
    assert_eq!(
        storage.manifest_bytes().as_ref(),
        tempest_protocol::Encode::to_bytes(manifest.as_ref())
    );
}

#[tokio::test]
async fn a_changed_shared_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = make_bundle(dir.path(), &[("f", contents(20_000, 0))], vec![]);
    fs::write(dir.path().join("f"), contents(20_000, 1)).unwrap();
    let error = Storage::open_shared(dir.path(), manifest.clone(), StorageConfig::default())
        .await
        .unwrap_err();
    assert!(matches!(error, StorageError::Unusable { .. }), "{error}");
    fs::remove_file(dir.path().join("f")).unwrap();
    let error = Storage::open_shared(dir.path(), manifest, StorageConfig::default())
        .await
        .unwrap_err();
    assert!(matches!(error, StorageError::Io { .. }), "{error}");
}

/// Downloads `manifest` from `source` into `dest`, one Request per item of `requests`.
async fn transfer(source: &Storage, dest: &Storage, requests: &[(u32, u32)]) -> Vec<bool> {
    let mut completes = Vec::new();
    for &(start, count) in requests {
        let data = source.read(start, count).await.unwrap();
        let written = dest.write(start, count, data.clone()).await.unwrap();
        completes.push(written.complete);
        // What was just stored can be served again, with the same proof.
        assert_eq!(dest.read(start, count).await.unwrap(), data);
    }
    completes
}

#[tokio::test]
async fn a_download_lays_out_every_entry() {
    let source = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let files = [
        ("docs/a.txt", contents(3 * CHUNK_SIZE + 1, 0)),
        ("docs/empty.txt", vec![]),
        ("run.sh", b"#!/bin/sh\n".to_vec()),
    ];
    for (path, data) in &files {
        fs::create_dir_all(source.path().join(path).parent().unwrap()).unwrap();
        fs::write(source.path().join(path), data).unwrap();
    }
    let manifest = Arc::new(
        Manifest::new(
            "bundle",
            "",
            0,
            "test",
            vec![
                file_entry("docs/a.txt", &files[0].1, false),
                file_entry("docs/empty.txt", &files[1].1, false),
                file_entry("run.sh", &files[2].1, true),
                Entry::symlink("docs/link", "../run.sh"),
                Entry::directory("nothing/here"),
            ],
        )
        .unwrap(),
    );
    let shared = Storage::open_shared(source.path(), manifest.clone(), StorageConfig::default())
        .await
        .unwrap();
    let (download, have) =
        Storage::create_download(dest.path().join("out"), manifest.clone(), quick_syncs())
            .await
            .unwrap();
    // Only the empty file's chunk is present.
    assert_eq!(have, RangeList::from_chunks([4]));
    let out = dest.path().join("out");
    assert_eq!(
        fs::metadata(out.join("docs/a.txt")).unwrap().len(),
        3 * CHUNK_SIZE as u64 + 1
    );
    assert!(out.join("nothing/here").is_dir());
    assert_eq!(
        fs::read_link(out.join("docs/link")).unwrap(),
        Path::new("../run.sh")
    );

    // Not stored yet.
    assert!(matches!(
        download.read(0, 1).await,
        Err(StorageError::Missing { .. })
    ));

    let completes = transfer(&shared, &download, &[(0, 4), (5, 1)]).await;
    assert_eq!(completes, [false, true]);
    for (path, data) in &files {
        assert_eq!(&fs::read(out.join(path)).unwrap(), data, "{path}");
    }
    assert_eq!(fs::read(out.join("docs/link")).unwrap(), files[2].1);
    let modified = fs::metadata(out.join("docs/a.txt"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(
        modified,
        std::time::UNIX_EPOCH + Duration::from_millis(1_700_000_000_000)
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &str| fs::metadata(out.join(p)).unwrap().permissions().mode();
        assert_ne!(mode("run.sh") & 0o100, 0);
        assert_eq!(mode("docs/a.txt") & 0o111, 0);
    }
    download.close().await.unwrap();
}

/// Bumps `path`'s modification time, as a later write would.
fn touch(path: &Path) {
    let file = fs::OpenOptions::new().write(true).open(path).unwrap();
    let later = file.metadata().unwrap().modified().unwrap() + Duration::from_secs(5);
    file.set_modified(later).unwrap();
}

#[tokio::test]
async fn a_download_resumes_from_its_checkpoint() {
    let source = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let files = [
        ("a", contents(5 * CHUNK_SIZE + 7, 0)),
        ("b", contents(3 * CHUNK_SIZE, 1)),
    ];
    let manifest = make_bundle(source.path(), &files, vec![]);
    let shared = Storage::open_shared(source.path(), manifest.clone(), StorageConfig::default())
        .await
        .unwrap();
    let (download, _) =
        Storage::create_download(dest.path(), manifest.clone(), StorageConfig::default())
            .await
            .unwrap();
    // Chunks 1 and 4 of "a" and the first two of "b" (bundle chunks 6 and 7).
    transfer(&shared, &download, &[(1, 1), (4, 1), (6, 2)]).await;
    let checkpoint = download.close().await.unwrap();
    assert_eq!(checkpoint.chunks_stored(), 4);
    let checkpoint = tempest_net::Checkpoint::from_bytes(&checkpoint.to_bytes()).unwrap();

    // Unchanged files: the recorded chunks come back, and can be served with their proofs.
    let (resumed, have) = Storage::resume_download(
        dest.path(),
        manifest.clone(),
        quick_syncs(),
        checkpoint.clone(),
    )
    .await
    .unwrap();
    assert_eq!(have, RangeList::from_chunks([1, 4, 6, 7]));
    for (start, count) in [(1, 1), (4, 1), (6, 2)] {
        let data = resumed.read(start, count).await.unwrap();
        verify(&manifest, start, count, &data).unwrap();
    }
    let completes = transfer(&shared, &resumed, &[(0, 1), (2, 2), (5, 1), (8, 1)]).await;
    assert_eq!(completes, [false, false, false, true]);
    resumed.close().await.unwrap();
    for (path, data) in &files {
        assert_eq!(&fs::read(dest.path().join(path)).unwrap(), data, "{path}");
    }
}

#[tokio::test]
async fn changed_files_are_checked_against_the_checkpoint() {
    let source = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let files = [
        ("a", contents(4 * CHUNK_SIZE, 0)),
        ("b", contents(2 * CHUNK_SIZE, 1)),
    ];
    let manifest = make_bundle(source.path(), &files, vec![]);
    let shared = Storage::open_shared(source.path(), manifest.clone(), StorageConfig::default())
        .await
        .unwrap();
    let (download, _) =
        Storage::create_download(dest.path(), manifest.clone(), StorageConfig::default())
            .await
            .unwrap();
    transfer(&shared, &download, &[(0, 2), (4, 1)]).await;
    let checkpoint = download.close().await.unwrap();

    // "a" changed since: its chunk 1 is damaged, so only chunk 0 is kept. "b" shrank, so it
    // starts over.
    let a = dest.path().join("a");
    let mut bytes = fs::read(&a).unwrap();
    bytes[CHUNK_SIZE + 3] ^= 1;
    fs::write(&a, bytes).unwrap();
    touch(&a);
    let b = dest.path().join("b");
    fs::OpenOptions::new()
        .write(true)
        .open(&b)
        .unwrap()
        .set_len(10)
        .unwrap();

    let (resumed, have) =
        Storage::resume_download(dest.path(), manifest.clone(), quick_syncs(), checkpoint)
            .await
            .unwrap();
    assert_eq!(have, RangeList::from_chunks([0]));
    assert_eq!(fs::metadata(&b).unwrap().len(), 2 * CHUNK_SIZE as u64);
    transfer(&shared, &resumed, &[(1, 1), (2, 2), (4, 2)]).await;
    resumed.close().await.unwrap();
    for (path, data) in &files {
        assert_eq!(&fs::read(dest.path().join(path)).unwrap(), data, "{path}");
    }
}

#[tokio::test]
async fn a_checkpoint_that_does_not_fit_the_manifest_is_ignored() {
    let source = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let manifest = make_bundle(source.path(), &[("a", contents(3 * CHUNK_SIZE, 0))], vec![]);
    let other_manifest = make_bundle(other.path(), &[("a", contents(3 * CHUNK_SIZE, 9))], vec![]);
    let foreign = Storage::open_shared(other.path(), other_manifest, StorageConfig::default())
        .await
        .unwrap()
        .close()
        .await
        .unwrap();

    // The foreign tree contradicts this manifest's root, so nothing is trusted.
    let (download, have) =
        Storage::resume_download(dest.path(), manifest, StorageConfig::default(), foreign)
            .await
            .unwrap();
    assert_eq!(have, RangeList::new());
    download.close().await.unwrap();
}

#[tokio::test]
async fn a_shared_bundle_resumes_without_hashing_unchanged_files() {
    let source = tempfile::tempdir().unwrap();
    let files = [("a", contents(3 * CHUNK_SIZE, 0)), ("b", contents(100, 1))];
    let manifest = make_bundle(source.path(), &files, vec![]);
    let shared = Storage::open_shared(source.path(), manifest.clone(), StorageConfig::default())
        .await
        .unwrap();
    let checkpoint = shared.close().await.unwrap();

    let resumed = Storage::resume_shared(
        source.path(),
        manifest.clone(),
        StorageConfig::default(),
        checkpoint.clone(),
    )
    .await
    .unwrap();
    for (start, count) in all_requests(&manifest) {
        let data = resumed.read(start, count).await.unwrap();
        verify(&manifest, start, count, &data).unwrap();
    }
    resumed.close().await.unwrap();

    // A touched file with the same contents still passes; a changed one fails.
    let a = source.path().join("a");
    touch(&a);
    Storage::resume_shared(
        source.path(),
        manifest.clone(),
        StorageConfig::default(),
        checkpoint.clone(),
    )
    .await
    .unwrap();
    let mut bytes = fs::read(&a).unwrap();
    bytes[0] ^= 1;
    fs::write(&a, bytes).unwrap();
    let error = Storage::resume_shared(
        source.path(),
        manifest,
        StorageConfig::default(),
        checkpoint,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("has changed"), "{error}");
}

#[tokio::test]
async fn bad_data_is_never_stored() {
    let source = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let manifest = make_bundle(source.path(), &[("f", contents(3 * CHUNK_SIZE, 0))], vec![]);
    let shared = Storage::open_shared(source.path(), manifest.clone(), StorageConfig::default())
        .await
        .unwrap();
    let (download, _) = Storage::create_download(dest.path(), manifest, StorageConfig::default())
        .await
        .unwrap();

    let mut data = shared.read(0, 2).await.unwrap();
    data.chunks[100] ^= 1;
    let error = download.write(0, 2, data.clone()).await.unwrap_err();
    assert!(
        matches!(error, StorageError::Verify(VerifyError::RootMismatch)),
        "{error}"
    );
    data.chunks.push(0);
    let error = download.write(0, 2, data).await.unwrap_err();
    assert!(
        matches!(
            error,
            StorageError::Verify(VerifyError::WrongDataLength { .. })
        ),
        "{error}"
    );
    let error = download.write(1, 2, Data::default()).await.unwrap_err();
    assert!(
        matches!(
            error,
            StorageError::InvalidRequest(InvalidRequest::Misaligned)
        ),
        "{error}"
    );

    assert!(download.have().await.unwrap().is_empty());
    assert!(matches!(
        download.read(0, 1).await,
        Err(StorageError::Missing { .. })
    ));
    assert_eq!(
        fs::read(dest.path().join("f")).unwrap(),
        vec![0; 3 * CHUNK_SIZE]
    );
    assert_eq!(download.stats().await.unwrap().writes, 0);
}

#[tokio::test]
async fn syncs_are_batched() {
    let source = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let manifest = make_bundle(
        source.path(),
        &[("f", contents(16 * CHUNK_SIZE, 0))],
        vec![],
    );
    let shared = Storage::open_shared(source.path(), manifest.clone(), StorageConfig::default())
        .await
        .unwrap();
    let config = StorageConfig {
        sync_bytes: 4 * CHUNK_SIZE as u64,
        sync_interval: Duration::from_secs(3600),
        ..StorageConfig::default()
    };
    let (download, _) = Storage::create_download(dest.path(), manifest, config)
        .await
        .unwrap();

    // Three chunks: below the threshold, so nothing is synced until a flush.
    transfer(&shared, &download, &[(0, 1), (1, 1), (2, 1)]).await;
    assert_eq!(download.stats().await.unwrap().syncs, 0);
    download.flush().await.unwrap();
    assert_eq!(download.stats().await.unwrap().syncs, 1);

    // Twelve more, one at a time: a sync every four chunks, then one on completion.
    let rest: Vec<_> = (3..15).map(|i| (i, 1)).collect();
    transfer(&shared, &download, &rest).await;
    download.flush().await.unwrap();
    let stats = download.stats().await.unwrap();
    assert_eq!(stats.writes, 15);
    assert_eq!(stats.syncs, 4);
    let completes = transfer(&shared, &download, &[(15, 1)]).await;
    assert_eq!(completes, [true]);
    let stats = download.stats().await.unwrap();
    assert_eq!(stats.syncs, 5);

    // Data already stored is verified but not written again.
    transfer(&shared, &download, &[(0, 16)]).await;
    assert_eq!(download.stats().await.unwrap().writes, 16);
}

#[tokio::test]
async fn syncs_happen_after_the_interval() {
    let source = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let manifest = make_bundle(source.path(), &[("f", contents(2 * CHUNK_SIZE, 0))], vec![]);
    let shared = Storage::open_shared(source.path(), manifest.clone(), StorageConfig::default())
        .await
        .unwrap();
    let (download, _) = Storage::create_download(dest.path(), manifest, quick_syncs())
        .await
        .unwrap();
    transfer(&shared, &download, &[(0, 1)]).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(download.stats().await.unwrap().syncs, 1);
}

#[tokio::test]
async fn closing_syncs_and_stops_the_task() {
    let source = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let manifest = make_bundle(source.path(), &[("f", contents(2 * CHUNK_SIZE, 0))], vec![]);
    let shared = Storage::open_shared(source.path(), manifest.clone(), StorageConfig::default())
        .await
        .unwrap();
    let (download, _) = Storage::create_download(dest.path(), manifest, StorageConfig::default())
        .await
        .unwrap();
    let other = download.clone();
    transfer(&shared, &download, &[(0, 1)]).await;
    download.close().await.unwrap();
    assert!(matches!(other.stats().await, Err(StorageError::Stopped)));
    assert_eq!(
        fs::read(dest.path().join("f")).unwrap()[..CHUNK_SIZE],
        contents(CHUNK_SIZE, 0)
    );
}

#[tokio::test]
async fn existing_complete_files_are_kept() {
    let source = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let files = [
        ("done", contents(2 * CHUNK_SIZE, 0)),
        ("partial", contents(2 * CHUNK_SIZE, 1)),
        ("wrong_size", contents(CHUNK_SIZE + 1, 2)),
    ];
    let manifest = make_bundle(source.path(), &files, vec![]);
    fs::write(dest.path().join("done"), &files[0].1).unwrap();
    fs::write(dest.path().join("partial"), contents(2 * CHUNK_SIZE, 9)).unwrap();
    fs::write(dest.path().join("wrong_size"), b"short").unwrap();
    let (download, have) =
        Storage::create_download(dest.path(), manifest, StorageConfig::default())
            .await
            .unwrap();
    assert_eq!(have, RangeList::from_chunks([0, 1]));
    assert_eq!(download.read(0, 2).await.unwrap().chunks, files[0].1);
    assert_eq!(
        fs::metadata(dest.path().join("wrong_size")).unwrap().len(),
        CHUNK_SIZE as u64 + 1
    );
}

#[test]
fn manifests_with_escaping_paths_are_invalid() {
    let entry = |path: &str| file_entry(path, b"x", false);
    for path in [
        "../x",
        "/etc/passwd",
        "a/../../x",
        "a/./b",
        "C:x",
        "a\\..\\x",
    ] {
        assert!(
            Manifest::new("b", "", 0, "", vec![entry(path)]).is_err(),
            "{path}"
        );
    }
    for target in ["../outside", "../../x", "/etc/passwd", "a/../../x"] {
        let entries = vec![entry("a"), Entry::symlink("link", target)];
        assert!(Manifest::new("b", "", 0, "", entries).is_err(), "{target}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn existing_symlinks_are_never_followed() {
    use std::os::unix::fs::symlink;

    let source = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let manifest = make_bundle(source.path(), &[("dir/f", contents(10, 0))], vec![]);
    let start =
        || Storage::create_download(dest.path(), manifest.clone(), StorageConfig::default());

    // A directory of the bundle that is a symlink to somewhere else.
    symlink(outside.path(), dest.path().join("dir")).unwrap();
    let error = start().await.unwrap_err();
    assert!(matches!(error, StorageError::Unusable { .. }), "{error}");
    assert!(fs::read_dir(outside.path()).unwrap().next().is_none());

    // A file of the bundle that is a symlink to somewhere else.
    fs::remove_file(dest.path().join("dir")).unwrap();
    fs::create_dir(dest.path().join("dir")).unwrap();
    let victim = outside.path().join("victim");
    fs::write(&victim, b"keep").unwrap();
    symlink(&victim, dest.path().join("dir/f")).unwrap();
    let error = start().await.unwrap_err();
    assert!(matches!(error, StorageError::Unusable { .. }), "{error}");
    assert_eq!(fs::read(&victim).unwrap(), b"keep");

    // A symlink of the bundle where something else already is.
    fs::remove_file(dest.path().join("dir/f")).unwrap();
    let manifest = Arc::new(
        Manifest::new(
            "b",
            "",
            0,
            "",
            vec![file_entry("f", b"x", false), Entry::symlink("link", "f")],
        )
        .unwrap(),
    );
    symlink(&victim, dest.path().join("link")).unwrap();
    let error = Storage::create_download(dest.path(), manifest, StorageConfig::default())
        .await
        .unwrap_err();
    assert!(matches!(error, StorageError::Unusable { .. }), "{error}");
}

#[test]
fn manifests_are_saved_and_loaded() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = make_bundle(dir.path(), &[("f", contents(10, 0))], vec![]);
    let path = dir.path().join("bundle.manifest");
    save_manifest(
        &path,
        &tempest_protocol::Encode::to_bytes(manifest.as_ref()),
    )
    .unwrap();
    assert_eq!(
        load_manifest(&path, &manifest.bundle_id()).unwrap(),
        *manifest
    );
    let error = load_manifest(&path, &leaf_hash(b"other")).unwrap_err();
    assert!(
        matches!(error, StorageError::WrongManifest { .. }),
        "{error}"
    );
}

/// A bundle shape for property tests: file sizes in chunks plus a remainder, some empty.
fn bundle_shape() -> impl Strategy<Value = Vec<usize>> {
    let size = prop_oneof![
        Just(0),
        1..=CHUNK_SIZE,
        (0usize..9, 0..CHUNK_SIZE).prop_map(|(chunks, extra)| chunks * CHUNK_SIZE + extra),
    ];
    prop::collection::vec(size, 1..5)
}

/// A random cover of every file's chunks by aligned nodes, in random order.
fn cover(manifest: &Manifest, splits: &[bool]) -> Vec<(u32, u32)> {
    let layout = ChunkLayout::new(manifest);
    let mut splits = splits.iter().cycle();
    let mut requests = Vec::new();
    for file in layout.files() {
        // Split the root node down to leaves at random.
        let mut stack = vec![(0u32, tree_width(file.chunk_count.into()) as u32)];
        while let Some((position, count)) = stack.pop() {
            if position >= file.chunk_count {
                continue;
            }
            if count > 1 && *splits.next().unwrap() {
                stack.push((position, count / 2));
                stack.push((position + count / 2, count / 2));
            } else {
                requests.push((file.first_chunk + position, count));
            }
        }
    }
    requests
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Downloading every chunk, in any order and in any request sizes, reproduces the source.
    #[test]
    fn downloads_reproduce_the_source(
        sizes in bundle_shape(),
        splits in prop::collection::vec(any::<bool>(), 1..64),
        order in any::<u64>(),
        parallel_jobs in 1usize..4,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let source = tempfile::tempdir().unwrap();
            let dest = tempfile::tempdir().unwrap();
            let files: Vec<(String, Vec<u8>)> = sizes
                .iter()
                .enumerate()
                .map(|(i, &size)| (format!("d{}/f{i}", i % 2), contents(size, i as u8)))
                .collect();
            let borrowed: Vec<(&str, Vec<u8>)> =
                files.iter().map(|(p, d)| (p.as_str(), d.clone())).collect();
            let manifest = make_bundle(source.path(), &borrowed, vec![]);
            let shared =
                Storage::open_shared(source.path(), manifest.clone(), StorageConfig::default())
                    .await
                    .unwrap();
            let config = StorageConfig {
                parallel_jobs,
                sync_bytes: 3 * CHUNK_SIZE as u64,
                ..StorageConfig::default()
            };
            let (download, have) = Storage::create_download(dest.path(), manifest.clone(), config)
                .await
                .unwrap();

            // Shuffle the cover deterministically from `order`.
            let mut requests = cover(&manifest, &splits);
            let mut state = order;
            for i in (1..requests.len()).rev() {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                requests.swap(i, (state >> 33) as usize % (i + 1));
            }

            // Writes run concurrently, up to the storage's parallel jobs.
            let writes: Vec<_> = requests
                .into_iter()
                .map(|(start, count)| {
                    let (shared, download) = (shared.clone(), download.clone());
                    tokio::spawn(async move {
                        let data = shared.read(start, count).await.unwrap();
                        download.write(start, count, data).await.unwrap().complete
                    })
                })
                .collect();
            let mut completes = 0;
            for write in writes {
                completes += usize::from(write.await.unwrap());
            }
            let all = RangeList::from_chunks(0..ChunkLayout::new(&manifest).chunk_count());
            // Exactly one write completes the bundle, unless every file was empty.
            prop_assert_eq!(completes, usize::from(have != all));
            prop_assert_eq!(download.have().await.unwrap(), all);
            let stats = download.stats().await.unwrap();
            download.close().await.unwrap();
            prop_assert!(stats.syncs <= stats.writes + 1);
            for (path, data) in &files {
                prop_assert_eq!(&fs::read(dest.path().join(path)).unwrap(), data);
            }
            Ok(())
        })?;
    }
}
