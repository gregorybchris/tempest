//! Looking inside transfers through the API: files, peers and chunk maps, from a real share and
//! download between two engines.

mod common;

use std::fs;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tempest_engine::api::{Access, router};
use tempest_engine::config::SpeedLimitSettings;
use tempest_engine::inspect::{ChunkMapInfo, FileInfo, PeerInfo};
use tempest_engine::{AddOptions, Engine, EngineConfig, ShareOptions, Status};
use tower::ServiceExt;

use common::{LIMIT, config, contents, nobodys_link, wait_for};

const MIB: usize = 1 << 20;

/// GETs `uri` and returns the status and the JSON body.
async fn get(app: &Router, uri: &str) -> (StatusCode, Value) {
    let request = Request::get(uri).body(Body::empty()).unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

/// GETs `uri`, which must succeed, as a `T`.
async fn fetch<T: serde::de::DeserializeOwned>(app: &Router, uri: &str) -> T {
    let (status, body) = get(app, uri).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    serde_json::from_value(body).unwrap()
}

/// Polls `uri` until `done` holds for what it returns.
async fn poll<T: serde::de::DeserializeOwned>(
    app: &Router,
    uri: &str,
    what: &str,
    done: impl Fn(&T) -> bool,
) -> T {
    let wait = async {
        loop {
            let value: T = fetch(app, uri).await;
            if done(&value) {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::time::timeout(LIMIT, wait)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

fn assert_covers(map: &ChunkMapInfo, cells: usize) {
    assert_eq!(map.cells.len(), cells);
    let covered: u32 = map.cells.iter().map(|cell| cell.chunks()).sum();
    assert_eq!(covered, map.chunks);
}

#[tokio::test(flavor = "multi_thread")]
async fn files_peers_and_chunks_show_a_share_and_a_download() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("album");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("one.bin"), contents(MIB, 1)).unwrap();
    fs::write(source.join("two.bin"), contents(MIB / 2, 2)).unwrap();

    let a = Engine::open(config(&root.path().join("a"))).await.unwrap();
    // A slow download, to see it while it runs.
    let b = Engine::open(EngineConfig {
        limits: SpeedLimitSettings {
            download: Some(MIB as u64 / 2),
            ..SpeedLimitSettings::default()
        },
        ..config(&root.path().join("b"))
    })
    .await
    .unwrap();
    let api_a = router(a.clone(), Access::Local);
    let api_b = router(b.clone(), Access::Local);

    let share = a.add_path(&source, ShareOptions::default()).await.unwrap();
    let id = share.id;
    wait_for(&a, id, "sharing", |t| t.status == Status::Sharing).await;
    let files: Vec<FileInfo> = fetch(&api_a, &format!("/v1/transfers/{id}/files")).await;
    let summary: Vec<_> = files
        .iter()
        .map(|f| (f.path.as_str(), f.size, f.chunks, f.progress))
        .collect();
    assert_eq!(
        summary,
        [
            ("one.bin", MIB as u64, 64, 1.0),
            ("two.bin", MIB as u64 / 2, 32, 1.0)
        ]
    );
    let map: ChunkMapInfo = fetch(&api_a, &format!("/v1/transfers/{id}/chunks?cells=10")).await;
    assert!(map.running);
    assert_eq!(map.chunks, 96);
    assert_covers(&map, 10);
    assert!(map.cells.iter().all(|cell| cell.have == cell.chunks()));

    b.add_link(&share.link, AddOptions::default())
        .await
        .unwrap();
    let peers: Vec<PeerInfo> = poll(
        &api_b,
        &format!("/v1/transfers/{id}/peers"),
        "the download's connection to the share",
        |peers: &Vec<PeerInfo>| peers.iter().any(|p| p.open && p.serving_us),
    )
    .await;
    let peer = &peers[0];
    assert_eq!(peer.peer_id, a.peer_id().to_string());
    assert!(peer.address.starts_with("127.0.0.1:"), "{peer:?}");
    assert_eq!(
        (peer.path.as_str(), peer.direction.as_str()),
        ("direct", "dialed")
    );
    assert!(peer.we_want);
    assert_eq!(peer.progress, Some(1.0));

    // While it downloads, the map counts the share as holding every chunk.
    let map: ChunkMapInfo = fetch(&api_b, &format!("/v1/transfers/{id}/chunks?cells=4")).await;
    assert!(map.running);
    assert_covers(&map, 4);
    assert!(
        map.cells.iter().all(|cell| cell.availability == 1),
        "{map:?}"
    );
    // The cells asked for are capped by the chunks there are.
    let map: ChunkMapInfo = fetch(&api_b, &format!("/v1/transfers/{id}/chunks?cells=500")).await;
    assert_covers(&map, 96);

    wait_for(&b, id, "downloaded", |t| t.status == Status::Sharing).await;
    let files: Vec<FileInfo> = fetch(&api_b, &format!("/v1/transfers/{id}/files")).await;
    assert!(files.iter().all(|f| f.have == f.chunks), "{files:?}");

    // Stopped, it shows what it stored, and no peers.
    b.stop(id).await.unwrap();
    wait_for(&b, id, "stopped", |t| t.status == Status::Stopped).await;
    let map: ChunkMapInfo = poll(
        &api_b,
        &format!("/v1/transfers/{id}/chunks?cells=3"),
        "the stopped map",
        |map: &ChunkMapInfo| !map.running,
    )
    .await;
    assert_covers(&map, 3);
    assert!(map.cells.iter().all(|cell| cell.have == cell.chunks()));
    let peers: Vec<PeerInfo> = fetch(&api_b, &format!("/v1/transfers/{id}/peers")).await;
    assert!(peers.is_empty());
    let files: Vec<FileInfo> = fetch(&api_b, &format!("/v1/transfers/{id}/files")).await;
    assert!(files.iter().all(|f| f.progress == 1.0), "{files:?}");

    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn before_the_manifest_there_is_nothing_inside() {
    let root = tempfile::tempdir().unwrap();
    let engine = Engine::open(config(root.path())).await.unwrap();
    let api = router(engine.clone(), Access::Local);
    let id = (engine
        .add_link(&nobodys_link(7), AddOptions::default())
        .await)
        .unwrap()
        .id;

    let files: Vec<FileInfo> = fetch(&api, &format!("/v1/transfers/{id}/files")).await;
    assert!(files.is_empty());
    let map: ChunkMapInfo = fetch(&api, &format!("/v1/transfers/{id}/chunks")).await;
    assert_eq!((map.chunks, map.cells.len()), (0, 0));
    let peers: Vec<PeerInfo> = fetch(&api, &format!("/v1/transfers/{id}/peers")).await;
    assert!(peers.iter().all(|peer| !peer.open));

    for route in ["files", "peers", "chunks"] {
        let (status, body) = get(&api, &format!("/v1/transfers/{}/{route}", "0".repeat(64))).await;
        assert_eq!(
            (status, body["error"].as_str()),
            (StatusCode::NOT_FOUND, Some("not_found"))
        );
        let (status, _) = get(&api, &format!("/v1/transfers/nope/{route}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let (status, body) = get(&api, &format!("/v1/transfers/{id}/chunks?cells=many")).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("invalid_request"))
    );

    engine.shutdown().await;
}
