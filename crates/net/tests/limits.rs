//! Speed limits on localhost: a bundle's upload limit, and a node's download limit shared by
//! two bundles, hold the measured rate near the limit.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tempest_net::{
    BundleHandle, Endpoint, EndpointConfig, Identity, Node, NodeConfig, Storage, StorageConfig,
};
use tempest_protocol::message::PeerRecord;
use tempest_protocol::tree;
use tempest_protocol::{Address, Entry, FileInfo, Manifest};
use tokio::time::Instant;

const MIB: u64 = 1024 * 1024;

/// How far the measured rate may stray from the limit. Above it by up to a burst and a frame
/// over a few seconds; below it by the time connecting and fetching the manifest take.
const TOLERANCE: (f64, f64) = (0.75, 1.2);

fn start_node() -> Node {
    let config = NodeConfig::default();
    let endpoint_config = EndpointConfig {
        max_open_requests: config.session.upload.max_open_requests,
        ..EndpointConfig::localhost()
    };
    let endpoint = Endpoint::bind(Identity::generate().unwrap(), &endpoint_config).unwrap();
    Node::start(endpoint, config)
}

fn record(node: &Node) -> PeerRecord {
    let endpoint = node.endpoint();
    PeerRecord {
        peer_id: endpoint.peer_id(),
        addresses: endpoint
            .local_addrs()
            .into_iter()
            .map(Address::from)
            .collect(),
        relay: None,
    }
}

/// A bundle named `name` of one file of `size` bytes, written under `dir`.
fn make_bundle(dir: &Path, name: &str, size: u64, seed: u8) -> Arc<Manifest> {
    let data: Vec<u8> = (0..size)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect();
    fs::write(dir.join("data.bin"), &data).unwrap();
    let info = FileInfo {
        size,
        root: tree::root(&data),
        executable: false,
        modified: 1_700_000_000_000,
    };
    Arc::new(Manifest::new(name, "", 0, "test", vec![Entry::file("data.bin", info)]).unwrap())
}

async fn share(node: &Node, dir: &Path, manifest: &Arc<Manifest>) -> BundleHandle {
    let storage = Storage::open_shared(dir, manifest.clone(), StorageConfig::default())
        .await
        .unwrap();
    node.share(storage).await.unwrap()
}

async fn wait_complete(handle: &mut BundleHandle) {
    let wait = handle.status.wait_for(|status| status.complete);
    tokio::time::timeout(Duration::from_secs(60), wait)
        .await
        .expect("timed out")
        .expect("the bundle stopped before completing");
}

fn assert_near(bytes: u64, elapsed: Duration, limit: u64) {
    let rate = bytes as f64 / elapsed.as_secs_f64();
    let ratio = rate / limit as f64;
    assert!(
        (TOLERANCE.0..=TOLERANCE.1).contains(&ratio),
        "{rate:.0} B/s against a limit of {limit} B/s ({ratio:.2}×)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bundles_upload_limit_holds() {
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let size = 4 * MIB;
    let manifest = make_bundle(source.path(), "limited", size, 1);
    let seeder = start_node();
    let seeding = share(&seeder, source.path(), &manifest).await;
    let limit = MIB;
    seeding.limits().upload.set_rate(Some(limit));
    assert_eq!(seeder.limits().upload.rate(), None, "the node has no limit");

    let leecher = start_node();
    let started = Instant::now();
    let mut downloading = leecher
        .download(manifest.bundle_id(), target.path(), vec![record(&seeder)])
        .await
        .unwrap();
    wait_complete(&mut downloading).await;
    assert_near(size, started.elapsed(), limit);

    leecher.shutdown().await;
    seeder.shutdown().await;
    assert_eq!(seeding.finished().await.unwrap().uploaded, size);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_nodes_download_limit_is_shared_by_its_bundles() {
    let size = 2 * MIB;
    let mut seeders = Vec::new();
    let mut sources = Vec::new();
    for seed in 0..2 {
        let source = tempfile::tempdir().unwrap();
        let manifest = make_bundle(source.path(), &format!("bundle{seed}"), size, seed);
        let seeder = start_node();
        let handle = share(&seeder, source.path(), &manifest).await;
        seeders.push((seeder, handle, manifest));
        sources.push(source);
    }

    let leecher = start_node();
    let limit = MIB;
    leecher.limits().download.set_rate(Some(limit));
    let target = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let mut downloads = Vec::new();
    for (seeder, _, manifest) in &seeders {
        let handle = leecher
            .download(
                manifest.bundle_id(),
                target.path().join(manifest.name()),
                vec![record(seeder)],
            )
            .await
            .unwrap();
        downloads.push(handle);
    }
    for handle in &mut downloads {
        wait_complete(handle).await;
    }
    assert_near(2 * size, started.elapsed(), limit);

    leecher.shutdown().await;
    for (seeder, _, _) in &seeders {
        seeder.shutdown().await;
    }
}
