//! LAN discovery on loopback: downloads that know nothing but the bundle ID find a seeder
//! through multicast.

use std::fs;
use std::future::Future;
use std::net::UdpSocket;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tempest_net::{
    Destination, Endpoint, EndpointConfig, Identity, Lan, LanConfig, Node, NodeConfig, Storage,
    StorageConfig,
};
use tempest_protocol::tree::{self, CHUNK_SIZE};
use tempest_protocol::{Address, Entry, FileInfo, Manifest};

const TIMEOUT: Duration = Duration::from_secs(60);

async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(TIMEOUT, future)
        .await
        .expect("timed out")
}

/// A group port of this test's own, so concurrent tests don't hear each other.
fn lan_config() -> LanConfig {
    let port = UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    LanConfig {
        port,
        ..LanConfig::default()
    }
}

/// A node on IPv4 loopback, which LAN discovery announces on loopback.
fn start_node() -> Node {
    let mut config = NodeConfig::default();
    let endpoint_config = EndpointConfig {
        max_open_requests: config.session.upload.max_open_requests,
        ..EndpointConfig::localhost()
    };
    let endpoint = Endpoint::bind(Identity::generate().unwrap(), &endpoint_config).unwrap();
    config.addresses = endpoint
        .local_addrs()
        .into_iter()
        .map(Address::from)
        .collect();
    Node::start(endpoint, config)
}

fn make_bundle(dir: &Path) -> Arc<Manifest> {
    let data: Vec<u8> = (0..CHUNK_SIZE * 12 + 5).map(|i| (i % 251) as u8).collect();
    fs::write(dir.join("lan.bin"), &data).unwrap();
    let info = FileInfo {
        size: data.len() as u64,
        root: tree::root(&data),
        executable: false,
        modified: 1_700_000_000_000,
    };
    Arc::new(Manifest::new("lan", "", 0, "test", vec![Entry::file("lan.bin", info)]).unwrap())
}

/// A bystander without the bundle, a seeder and two downloaders all announce on loopback. The
/// downloaders, given no peers, find the seeder (and dial the bystander, which turns them away)
/// and complete.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn downloads_find_a_seeder_on_the_local_network() {
    within(async {
        let config = lan_config();
        let source = tempfile::tempdir().unwrap();
        let manifest = make_bundle(source.path());

        let bystander = start_node();
        let _bystander_lan = Lan::start(bystander.endpoint(), config.clone()).unwrap();

        let seeder = start_node();
        let seeder_lan = Lan::start(seeder.endpoint(), config.clone()).unwrap();
        let storage =
            Storage::open_shared(source.path(), manifest.clone(), StorageConfig::default())
                .await
                .unwrap();
        let seeding = seeder.share(storage).await.unwrap();
        // A complete bundle wants no peers; watching it does nothing.
        seeder_lan.watch(&seeding).await;

        let mut downloads = Vec::new();
        for _ in 0..2 {
            let node = start_node();
            let lan = Lan::start(node.endpoint(), config.clone()).unwrap();
            let target = tempfile::tempdir().unwrap();
            let destination = Destination::NewFolderIn(target.path().to_owned());
            let handle = node
                .download(manifest.bundle_id(), destination, vec![])
                .await
                .unwrap();
            lan.watch(&handle).await;
            downloads.push((node, lan, target, handle));
        }

        for (_, _, target, handle) in &mut downloads {
            handle
                .status
                .wait_for(|status| status.complete)
                .await
                .expect("the download stopped before completing");
            let got = fs::read(target.path().join("lan.bin")).unwrap();
            assert!(got == fs::read(source.path().join("lan.bin")).unwrap());
        }
        for (node, ..) in &downloads {
            node.shutdown().await;
        }
        seeder.shutdown().await;
        bystander.shutdown().await;
    })
    .await;
}
